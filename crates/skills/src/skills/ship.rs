/*
   File: crates/skills/src/skills/ship.rs

   Purpose
   End-to-end ship gauntlet — clean tree → install/typecheck → review →
   cso → MANDATORY pentest → version bump → push → optional GitHub PR.

   Each stage is gated against `fail_on`. The first blocking failure
   halts the gauntlet and the result reflects the failed stage.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/skills/src/skills/ship.ts
*/

use async_trait::async_trait;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::git;
use crate::skills::{Cso, Review};
use crate::types::{Provider, Severity, Skill, SkillContext, SkillFinding, SkillResult};

#[derive(Debug, Clone, Deserialize)]
struct Input {
    #[serde(default = "default_bump")]
    bump: String,
    #[serde(default, rename = "skipBuild")]
    skip_build: bool,
    #[serde(default = "default_true")]
    review: bool,
    #[serde(default = "default_true")]
    audit: bool,
    #[serde(default = "default_true")]
    pentest: bool,
    #[serde(default, rename = "pentestTarget")]
    pentest_target: Option<String>,
    #[serde(default = "default_true")]
    push: bool,
    #[serde(default, rename = "openPr")]
    open_pr: bool,
    #[serde(default = "default_fail_on", rename = "failOn")]
    fail_on: String,
}

fn default_true() -> bool {
    true
}
fn default_bump() -> String {
    "patch".into()
}
fn default_fail_on() -> String {
    "high".into()
}

#[derive(Debug, Clone)]
struct Stage {
    name: &'static str,
    ok: bool,
    detail: String,
    blocking: bool,
}

/// `monkey ship` skill.
#[derive(Debug, Clone, Copy)]
pub struct Ship;

#[async_trait]
impl Skill for Ship {
    fn name(&self) -> &str {
        "ship"
    }
    fn description(&self) -> &str {
        "Ship gauntlet: typecheck → review → cso → pentest → bump → push"
    }
    fn category(&self) -> &str {
        "release"
    }
    fn composes(&self) -> &[&str] {
        &["review", "cso", "pentest"]
    }

