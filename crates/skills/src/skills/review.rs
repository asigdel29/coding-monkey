/*
   File: crates/skills/src/skills/review.rs

   Purpose
   Multi-model pre-merge review of the current branch vs base. Sends
   the unified diff (capped at `max_diff_chars`) to the configured
   LLM with a structured one-line-per-finding output contract; an
   optional `second_opinion=true` runs the alternate provider.

   Findings are deduplicated by (severity, file, line, title) and
   gated against `fail_on`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/skills/src/skills/review.ts
*/

use async_trait::async_trait;
use monkey_core::{TaskType, TokenUsage};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Instant;

use crate::git;
use crate::llm::{LLMClient, LLMRequest, LLMUnavailableError};
use crate::types::{
    merge_usage, Provider, Severity, Skill, SkillContext, SkillFinding, SkillResult,
};

const SYSTEM: &str = "You are a senior staff engineer doing a pre-merge code review.

Be terse. No applause. Focus on issues that will cause real problems:
- Correctness bugs (off-by-one, null deref, race, async misuse)
- Security (injection, missing authz, SSRF, secrets, unsafe deserialization)
- Data safety (irreversible migrations, unguarded deletes, schema breakage)
- API/contract breaks
- Performance landmines (N+1, unbounded queries/loops)

For each issue, output a single line:
SEVERITY | path:line | one-sentence problem | one-sentence fix

Use SEVERITY in {CRITICAL, HIGH, MEDIUM, LOW, INFO}. Do not invent file paths — only cite paths present in the diff. If no real issues, output:
NONE | - | clean | -";

static LINE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)^(CRITICAL|HIGH|MEDIUM|LOW|INFO|NONE)\s*\|\s*([^|]*)\s*\|\s*([^|]+)\s*\|\s*(.*)$",
    )
    .expect("re")
});

#[derive(Debug, Clone, Deserialize)]
struct Input {
    #[serde(default)]
    base: Option<String>,
    #[serde(default, rename = "secondOpinion")]
    second_opinion: bool,
    #[serde(default = "default_fail_on", rename = "failOn")]
    fail_on: String,
    #[serde(default = "default_max_diff", rename = "maxDiffChars")]
    max_diff_chars: usize,
}

fn default_fail_on() -> String {
    "high".into()
}
fn default_max_diff() -> usize {
    120_000
}

impl Default for Input {
    fn default() -> Self {
        Self {
            base: None,
            second_opinion: false,
            fail_on: default_fail_on(),
            max_diff_chars: default_max_diff(),
        }
    }
}

/// `monkey review` skill.
#[derive(Debug, Clone, Copy)]
pub struct Review;

#[async_trait]
impl Skill for Review {
    fn name(&self) -> &str {
        "review"
    }
    fn description(&self) -> &str {
        "Pre-merge multi-model diff review against the base branch"
    }
    fn category(&self) -> &str {
        "review"
    }

