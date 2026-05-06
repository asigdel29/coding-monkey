/*
   File: crates/engulf/src/security.rs

   Purpose
   Static-plus-LLM security audit. Two layers stitched together:

     1. Static layer — every SecurityHint produced by the scanner is
        promoted to a Finding using a small CWE-tagged category table.
        Deterministic, runs without any API key.

     2. LLM layer — a structured prompt gives Claude (or GPT) a
        compact view of the project profile (stack, deps count, env
        vars, CI, scanner hints) and asks for a JSON array of
        additional findings. Skipped silently when no key is set so
        engulf still produces a useful report offline.

   The two lists are merged, counted by severity, and rendered to a
   single markdown report.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/engulf/src/security.ts;
                                 wires the new `llm` module
*/

use serde::{Deserialize, Serialize};

use crate::llm::{complete, strip_json_fences, PromptRequest, Provider as LlmProvider};
use crate::scanner::{HintSeverity, ScanResult, SecurityHint};
use crate::Provider;

/// Top-level audit result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityAuditResult {
    /// One-line summary suitable for log lines.
    pub summary: String,
    /// Count of findings with [`Severity::Critical`].
    pub critical_count: usize,
    /// Count of findings with [`Severity::High`].
    pub high_count: usize,
    /// Count of findings with [`Severity::Medium`].
    pub medium_count: usize,
    /// Count of findings with [`Severity::Low`].
    pub low_count: usize,
    /// All findings (static + LLM), in static-then-LLM order.
    pub findings: Vec<SecurityFinding>,
    /// Pre-rendered markdown report.
    pub markdown: String,
}

/// One finding from either the static or LLM layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityFinding {
    /// Severity bucket.
    pub severity: Severity,
    /// Category — `"Secrets Management"`, `"Authentication"`, …
    pub category: String,
    /// Short title.
    pub title: String,
    /// Free-text detail.
    pub description: String,
    /// Source file path, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// 1-indexed line number, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_number: Option<usize>,
    /// Recommended remediation.
    pub recommendation: String,
    /// CWE identifier (`CWE-798`), if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwe_id: Option<String>,
}

/// Severity ordering. Same shape as the rest of the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational.
    Info,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Critical.
    Critical,
}

impl From<HintSeverity> for Severity {
    fn from(h: HintSeverity) -> Self {
        match h {
            HintSeverity::Info => Severity::Info,
            HintSeverity::Low => Severity::Low,
            HintSeverity::Medium => Severity::Medium,
            HintSeverity::High => Severity::High,
            HintSeverity::Critical => Severity::Critical,
        }
    }
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
    fn marker(self) -> &'static str {
        match self {
            Severity::Info => "[info]",
            Severity::Low => "[low]",
            Severity::Medium => "[medium]",
            Severity::High => "[high]",
            Severity::Critical => "[critical]",
        }
    }
}

// ─── Category metadata for static hints ─────────────────────────────────────

struct CategoryMeta {
    category: &'static str,
    cwe_id: Option<&'static str>,
    recommendation: &'static str,
}

