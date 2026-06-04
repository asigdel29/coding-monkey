/*
   File: crates/skills/src/skills/investigate.rs

   Purpose
   Four-phase root-cause debugging with model-tier escalation.

       Phase 1 (fast)      — locate suspicious places (file:line)
       Phase 2 (balanced)  — identify violated invariants and required evidence
       Phase 3 (powerful)  — pick the single root cause + minimal fix

   The Iron Law: Phase 3 must end with a `VERDICT:` line stating
   CONFIRMED / LIKELY / INSUFFICIENT EVIDENCE. We bail out early on
   CONFIRMED so we don't escalate when we don't need to.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/skills/src/skills/investigate.ts
*/

use async_trait::async_trait;
use monkey_core::{ModelTier, TaskType, TokenUsage};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::llm::{LLMClient, LLMRequest, LLMUnavailableError};
use crate::types::{
    merge_usage, Provider, Severity, Skill, SkillContext, SkillFinding, SkillResult,
};

#[derive(Debug, Clone, Deserialize)]
struct Input {
    symptom: String,
    #[serde(default)]
    hints: Vec<String>,
    #[serde(default = "default_max_snippets", rename = "maxSnippets")]
    max_snippets: usize,
    #[serde(default = "default_snippet_lines", rename = "snippetLines")]
    snippet_lines: usize,
    #[serde(default = "default_max_escalations", rename = "maxEscalations")]
    max_escalations: u8,
}

fn default_max_snippets() -> usize {
    8
}
fn default_snippet_lines() -> usize {
    120
}
fn default_max_escalations() -> u8 {
    2
}

#[derive(Debug, Clone, Copy, Serialize)]
enum Verdict {
    Confirmed,
    Likely,
    InsufficientEvidence,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Confirmed => "CONFIRMED",
            Verdict::Likely => "LIKELY",
            Verdict::InsufficientEvidence => "INSUFFICIENT EVIDENCE",
        }
    }
}

struct Phase {
    name: &'static str,
    tier: ModelTier,
    system: &'static str,
    output_contract: &'static str,
}

const PHASES: &[Phase] = &[
    Phase {
        name: "1-investigate",
        tier: ModelTier::Fast,
        system: "You are a debugging investigator. Given a symptom and code snippets, list the 3-5 most likely *places* something is wrong (file:line). Do NOT propose fixes yet. Be concrete.",
        output_contract: "Output a numbered list. For each item: file:line — one-sentence reason.",
    },
    Phase {
        name: "2-analyze",
        tier: ModelTier::Balanced,
        system: "You are a debugging analyst. Given investigation candidates, examine them and identify which *invariants* the symptom violates. Trace the data flow. State what observable evidence would confirm each hypothesis.",
        output_contract: "Output: VIOLATED INVARIANT — TRACE — EVIDENCE NEEDED, one block per hypothesis.",
    },
    Phase {
        name: "3-hypothesize",
        tier: ModelTier::Powerful,
        system: "You are a debugging chief. Given hypotheses, pick the single most likely root cause and explain why with file:line references. State a minimal repro and the smallest possible fix.\n\nIron Law: do NOT propose a fix without a clear root cause. If the evidence is insufficient, say \"INSUFFICIENT EVIDENCE\" and list exactly what to gather.",
        output_contract: "Sections: ROOT CAUSE, REPRO, FIX (file:line + concrete change), CONFIDENCE (0-10). End with the line: VERDICT: <CONFIRMED|LIKELY|INSUFFICIENT EVIDENCE>",
    },
];

static VERDICT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)VERDICT\s*:\s*(CONFIRMED|LIKELY|INSUFFICIENT EVIDENCE)").expect("re")
});

/// `monkey investigate` skill.
#[derive(Debug, Clone, Copy)]
pub struct Investigate;

#[async_trait]
impl Skill for Investigate {
    fn name(&self) -> &str {
        "investigate"
    }
    fn description(&self) -> &str {
        "Four-phase root-cause debugging with model-tier escalation"
    }
    fn category(&self) -> &str {
        "debug"
    }