    async fn run(
        &self,
        input: serde_json::Value,
        ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        let start = Instant::now();
        let input: Input = if input.is_null() {
            Input::default()
        } else {
            serde_json::from_value(input).unwrap_or_default()
        };

        let cwd = git::repo_root(&ctx.cwd)?;
        let base = input
            .base
            .clone()
            .or_else(|| ctx.base_branch.clone())
            .unwrap_or_else(|| git::default_remote_branch(&cwd));
        let branch = git::current_branch(&cwd)?;

        let files = git::changed_files(&cwd, &base);
        let commits = git::commit_messages(&cwd, &base, 30);

        if files.is_empty() {
            return Ok(SkillResult {
                ok: true,
                summary: format!("no diff vs {base}"),
                findings: Vec::new(),
                duration_ms: start.elapsed().as_millis() as u64,
                ..Default::default()
            });
        }

        let mut diff = git::diff_against(&cwd, &base, false);
        let mut truncated = false;
        if diff.len() > input.max_diff_chars {
            let trimmed = diff[..input.max_diff_chars].to_string();
            let omitted = diff.len() - input.max_diff_chars;
            diff = format!("{trimmed}\n...\n[truncated {omitted} chars]");
            truncated = true;
        }

        let user_prompt = build_user_prompt(&cwd, &branch, &base, &files, &commits, &diff);
        let primary_provider = ctx.provider.unwrap_or(Provider::Anthropic);
        let primary = LLMClient::new(primary_provider);

        let mut findings: Vec<SkillFinding> = Vec::new();
        let mut model_lines: Vec<String> = Vec::new();
        let mut usage: Option<TokenUsage> = None;

        // Primary review.
        match primary
            .complete(LLMRequest {
                task_type: TaskType::Review,
                force_tier: ctx.force_tier,
                provider: Some(primary_provider),
                system: SYSTEM.into(),
                user: user_prompt.clone(),
                max_tokens: Some(2000),
            })
            .await
        {
            Ok(res) => {
                model_lines.push(format!(
                    "primary: {} ({:?})",
                    res.model.display_name, res.model.provider
                ));
                findings.extend(parse_findings(&res.text));
                usage = Some(merge_usage(usage, res.usage));
            }
            Err(err) => {
                if let Some(_unavail) = err.downcast_ref::<LLMUnavailableError>() {
                    return Ok(SkillResult {
                        ok: false,
                        blocked: true,
                        summary: err.to_string(),
                        findings: vec![SkillFinding {
                            severity: Severity::High,
                            title: err.to_string(),
                            file_path: None,
                            line: None,
                            recommendation: None,
                            detail: None,
                        }],
                        duration_ms: start.elapsed().as_millis() as u64,
                        ..Default::default()
                    });
                }
                return Err(err);
            }
        }

        // Optional second opinion.
        if input.second_opinion {
            let alt_provider = match primary_provider {
                Provider::Anthropic => Provider::Openai,
                Provider::Openai => Provider::Anthropic,
            };
            let alt = LLMClient::new(alt_provider);
            match alt
                .complete(LLMRequest {
                    task_type: TaskType::Review,
                    force_tier: None,
                    provider: Some(alt_provider),
                    system: SYSTEM.into(),
                    user: user_prompt.clone(),
                    max_tokens: Some(2000),
                })
                .await
            {
                Ok(res) => {
                    model_lines.push(format!(
                        "second-opinion: {} ({:?})",
                        res.model.display_name, res.model.provider
                    ));
                    findings.extend(parse_findings(&res.text));
                    usage = Some(merge_usage(usage, res.usage));
                }
                Err(err) => {
                    tracing::warn!("second-opinion skipped: {err}");
                }
            }
        }

        let deduped = dedupe(findings);
        let threshold = Severity::from_threshold(&input.fail_on);
        let blocking: Vec<&SkillFinding> = deduped
            .iter()
            .filter(|f| f.severity.rank() >= threshold.rank())
            .collect();
        let ok = blocking.is_empty();
        let summary = if ok {
            format!("clean ({} non-blocking)", deduped.len())
        } else {
            format!("BLOCKED — {} >= {}", blocking.len(), input.fail_on)
        };

        let markdown = build_markdown(&base, &branch, &files, &commits, &deduped, &model_lines);

        Ok(SkillResult {
            ok,
            blocked: !ok,
            summary,
            findings: deduped,
            markdown: Some(markdown),
            duration_ms: start.elapsed().as_millis() as u64,
            usage,
            data: serde_json::json!({
                "base": base,
                "branch": branch,
                "files": files,
                "commits": commits,
                "models": model_lines,
                "truncated": truncated,
            }),
        })
    }
}

fn build_user_prompt(
    cwd: &std::path::Path,
    branch: &str,
    base: &str,
    files: &[String],
    commits: &[String],
    diff: &str,
) -> String {
    let project = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let files_block: String = files
        .iter()
        .take(100)
        .map(|f| format!("  {f}"))
        .collect::<Vec<_>>()
        .join("\n");
    let commits_block: String = commits
        .iter()
        .take(30)
        .map(|c| format!("  {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Repo root: {project}\n\
         Branch: {branch}  Base: {base}\n\
         Files changed ({fcount}):\n{files_block}\n\n\
         Commit log:\n{commits_block}\n\n\
         Unified diff:\n```diff\n{diff}\n```\n\n\
         Output one finding per line in the format described.",
        fcount = files.len()
    )
}

fn parse_findings(text: &str) -> Vec<SkillFinding> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Some(c) = LINE_RE.captures(line) else {
            continue;
        };
        let sev_label = c[1].to_uppercase();
        if sev_label == "NONE" {
            continue;
        }
        let where_ = c[2].trim().to_string();
        let title = c[3].trim().to_string();
        let fix = c[4].trim().to_string();
        let (file_path, line_num) = parse_location(&where_);
        let severity = match sev_label.as_str() {
            "CRITICAL" => Severity::Critical,
            "HIGH" => Severity::High,
            "MEDIUM" => Severity::Medium,
            "LOW" => Severity::Low,
            _ => Severity::Info,
        };
        out.push(SkillFinding {
            severity,
            title,
            file_path,
            line: line_num,
            recommendation: if fix == "-" || fix.is_empty() {
                None
            } else {
                Some(fix)
            },
            detail: None,
        });
    }
    out
}