fn category_for(kind: &str) -> Option<CategoryMeta> {
    match kind {
        "hardcoded-secret" => Some(CategoryMeta {
            category: "Secrets Management",
            cwe_id: Some("CWE-798"),
            recommendation: "Move secrets to environment variables and add them to .gitignore",
        }),
        "committed-secrets" => Some(CategoryMeta {
            category: "Secrets Management",
            cwe_id: Some("CWE-312"),
            recommendation: "Remove .env from git history using git-filter-repo, add to .gitignore",
        }),
        "openai-key" => Some(CategoryMeta {
            category: "API Key Exposure",
            cwe_id: Some("CWE-798"),
            recommendation: "Rotate the key immediately and store in environment variables only",
        }),
        "jwt-token" => Some(CategoryMeta {
            category: "Token Exposure",
            cwe_id: Some("CWE-522"),
            recommendation: "Never hardcode tokens; use environment variables or secrets managers",
        }),
        "missing-gitignore" => Some(CategoryMeta {
            category: "Configuration",
            cwe_id: None,
            recommendation:
                "Create a .gitignore that excludes .env, node_modules, dist, build, etc.",
        }),
        _ => None,
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Run the full audit. Returns an offline-only result if no LLM key is
/// available — never errors on a key-less run.
pub async fn audit(scan: &ScanResult) -> anyhow::Result<SecurityAuditResult> {
    audit_with(scan, AuditOptions::default()).await
}

/// Knobs for [`audit_with`].
#[derive(Debug, Clone, Default)]
pub struct AuditOptions {
    /// Provider to use for the LLM layer. Defaults to Anthropic.
    pub provider: Option<Provider>,
    /// API key override; defaults to env var.
    pub api_key: Option<String>,
    /// Skip the LLM layer entirely (faster, cheaper, deterministic).
    pub skip_llm: bool,
    /// Model id override.
    pub model: Option<String>,
}

/// `audit` with explicit options.
pub async fn audit_with(
    scan: &ScanResult,
    opts: AuditOptions,
) -> anyhow::Result<SecurityAuditResult> {
    let static_findings = scan
        .security_hints
        .iter()
        .map(static_hint_to_finding)
        .collect::<Vec<_>>();

    let llm_findings = if opts.skip_llm {
        Vec::new()
    } else {
        match run_llm_layer(scan, &opts).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!("engulf LLM security analysis skipped: {err}");
                Vec::new()
            }
        }
    };

    let mut all = Vec::with_capacity(static_findings.len() + llm_findings.len());
    all.extend(static_findings);
    all.extend(llm_findings);

    let critical_count = count(&all, Severity::Critical);
    let high_count = count(&all, Severity::High);
    let medium_count = count(&all, Severity::Medium);
    let low_count = count(&all, Severity::Low);

    let summary = format!(
        "{} security findings: {} critical, {} high, {} medium, {} low",
        all.len(),
        critical_count,
        high_count,
        medium_count,
        low_count,
    );
    let markdown = render_markdown(scan, &all, &summary);

    Ok(SecurityAuditResult {
        summary,
        critical_count,
        high_count,
        medium_count,
        low_count,
        findings: all,
        markdown,
    })
}

// ─── Static layer ───────────────────────────────────────────────────────────

fn static_hint_to_finding(hint: &SecurityHint) -> SecurityFinding {
    let meta = category_for(&hint.kind);
    let category = meta
        .as_ref()
        .map(|m| m.category.to_string())
        .unwrap_or_else(|| "General".into());
    let recommendation = meta
        .as_ref()
        .map(|m| m.recommendation.to_string())
        .unwrap_or_else(|| "Review and address the security concern".into());
    let cwe_id = meta.and_then(|m| m.cwe_id.map(|s| s.to_string()));
    SecurityFinding {
        severity: hint.severity.into(),
        category,
        title: title_case_kind(&hint.kind),
        description: hint.description.clone(),
        file_path: hint.file_path.clone(),
        line_number: hint.line_number,
        recommendation,
        cwe_id,
    }
}