    async fn run(
        &self,
        input: serde_json::Value,
        ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        let start = Instant::now();
        let input: Input = serde_json::from_value(input).unwrap_or(Input {
            bump: default_bump(),
            skip_build: false,
            review: true,
            audit: true,
            pentest: true,
            pentest_target: None,
            push: true,
            open_pr: false,
            fail_on: default_fail_on(),
        });

        let cwd = git::repo_root(&ctx.cwd)?;
        let branch = git::current_branch(&cwd)?;
        let base = ctx
            .base_branch
            .clone()
            .unwrap_or_else(|| git::default_remote_branch(&cwd));

        let mut stages: Vec<Stage> = Vec::new();
        let mut findings: Vec<SkillFinding> = Vec::new();

        if branch == base {
            return Ok(finalize(
                start,
                &branch,
                &base,
                &[Stage {
                    name: "branch-guard",
                    ok: false,
                    blocking: true,
                    detail: format!("on base branch {base}"),
                }],
                vec![SkillFinding {
                    severity: Severity::High,
                    title: "On base branch".into(),
                    file_path: None,
                    line: None,
                    recommendation: Some("Create a feature branch first.".into()),
                    detail: None,
                }],
                serde_json::Value::Null,
            ));
        }

        // Stage 1: clean tree.
        let clean = git::is_clean(&cwd);
        push_stage(
            &mut stages,
            Stage {
                name: "clean-tree",
                ok: clean,
                blocking: true,
                detail: "git status --porcelain".into(),
            },
        );
        if !clean {
            findings.push(SkillFinding {
                severity: Severity::High,
                title: "Working tree dirty — commit or stash before shipping".into(),
                file_path: None,
                line: None,
                recommendation: None,
                detail: None,
            });
            return Ok(finalize(
                start,
                &branch,
                &base,
                &stages,
                findings,
                serde_json::Value::Null,
            ));
        }

        // Stage 2: install + typecheck (skippable).
        if !input.skip_build {
            let pm = detect_pm(&cwd);
            let install = run_shell(
                "install",
                &pm,
                &["install", "--frozen-lockfile"],
                &cwd,
                true,
            );
            push_stage(&mut stages, install);

            let typecheck = run_shell("typecheck", &pm, &["run", "-r", "typecheck"], &cwd, true);
            // typecheck script not present? swallow.
            if !typecheck.ok
                && (typecheck.detail.to_lowercase().contains("missing script")
                    || typecheck.detail.to_lowercase().contains("unknown command"))
            {
                push_stage(
                    &mut stages,
                    Stage {
                        name: "typecheck",
                        ok: true,
                        blocking: false,
                        detail: "skipped — no typecheck script".into(),
                    },
                );
            } else {
                let ok = typecheck.ok;
                let detail = typecheck.detail.clone();
                push_stage(&mut stages, typecheck);
                if !ok {
                    findings.push(SkillFinding {
                        severity: Severity::High,
                        title: "Typecheck failed".into(),
                        file_path: None,
                        line: None,
                        recommendation: None,
                        detail: Some(detail),
                    });
                    return Ok(finalize(
                        start,
                        &branch,
                        &base,
                        &stages,
                        findings,
                        serde_json::Value::Null,
                    ));
                }
            }
        }

        // Stage 3: review.
        if input.review {
            let r = Review
                .run(
                    serde_json::json!({
                        "base": base.clone(),
                        "secondOpinion": false,
                        "failOn": input.fail_on,
                        "maxDiffChars": 120_000,
                    }),
                    ctx,
                )
                .await?;
            push_stage(
                &mut stages,
                Stage {
                    name: "review",
                    ok: r.ok,
                    blocking: true,
                    detail: r.summary.clone(),
                },
            );
            findings.extend(r.findings.iter().cloned());
            if !r.ok {
                return Ok(finalize(
                    start,
                    &branch,
                    &base,
                    &stages,
                    findings,
                    serde_json::Value::Null,
                ));
            }
        }

        // Stage 4: cso audit.
        if input.audit {
            let r = Cso
                .run(
                    serde_json::json!({
                        "mode": "daily",
                        "failOn": input.fail_on,
                        "skipLLM": false,
                    }),
                    ctx,
                )
                .await?;
            push_stage(
                &mut stages,
                Stage {
                    name: "cso-audit",
                    ok: r.ok,
                    blocking: true,
                    detail: r.summary.clone(),
                },
            );
            findings.extend(r.findings.iter().cloned());
            if !r.ok {
                return Ok(finalize(
                    start,
                    &branch,
                    &base,
                    &stages,
                    findings,
                    serde_json::Value::Null,
                ));
            }
        }

        // Stage 5: mandatory pentest.
        if input.pentest {
            let opts = monkey_pentest_agent::runner::PentestOpts {
                target: input.pentest_target.clone(),
                cwd: Some(cwd.clone()),
                fail_on: severity_to_pentest(Severity::from_threshold(&input.fail_on)),
                ..Default::default()
            };
            match monkey_pentest_agent::run_pre_push_pentest(opts).await {
                Ok(r) => {
                    push_stage(
                        &mut stages,
                        Stage {
                            name: "pentest",
                            ok: r.ok,
                            blocking: true,
                            detail: format!("findings={}", r.findings.len()),
                        },
                    );
                    if !r.ok {
                        findings.push(SkillFinding {
                            severity: Severity::Critical,
                            title: "Pentest blocked the ship".into(),
                            file_path: None,
                            line: None,
                            recommendation: None,
                            detail: None,
                        });
                        return Ok(finalize(
                            start,
                            &branch,
                            &base,
                            &stages,
                            findings,
                            serde_json::Value::Null,
                        ));
                    }
                }
                Err(err) => {
                    push_stage(
                        &mut stages,
                        Stage {
                            name: "pentest",
                            ok: false,
                            blocking: true,
                            detail: err.to_string(),
                        },
                    );
                    findings.push(SkillFinding {
                        severity: Severity::Critical,
                        title: "Pentest failed to complete".into(),
                        file_path: None,
                        line: None,
                        recommendation: None,
                        detail: Some(err.to_string()),
                    });
                    return Ok(finalize(
                        start,
                        &branch,
                        &base,
                        &stages,
                        findings,
                        serde_json::Value::Null,
                    ));
                }
            }
        }

        // Stage 6: version bump.
        if input.bump != "none" {
            let kind = match input.bump.as_str() {
                "minor" => git::BumpKind::Minor,
                "major" => git::BumpKind::Major,
                _ => git::BumpKind::Patch,
            };
            match git::bump_version(&cwd, kind)? {
                Some(b) => {
                    let file_str = b.file.to_string_lossy().into_owned();
                    git::add(&cwd, &[&file_str]).ok();
                    push_stage(
                        &mut stages,
                        Stage {
                            name: "version-bump",
                            ok: true,
                            blocking: false,
                            detail: format!("{} → {}", b.from, b.to),
                        },
                    );
                    let _ = git::commit(&cwd, &format!("chore: bump version to {}", b.to));
                }
                None => push_stage(
                    &mut stages,
                    Stage {
                        name: "version-bump",
                        ok: true,
                        blocking: false,
                        detail: "no package.json/VERSION found — skipped".into(),
                    },
                ),
            }
        }

        // Stage 7: nothing to push?
        let ahead = git::commit_messages(&cwd, &base, 100);
        if ahead.is_empty() {
            push_stage(
                &mut stages,
                Stage {
                    name: "diff-vs-base",
                    ok: false,
                    blocking: true,
                    detail: "no commits ahead of base".into(),
                },
            );
            findings.push(SkillFinding {
                severity: Severity::Medium,
                title: "Nothing to push".into(),
                file_path: None,
                line: None,
                recommendation: Some("Commit your changes first.".into()),
                detail: None,
            });
            return Ok(finalize(
                start,
                &branch,
                &base,
                &stages,
                findings,
                serde_json::json!({ "ahead": 0 }),
            ));
        }

        // Stage 8: push.
        if input.push {
            if ctx.dry_run {
                push_stage(
                    &mut stages,
                    Stage {
                        name: "push",
                        ok: true,
                        blocking: false,
                        detail: "dry-run: would push".into(),
                    },
                );
            } else if !git::has_remote(&cwd, "origin") {
                push_stage(
                    &mut stages,
                    Stage {
                        name: "push",
                        ok: false,
                        blocking: true,
                        detail: "no remote 'origin'".into(),
                    },
                );
                findings.push(SkillFinding {
                    severity: Severity::High,
                    title: "No git remote configured".into(),
                    file_path: None,
                    line: None,
                    recommendation: None,
                    detail: None,
                });
                return Ok(finalize(
                    start,
                    &branch,
                    &base,
                    &stages,
                    findings,
                    serde_json::Value::Null,
                ));
            } else {
                match git::push(&cwd, &branch, "origin") {
                    Ok(()) => push_stage(
                        &mut stages,
                        Stage {
                            name: "push",
                            ok: true,
                            blocking: false,
                            detail: format!("pushed {branch}"),
                        },
                    ),
                    Err(e) => {
                        push_stage(
                            &mut stages,
                            Stage {
                                name: "push",
                                ok: false,
                                blocking: true,
                                detail: e.to_string(),
                            },
                        );
                        findings.push(SkillFinding {
                            severity: Severity::High,
                            title: "git push failed".into(),
                            file_path: None,
                            line: None,
                            recommendation: None,
                            detail: Some(e.to_string()),
                        });
                        return Ok(finalize(
                            start,
                            &branch,
                            &base,
                            &stages,
                            findings,
                            serde_json::Value::Null,
                        ));
                    }
                }
            }
        }

        // Stage 9: optional PR via gh.
        let mut data = serde_json::json!({
            "branch": branch,
            "base": base,
            "ahead": ahead.len(),
        });
        if input.open_pr {
            let subject = ahead
                .first()
                .cloned()
                .unwrap_or_else(|| format!("Ship {branch}"));
            let subject: String = subject.chars().take(70).collect();
            let body = "Automated ship via `monkey ship`.";
            let pr = Command::new("gh")
                .current_dir(&cwd)
                .args([
                    "pr", "create", "--title", &subject, "--body", body, "--base", &base,
                ])
                .output();
            match pr {
                Ok(o) if o.status.success() => {
                    let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    push_stage(
                        &mut stages,
                        Stage {
                            name: "pr",
                            ok: true,
                            blocking: false,
                            detail: url.clone(),
                        },
                    );
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert("pr".into(), serde_json::json!({ "url": url }));
                    }
                }
                Ok(o) => {
                    let detail = String::from_utf8_lossy(&o.stderr)
                        .chars()
                        .take(500)
                        .collect();
                    push_stage(
                        &mut stages,
                        Stage {
                            name: "pr",
                            ok: false,
                            blocking: false,
                            detail,
                        },
                    );
                }
                Err(e) => push_stage(
                    &mut stages,
                    Stage {
                        name: "pr",
                        ok: false,
                        blocking: false,
                        detail: e.to_string(),
                    },
                ),
            }
        }