fn parse_location(where_: &str) -> (Option<String>, Option<usize>) {
    if where_.is_empty() || where_ == "-" {
        return (None, None);
    }
    if let Some(idx) = where_.rfind(':') {
        let tail = &where_[idx + 1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            let line = tail.parse().ok();
            return (Some(where_[..idx].to_string()), line);
        }
    }
    (Some(where_.to_string()), None)
}

fn dedupe(findings: Vec<SkillFinding>) -> Vec<SkillFinding> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(findings.len());
    for f in findings {
        let key = format!(
            "{:?}::{}:{}::{}",
            f.severity,
            f.file_path.as_deref().unwrap_or(""),
            f.line.map(|n| n.to_string()).unwrap_or_default(),
            f.title
        );
        if seen.insert(key) {
            out.push(f);
        }
    }
    out
}

fn build_markdown(
    base: &str,
    branch: &str,
    files: &[String],
    commits: &[String],
    findings: &[SkillFinding],
    model_lines: &[String],
) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Review: {branch} → {base}\n\n"));
    s.push_str(&format!(
        "**Files changed:** {}  •  **Commits:** {}  •  **Findings:** {}\n\n",
        files.len(),
        commits.len(),
        findings.len()
    ));
    s.push_str("## Models\n");
    for ml in model_lines {
        s.push_str(&format!("- {ml}\n"));
    }
    s.push('\n');
    if findings.is_empty() {
        s.push_str("No blockers. Diff is clean.\n");
    } else {
        s.push_str("## Findings\n\n| Severity | Location | Problem | Fix |\n|---|---|---|---|\n");
        for f in findings {
            let loc = match (f.file_path.as_deref(), f.line) {
                (Some(p), Some(n)) => format!("{p}:{n}"),
                (Some(p), None) => p.to_string(),
                _ => "-".into(),
            };
            s.push_str(&format!(
                "| {} | `{loc}` | {} | {} |\n",
                f.severity.upper(),
                f.title,
                f.recommendation.as_deref().unwrap_or("-"),
            ));
        }
    }
    if !commits.is_empty() {
        s.push_str("\n## Commits\n");
        for c in commits {
            s.push_str(&format!("- {c}\n"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_review_lines_into_findings() {
        let raw = "HIGH | src/foo.rs:42 | possible null deref | guard with `?`\n\
                   MEDIUM | src/bar.rs | unbounded query | add LIMIT\n\
                   NONE | - | clean | -\n\
                   garbage line that should be ignored";
        let f = parse_findings(raw);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].severity, Severity::High);
        assert_eq!(f[0].file_path.as_deref(), Some("src/foo.rs"));
        assert_eq!(f[0].line, Some(42));
        assert_eq!(f[1].severity, Severity::Medium);
        assert_eq!(f[1].file_path.as_deref(), Some("src/bar.rs"));
        assert_eq!(f[1].line, None);
    }

    #[test]
    fn dedupes_identical_findings() {
        let f = vec![
            SkillFinding {
                severity: Severity::High,
                title: "x".into(),
                file_path: Some("a.rs".into()),
                line: Some(1),
                recommendation: None,
                detail: None,
            },
            SkillFinding {
                severity: Severity::High,
                title: "x".into(),
                file_path: Some("a.rs".into()),
                line: Some(1),
                recommendation: None,
                detail: None,
            },
        ];
        let out = dedupe(f);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn parse_location_handles_path_and_line() {
        assert_eq!(parse_location("a.rs:12"), (Some("a.rs".into()), Some(12)));
        assert_eq!(parse_location("a.rs"), (Some("a.rs".into()), None));
        assert_eq!(parse_location("-"), (None, None));
        assert_eq!(parse_location(""), (None, None));
        // Windows-style paths with a drive letter colon: still pick up a line if last colon is digits.
        assert_eq!(
            parse_location("C:/x/a.rs:9"),
            (Some("C:/x/a.rs".into()), Some(9))
        );
    }
}