fn title_case_kind(kind: &str) -> String {
    let mut out = String::with_capacity(kind.len());
    let mut next_upper = true;
    for c in kind.chars() {
        if c == '-' || c == '_' {
            out.push(' ');
            next_upper = true;
        } else if next_upper {
            out.extend(c.to_uppercase());
            next_upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

// ─── LLM layer ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RawLlmFinding {
    severity: String,
    category: String,
    title: String,
    description: String,
    #[serde(default)]
    recommendation: String,
    #[serde(default, rename = "cweId")]
    cwe_id: Option<String>,
    #[serde(default, rename = "filePath")]
    file_path: Option<String>,
    #[serde(default, rename = "lineNumber")]
    line_number: Option<usize>,
}

async fn run_llm_layer(
    scan: &ScanResult,
    opts: &AuditOptions,
) -> anyhow::Result<Vec<SecurityFinding>> {
    let provider = opts.provider.unwrap_or(Provider::Anthropic);
    let llm_provider = match provider {
        Provider::Anthropic => LlmProvider::Anthropic,
        Provider::Openai => LlmProvider::Openai,
    };
    let model = opts.model.clone().unwrap_or_else(|| match provider {
        Provider::Anthropic => "claude-haiku-4-5".into(),
        Provider::Openai => "gpt-5-mini".into(),
    });
    let prompt = build_prompt(scan);
    let req = PromptRequest {
        provider: llm_provider,
        model,
        system: Some("You are a senior security engineer performing a code security audit. Output JSON only.".into()),
        user: prompt,
        max_tokens: 2000,
        api_key: opts.api_key.clone(),
    };
    let raw = complete(req).await?;
    let stripped = strip_json_fences(&raw);
    let parsed: Vec<RawLlmFinding> = serde_json::from_str(stripped)
        .map_err(|e| anyhow::anyhow!("decode LLM JSON: {e}; got: {stripped}"))?;
    Ok(parsed.into_iter().map(raw_to_finding).collect())
}

fn raw_to_finding(r: RawLlmFinding) -> SecurityFinding {
    let severity = match r.severity.to_ascii_lowercase().as_str() {
        "critical" => Severity::Critical,
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" => Severity::Low,
        _ => Severity::Info,
    };
    SecurityFinding {
        severity,
        category: r.category,
        title: r.title,
        description: r.description,
        file_path: r.file_path,
        line_number: r.line_number,
        recommendation: if r.recommendation.is_empty() {
            "Review and address the security concern".into()
        } else {
            r.recommendation
        },
        cwe_id: r.cwe_id,
    }
}

fn build_prompt(scan: &ScanResult) -> String {
    let known_hints = if scan.security_hints.is_empty() {
        "None detected".to_string()
    } else {
        scan.security_hints
            .iter()
            .map(|h| {
                format!(
                    "- [{}] {}",
                    label_hint(h.severity).to_uppercase(),
                    h.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let env_vars = if scan.env_vars.is_empty() {
        "None found".to_string()
    } else {
        scan.env_vars
            .iter()
            .map(|e| format!("- {} (hasExample: {})", e.name, e.has_example))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let ci = if scan.ci_configs.is_empty() {
        "None".to_string()
    } else {
        scan.ci_configs
            .iter()
            .map(|c| format!("{:?}", c.ci_type))
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "Project: {root}
Tech Stack: {primary} / {framework}
Language: {language}
Dependencies count: {deps}
API Routes found: {routes}

Known security hints from static analysis:
{known_hints}

Environment variables in use:
{env_vars}

CI/CD detected: {ci}

Based on the tech stack and project profile, identify the TOP security risks. Focus on:
1. Missing security headers (if web app)
2. Authentication/authorization gaps
3. Dependency vulnerabilities (based on framework version)
4. CSRF, XSS, SQLi risks for the detected stack
5. Deployment security concerns
6. Secrets management

Respond with a JSON array of findings. Each finding must have:
{{
  \"severity\": \"critical|high|medium|low|info\",
  \"category\": \"string\",
  \"title\": \"string\",
  \"description\": \"string\",
  \"recommendation\": \"string\",
  \"cweId\": \"CWE-XXX or null\"
}}

Return ONLY the JSON array, no markdown fences.",
        root = scan.root_path.display(),
        primary = scan.tech_stack.primary,
        framework = scan
            .tech_stack
            .framework
            .as_deref()
            .unwrap_or("unknown framework"),
        language = scan.tech_stack.language,
        deps = scan.dependencies.len(),
        routes = scan.api_routes.len(),
        known_hints = known_hints,
        env_vars = env_vars,
        ci = ci,
    )
}

fn label_hint(s: HintSeverity) -> &'static str {
    match s {
        HintSeverity::Info => "info",
        HintSeverity::Low => "low",
        HintSeverity::Medium => "medium",
        HintSeverity::High => "high",
        HintSeverity::Critical => "critical",
    }
}

// ─── Markdown rendering ─────────────────────────────────────────────────────

fn count(findings: &[SecurityFinding], sev: Severity) -> usize {
    findings.iter().filter(|f| f.severity == sev).count()
}

fn render_markdown(scan: &ScanResult, findings: &[SecurityFinding], summary: &str) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d");

    let mut sections = Vec::new();
    for sev in [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Info,
    ] {
        let group: Vec<&SecurityFinding> = findings.iter().filter(|f| f.severity == sev).collect();
        if group.is_empty() {
            continue;
        }
        let mut body = String::new();
        body.push_str(&format!(
            "## {} {} ({})\n\n",
            sev.marker(),
            capitalize(sev.label()),
            group.len()
        ));
        for (i, f) in group.iter().enumerate() {
            if i > 0 {
                body.push_str("\n---\n\n");
            }
            body.push_str(&format!("### {}\n", f.title));
            if let Some(cwe) = &f.cwe_id {
                body.push_str(&format!("**CWE:** {cwe}  \n"));
            }
            body.push_str(&format!("**Category:** {}\n\n", f.category));
            body.push_str(&format!("{}\n", f.description));
            if let Some(path) = &f.file_path {
                let line = match f.line_number {
                    Some(n) => format!(":{n}"),
                    None => String::new(),
                };
                body.push_str(&format!("**Location:** `{path}{line}`\n"));
            }
            body.push_str(&format!("\n> **Recommendation:** {}\n", f.recommendation));
        }
        sections.push(body);
    }
    let combined = if sections.is_empty() {
        "_No security findings detected._".to_string()
    } else {
        sections.join("\n\n---\n\n")
    };

    format!(
        "# Security Audit Report

**Project:** `{root}`
**Stack:** {stack}
**Date:** {date}
**Summary:** {summary}

---

## Overview

| Severity | Count |
|----------|-------|
| Critical | {critical} |
| High     | {high} |
| Medium   | {medium} |
| Low      | {low} |
| Info     | {info} |

---

{combined}

---

*Generated by monkey engulf on {date}*
",
        root = scan.root_path.display(),
        stack = scan.tech_stack.primary,
        date = now,
        critical = count(findings, Severity::Critical),
        high = count(findings, Severity::High),
        medium = count(findings, Severity::Medium),
        low = count(findings, Severity::Low),
        info = count(findings, Severity::Info),
    )
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::HintSeverity;

    fn fake_scan_with_hints(hints: Vec<SecurityHint>) -> ScanResult {
        ScanResult {
            root_path: std::path::PathBuf::from("/tmp/fake"),
            tech_stack: crate::scanner::TechStackInfo {
                primary: "Rust".into(),
                language: "Rust".into(),
                ..Default::default()
            },
            security_hints: hints,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn audit_runs_offline_with_no_key() {
        let scan = fake_scan_with_hints(vec![SecurityHint {
            severity: HintSeverity::Critical,
            kind: "committed-secrets".into(),
            description: ".env file present".into(),
            file_path: Some(".env".into()),
            line_number: None,
        }]);
        // Force skip to make this deterministic regardless of env state.
        let r = audit_with(
            &scan,
            AuditOptions {
                skip_llm: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(r.critical_count, 1);
        assert!(r.findings[0].cwe_id.as_deref() == Some("CWE-312"));
        assert!(r.markdown.contains("Security Audit Report"));
        assert!(r.markdown.contains("Committed Secrets"));
    }

    #[test]
    fn title_case_handles_kebab() {
        assert_eq!(title_case_kind("hardcoded-secret"), "Hardcoded Secret");
        assert_eq!(title_case_kind("missing-gitignore"), "Missing Gitignore");
    }

    #[test]
    fn severity_ordering_is_total() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Medium > Severity::Low);
        assert!(Severity::Low > Severity::Info);
    }

    #[test]
    fn unknown_hint_kind_falls_back_to_general_category() {
        let f = static_hint_to_finding(&SecurityHint {
            severity: HintSeverity::Medium,
            kind: "novel-kind".into(),
            description: "something odd".into(),
            file_path: None,
            line_number: None,
        });
        assert_eq!(f.category, "General");
        assert_eq!(f.recommendation, "Review and address the security concern");
        assert!(f.cwe_id.is_none());
    }
}