        // Tag the unused Provider import as intentional — used via downstream
        // skill ctx propagation if a future flag toggles provider per stage.
        let _ = Provider::OpenRouter;
        Ok(finalize(start, &branch, &base, &stages, findings, data))
    }
}

fn push_stage(stages: &mut Vec<Stage>, s: Stage) {
    stages.push(s);
}

fn finalize(
    start: Instant,
    branch: &str,
    base: &str,
    stages: &[Stage],
    findings: Vec<SkillFinding>,
    data: serde_json::Value,
) -> SkillResult {
    let blocked = stages.iter().any(|s| !s.ok && s.blocking);
    let mut md = format!("# Ship: {branch} → {base}\n\n");
    md.push_str(&format!(
        "**Outcome:** {}\n\n",
        if blocked { "BLOCKED" } else { "SHIPPED" }
    ));
    md.push_str("## Stages\n\n| # | Stage | Result | Detail |\n|---|---|---|---|\n");
    for (i, s) in stages.iter().enumerate() {
        let mark = if s.ok { "ok  " } else { "fail" };
        let one_line = s.detail.lines().next().unwrap_or("");
        md.push_str(&format!(
            "| {} | {} | {mark} | {one_line} |\n",
            i + 1,
            s.name
        ));
    }

    let summary = if blocked {
        let failed = stages
            .iter()
            .find(|s| !s.ok)
            .map(|s| s.name)
            .unwrap_or("unknown");
        format!("BLOCKED at {failed}")
    } else {
        format!("shipped {branch} ({} stages, base {base})", stages.len())
    };
    SkillResult {
        ok: !blocked,
        blocked,
        summary,
        findings,
        markdown: Some(md),
        duration_ms: start.elapsed().as_millis() as u64,
        usage: None,
        data,
    }
}

