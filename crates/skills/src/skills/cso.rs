/*
   File: crates/skills/src/skills/cso.rs

   Purpose
   CSO security audit. Composes:

     1. quick_secret_sweep  — high-confidence regex sweep over .env*
        files (always runs, no LLM).
     2. monkey-engulf scan + audit — full static-plus-LLM analysis.
     3. dep_audit            — pnpm/npm audit for advisories.
     4. monkey-pentest-agent — whitebox by default, blackbox when
        --pentest-target is set or mode=comprehensive.

   Aggregates findings, gates against `fail_on`, and renders a
   single markdown summary.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/skills/src/skills/cso.ts
*/

use async_trait::async_trait;
use monkey_engulf::{
    run_security_audit_with, AuditOptions, CodebaseScanner, Provider as EngulfProvider,
};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::types::{Provider, Severity, Skill, SkillContext, SkillFinding, SkillResult};

#[derive(Debug, Clone, Deserialize)]
struct Input {
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default, rename = "pentestTarget")]
    pentest_target: Option<String>,
    #[serde(default = "default_fail_on", rename = "failOn")]
    fail_on: String,
    #[serde(default, rename = "skipLLM")]
    skip_llm: bool,
}

fn default_mode() -> String {
    "daily".into()
}
fn default_fail_on() -> String {
    "high".into()
}

/// `monkey cso` skill.
#[derive(Debug, Clone, Copy)]
pub struct Cso;

#[async_trait]
impl Skill for Cso {
    fn name(&self) -> &str {
        "cso"
    }
    fn description(&self) -> &str {
        "CSO security audit — engulf scan + dep advisories + optional pentest"
    }
    fn category(&self) -> &str {
        "security"
    }
    fn composes(&self) -> &[&str] {
        &["pentest"]
    }

    async fn run(
        &self,
        input: serde_json::Value,
        ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        let start = Instant::now();
        let input: Input = serde_json::from_value(input).unwrap_or(Input {
            mode: default_mode(),
            pentest_target: None,
            fail_on: default_fail_on(),
            skip_llm: false,
        });
        let cwd = &ctx.cwd;
        let mut findings: Vec<SkillFinding> = Vec::new();

        // 1. Quick secret sweep.
        findings.extend(quick_secret_sweep(cwd));

        // 2. engulf scan + audit.
        let scan = CodebaseScanner::new(cwd).scan();
        let provider = match ctx.provider.unwrap_or(Provider::Anthropic) {
            Provider::Anthropic => EngulfProvider::Anthropic,
            Provider::Openai => EngulfProvider::Openai,
        };
        let audit = run_security_audit_with(
            &scan,
            AuditOptions {
                provider: Some(provider),
                skip_llm: input.skip_llm,
                ..Default::default()
            },
        )
        .await?;
        for f in &audit.findings {
            let sev = match f.severity {
                monkey_engulf::Severity::Info => Severity::Low,
                monkey_engulf::Severity::Low => Severity::Low,
                monkey_engulf::Severity::Medium => Severity::Medium,
                monkey_engulf::Severity::High => Severity::High,
                monkey_engulf::Severity::Critical => Severity::Critical,
            };
            findings.push(SkillFinding {
                severity: sev,
                title: f.title.clone(),
                file_path: f.file_path.clone(),
                line: f.line_number,
                recommendation: Some(f.recommendation.clone()),
                detail: Some(f.description.clone()),
            });
        }

        // 3. Dependency advisories.
        let dep = dep_audit(cwd);
        if dep.advisories > 0 {
            findings.push(SkillFinding {
                severity: if dep.advisories > 5 {
                    Severity::High
                } else {
                    Severity::Medium
                },
                title: format!("{} dependency advisories from {}", dep.advisories, dep.tool),
                file_path: None,
                line: None,
                recommendation: Some(format!(
                    "Run \"{} audit --fix\" or upgrade affected packages.",
                    if dep.tool == "pnpm-audit" {
                        "pnpm"
                    } else {
                        "npm"
                    }
                )),
                detail: None,
            });
        }

        // 4. Optional pentest.
        let mut pentest_ran = false;
        if input.mode == "comprehensive" || input.pentest_target.is_some() {
            pentest_ran = true;
            let opts = monkey_pentest_agent::runner::PentestOpts {
                target: input.pentest_target.clone(),
                cwd: Some(cwd.clone()),
                fail_on: severity_to_pentest(Severity::from_threshold(&input.fail_on)),
                ..Default::default()
            };
            match monkey_pentest_agent::runner::run_pentest(opts).await {
                Ok(r) => {
                    for f in r.findings {
                        let sev = match f.severity {
                            monkey_pentest_agent::runner::Severity::Info => Severity::Low,
                            monkey_pentest_agent::runner::Severity::Low => Severity::Low,
                            monkey_pentest_agent::runner::Severity::Medium => Severity::Medium,
                            monkey_pentest_agent::runner::Severity::High => Severity::High,
                            monkey_pentest_agent::runner::Severity::Critical => Severity::Critical,
                        };
                        findings.push(SkillFinding {
                            severity: sev,
                            title: f.title,
                            file_path: f.location.clone(),
                            line: None,
                            recommendation: None,
                            detail: Some(f.detail),
                        });
                    }
                }
                Err(err) => tracing::warn!("pentest skipped: {err}"),
            }
        }

        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for f in &findings {
            *counts.entry(f.severity.upper()).or_insert(0) += 1;
        }

        let threshold = Severity::from_threshold(&input.fail_on);
        let blocking: Vec<&SkillFinding> = findings
            .iter()
            .filter(|f| f.severity.rank() >= threshold.rank())
            .collect();
        let ok = blocking.is_empty();

        let summary = if ok {
            format!("clean ({} non-blocking)", findings.len())
        } else {
            format!("BLOCKED — {} >= {}", blocking.len(), input.fail_on)
        };

        let mut md = format!("# CSO Audit ({})\n\n", input.mode);
        md.push_str(&format!(
            "**Files scanned:** {}  •  **Dependencies:** {}  •  **Advisories:** {}\n\n",
            scan.files.len(),
            scan.dependencies.len(),
            dep.advisories
        ));
        md.push_str("## Counts\n\n");
        if counts.is_empty() {
            md.push_str("- none\n");
        } else {
            let mut entries: Vec<_> = counts.iter().collect();
            entries.sort_by_key(|(k, _)| *k);
            for (k, v) in entries {
                md.push_str(&format!("- {k}: {v}\n"));
            }
        }
        md.push_str("\n## Top findings\n\n");
        if findings.is_empty() {
            md.push_str("_none_\n");
        } else {
            for f in findings.iter().take(20) {
                let loc = match (f.file_path.as_deref(), f.line) {
                    (Some(p), Some(n)) => format!(" — `{p}:{n}`"),
                    (Some(p), None) => format!(" — `{p}`"),
                    _ => String::new(),
                };
                md.push_str(&format!("- **{}** {}{loc}\n", f.severity.upper(), f.title));
            }
        }

        Ok(SkillResult {
            ok,
            blocked: !ok,
            summary,
            findings,
            markdown: Some(md),
            duration_ms: start.elapsed().as_millis() as u64,
            usage: None,
            data: serde_json::json!({
                "mode": input.mode,
                "engulf": {
                    "summary": audit.summary,
                    "counts": {
                        "critical": audit.critical_count,
                        "high": audit.high_count,
                        "medium": audit.medium_count,
                        "low": audit.low_count,
                    },
                },
                "dependencies": { "tool": dep.tool, "advisories": dep.advisories },
                "pentest_ran": pentest_ran,
            }),
        })
    }
}