    async fn run(
        &self,
        input: serde_json::Value,
        ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        let start = Instant::now();
        let input: Input = serde_json::from_value(input)
            .map_err(|e| anyhow::anyhow!("invalid investigate input: {e}"))?;
        if input.symptom.trim().len() < 3 {
            return Err(anyhow::anyhow!("symptom must be at least 3 chars"));
        }

        let candidates =
            pick_candidate_files(&ctx.cwd, &input.symptom, &input.hints, input.max_snippets);
        let snippets = build_snippet_section(&ctx.cwd, &candidates, input.snippet_lines);

        let llm = LLMClient::new(ctx.provider.unwrap_or(Provider::OpenRouter));
        let phase_limit = std::cmp::min(PHASES.len(), input.max_escalations as usize + 1);

        let mut transcripts: Vec<(String, String, String)> = Vec::new();
        let mut usage: Option<TokenUsage> = None;
        let mut prior_output = String::new();

        for phase in &PHASES[..phase_limit] {
            let tier = ctx.force_tier.unwrap_or(phase.tier);
            let mut user_body = format!("Symptom:\n{}\n", input.symptom);
            if !candidates.is_empty() {
                let list = candidates
                    .iter()
                    .map(|c| format!("- {}", c.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                user_body.push_str(&format!("\nCandidate files:\n{list}\n"));
            }
            if !prior_output.is_empty() {
                user_body.push_str(&format!("\nPrior phase output:\n{prior_output}\n"));
            }
            if !snippets.is_empty() {
                user_body.push_str(&format!("\nCode:\n{snippets}\n"));
            }
            user_body.push_str(&format!("\nContract: {}\n", phase.output_contract));

            let res = llm
                .complete(LLMRequest {
                    task_type: TaskType::Investigate,
                    force_tier: Some(tier),
                    provider: ctx.provider,
                    system: phase.system.into(),
                    user: user_body,
                    max_tokens: Some(1500),
                })
                .await;
            match res {
                Ok(res) => {
                    usage = Some(merge_usage(usage, res.usage.clone()));
                    transcripts.push((
                        phase.name.to_string(),
                        res.model.display_name.clone(),
                        res.text.clone(),
                    ));
                    prior_output = res.text.clone();
                    if matches!(extract_verdict(&res.text), Some(Verdict::Confirmed)) {
                        break;
                    }
                }
                Err(err) => {
                    if let Some(u) = err.downcast_ref::<LLMUnavailableError>() {
                        return Ok(SkillResult {
                            ok: false,
                            blocked: true,
                            summary: u.0.clone(),
                            findings: vec![SkillFinding {
                                severity: Severity::High,
                                title: u.0.clone(),
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
        }

        let last_output = transcripts.last().map(|t| t.2.clone()).unwrap_or_default();
        let verdict = extract_verdict(&last_output);

        let mut findings = Vec::new();
        if let Some(v) = verdict {
            match v {
                Verdict::InsufficientEvidence => findings.push(SkillFinding {
                    severity: Severity::Medium,
                    title: "Insufficient evidence — gather more data before fixing".into(),
                    file_path: None,
                    line: None,
                    recommendation: None,
                    detail: Some(last_output.clone()),
                }),
                Verdict::Confirmed => findings.push(SkillFinding {
                    severity: Severity::High,
                    title: "Root cause confirmed".into(),
                    file_path: None,
                    line: None,
                    recommendation: None,
                    detail: Some(last_output.clone()),
                }),
                Verdict::Likely => findings.push(SkillFinding {
                    severity: Severity::Medium,
                    title: "Root cause likely".into(),
                    file_path: None,
                    line: None,
                    recommendation: None,
                    detail: Some(last_output.clone()),
                }),
            }
        }

        let symptom_head: String = input.symptom.chars().take(80).collect();
        let mut md = String::new();
        md.push_str(&format!("# Investigate: {symptom_head}\n\n"));
        md.push_str(&format!(
            "**Phases run:** {}  **Verdict:** {}\n\n",
            transcripts.len(),
            verdict.map(|v| v.label()).unwrap_or("n/a"),
        ));
        for (name, model, text) in &transcripts {
            md.push_str(&format!("## {name} — {model}\n\n{text}\n\n"));
        }

        let summary = match verdict {
            Some(v) => format!(
                "verdict: {} after {} phase{}",
                v.label().to_lowercase(),
                transcripts.len(),
                if transcripts.len() == 1 { "" } else { "s" }
            ),
            None => format!(
                "inconclusive after {} phase{}",
                transcripts.len(),
                if transcripts.len() == 1 { "" } else { "s" }
            ),
        };
        let ok = !matches!(verdict, Some(Verdict::InsufficientEvidence));

        Ok(SkillResult {
            ok,
            blocked: !ok,
            summary,
            findings,
            markdown: Some(md),
            duration_ms: start.elapsed().as_millis() as u64,
            usage,
            data: serde_json::json!({
                "candidates": candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "verdict": verdict.map(|v| v.label()),
            }),
        })
    }
}

fn extract_verdict(text: &str) -> Option<Verdict> {
    let cap = VERDICT_RE.captures(text)?;
    match cap[1].to_uppercase().as_str() {
        "CONFIRMED" => Some(Verdict::Confirmed),
        "LIKELY" => Some(Verdict::Likely),
        "INSUFFICIENT EVIDENCE" => Some(Verdict::InsufficientEvidence),
        _ => None,
    }
}

fn safe_read(file: &Path, lines_cap: usize) -> Option<String> {
    let content = std::fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= lines_cap {
        return Some(content);
    }
    let head = lines[..lines_cap].join("\n");
    Some(format!(
        "{head}\n... [+{} more lines]",
        lines.len() - lines_cap
    ))
}

fn ripgrep(cwd: &Path, query: &str, max_files: usize) -> Vec<String> {
    let out = Command::new("rg")
        .current_dir(cwd)
        .args([
            "-l",
            "--hidden",
            "-S",
            "-g",
            "!**/node_modules/**",
            "-g",
            "!**/dist/**",
            "-g",
            "!**/.git/**",
            "-g",
            "!**/target/**",
            "--",
            query,
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() && out.status.code() != Some(1) {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(max_files)
        .collect()
}

fn build_snippet_section(cwd: &Path, files: &[PathBuf], lines: usize) -> String {
    let mut blocks = Vec::new();
    for f in files {
        let abs = if f.is_absolute() {
            f.clone()
        } else {
            cwd.join(f)
        };
        let Some(body) = safe_read(&abs, lines) else {
            continue;
        };
        let rel = abs.strip_prefix(cwd).unwrap_or(&abs);
        blocks.push(format!("### {}\n```\n{body}\n```", rel.display()));
    }
    blocks.join("\n\n")
}

fn pick_candidate_files(cwd: &Path, symptom: &str, hints: &[String], max: usize) -> Vec<PathBuf> {
    let mut files: BTreeSet<String> = hints.iter().cloned().collect();
    static TOKEN_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]{3,}").expect("re"));
    static STOPWORD_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^(?i)(error|warning|failed|expected|received|undefined|cannot|module|cause)$")
            .expect("re")
    });
    let tokens: Vec<&str> = TOKEN_RE
        .find_iter(symptom)
        .map(|m| m.as_str())
        .filter(|t| !STOPWORD_RE.is_match(t))
        .take(6)
        .collect();
    for tok in tokens {
        if files.len() >= max {
            break;
        }
        for f in ripgrep(cwd, tok, 4) {
            if files.len() >= max {
                break;
            }
            files.insert(f);
        }
    }
    files.into_iter().take(max).map(PathBuf::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_verdict_case_insensitive() {
        assert!(matches!(
            extract_verdict("…\nVERDICT: CONFIRMED"),
            Some(Verdict::Confirmed)
        ));
        assert!(matches!(
            extract_verdict("verdict : likely"),
            Some(Verdict::Likely)
        ));
        assert!(matches!(
            extract_verdict("VERDICT: INSUFFICIENT EVIDENCE"),
            Some(Verdict::InsufficientEvidence)
        ));
        assert!(extract_verdict("no verdict here").is_none());
    }

    #[test]
    fn safe_read_truncates_long_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let body = (0..200)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &body).unwrap();
        let r = safe_read(&path, 10).unwrap();
        assert!(r.contains("more lines"));
        assert!(r.starts_with("line0"));
    }
}