fn run_shell(name: &'static str, cmd: &str, args: &[&str], cwd: &Path, blocking: bool) -> Stage {
    let r = Command::new(cmd).current_dir(cwd).args(args).output();
    match r {
        Ok(o) if o.status.success() => Stage {
            name,
            ok: true,
            blocking,
            detail: format!("{cmd} {} → ok", args.join(" ")),
        },
        Ok(o) => {
            let stderr: String = String::from_utf8_lossy(&o.stderr)
                .chars()
                .take(1500)
                .collect();
            Stage {
                name,
                ok: false,
                blocking,
                detail: format!(
                    "{cmd} {} exited {:?}\n{stderr}",
                    args.join(" "),
                    o.status.code()
                ),
            }
        }
        Err(e) => Stage {
            name,
            ok: false,
            blocking,
            detail: format!("{cmd} could not be invoked: {e}"),
        },
    }
}

fn detect_pm(cwd: &Path) -> String {
    if exists(cwd, "pnpm-lock.yaml") {
        "pnpm".into()
    } else if exists(cwd, "yarn.lock") {
        "yarn".into()
    } else {
        "npm".into()
    }
}

fn exists(cwd: &Path, name: &str) -> bool {
    let p: PathBuf = cwd.join(name);
    p.exists()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_marks_blocked_when_a_blocking_stage_failed() {
        let stages = vec![
            Stage {
                name: "a",
                ok: true,
                blocking: true,
                detail: "ok".into(),
            },
            Stage {
                name: "b",
                ok: false,
                blocking: true,
                detail: "boom".into(),
            },
        ];
        let r = finalize(
            Instant::now(),
            "feat",
            "main",
            &stages,
            vec![],
            serde_json::Value::Null,
        );
        assert!(r.blocked);
        assert!(!r.ok);
        assert!(r.summary.starts_with("BLOCKED at b"));
    }

    #[test]
    fn finalize_reports_shipped_when_all_stages_pass() {
        let stages = vec![
            Stage {
                name: "a",
                ok: true,
                blocking: true,
                detail: "ok".into(),
            },
            Stage {
                name: "b",
                ok: true,
                blocking: false,
                detail: "skipped".into(),
            },
        ];
        let r = finalize(
            Instant::now(),
            "feat",
            "main",
            &stages,
            vec![],
            serde_json::Value::Null,
        );
        assert!(r.ok);
        assert!(!r.blocked);
        assert!(r.summary.starts_with("shipped feat"));
    }
}