fn severity_to_pentest(s: Severity) -> monkey_pentest_agent::runner::Severity {
    match s {
        Severity::Critical => monkey_pentest_agent::runner::Severity::Critical,
        Severity::High => monkey_pentest_agent::runner::Severity::High,
        Severity::Medium => monkey_pentest_agent::runner::Severity::Medium,
        Severity::Low => monkey_pentest_agent::runner::Severity::Low,
        Severity::Info => monkey_pentest_agent::runner::Severity::Info,
    }
}

#[derive(Debug)]
struct DepAudit {
    advisories: usize,
    tool: String,
}

fn dep_audit(cwd: &Path) -> DepAudit {
    if let Some(r) = try_pnpm_audit(cwd) {
        return r;
    }
    if let Some(r) = try_npm_audit(cwd) {
        return r;
    }
    DepAudit {
        advisories: 0,
        tool: "none".into(),
    }
}

fn try_pnpm_audit(cwd: &Path) -> Option<DepAudit> {
    let out = Command::new("pnpm")
        .current_dir(cwd)
        .args(["audit", "--json"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let adv = json
        .get("metadata")
        .and_then(|m| m.get("vulnerabilities"))
        .and_then(|v| v.as_object())?;
    let total: usize = adv
        .values()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .sum();
    Some(DepAudit {
        advisories: total,
        tool: "pnpm-audit".into(),
    })
}

fn try_npm_audit(cwd: &Path) -> Option<DepAudit> {
    let out = Command::new("npm")
        .current_dir(cwd)
        .args(["audit", "--json"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let total = json
        .get("metadata")
        .and_then(|m| m.get("vulnerabilities"))
        .and_then(|v| v.get("total"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0) as usize;
    Some(DepAudit {
        advisories: total,
        tool: "npm-audit".into(),
    })
}

static SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"sk-(ant-)?[A-Za-z0-9_\-]{20,}",
        r"AKIA[0-9A-Z]{16}",
        r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----",
        r"ghp_[A-Za-z0-9]{30,}",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("re"))
    .collect()
});

fn looks_like_secret(line: &str) -> bool {
    SECRET_PATTERNS.iter().any(|re| re.is_match(line))
}

fn quick_secret_sweep(cwd: &Path) -> Vec<SkillFinding> {
    let mut out = Vec::new();
    for c in [".env", ".env.local", ".env.production", "config.json"] {
        let fp = cwd.join(c);
        let Ok(txt) = std::fs::read_to_string(&fp) else {
            continue;
        };
        for (i, line) in txt.lines().enumerate() {
            if looks_like_secret(line) {
                out.push(SkillFinding {
                    severity: Severity::Critical,
                    title: format!("Secret-like value in committed file {c}"),
                    file_path: Some(c.to_string()),
                    line: Some(i + 1),
                    recommendation: Some(
                        "Remove from version control, rotate the secret, add to .gitignore.".into(),
                    ),
                    detail: None,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_secret_matches_known_shapes() {
        assert!(looks_like_secret(
            "KEY=sk-ant-api03-1234567890abcdefghij1234"
        ));
        assert!(looks_like_secret("AKIA0123456789ABCDEF"));
        assert!(looks_like_secret("-----BEGIN PRIVATE KEY-----"));
        assert!(looks_like_secret(
            "GH=ghp_1234567890123456789012345678901234567890"
        ));
        assert!(!looks_like_secret("just a normal log line"));
    }

    #[test]
    fn quick_secret_sweep_finds_committed_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "OPENAI_KEY=sk-proj-1234567890ABCDEFGHIJKLMNOP\nharmless=value\n",
        )
        .unwrap();
        let f = quick_secret_sweep(dir.path());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Critical);
        assert_eq!(f[0].line, Some(1));
    }
}
