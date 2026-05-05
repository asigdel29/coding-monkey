/*
   File: crates/engulf/src/security.rs

   Purpose
   LLM-assisted security audit. Consumes a `ScanResult` and emits a
   structured findings list with severities.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use serde::{Deserialize, Serialize};

use crate::scanner::ScanResult;

/// Finding severity. Matches `--fail-on` thresholds.
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

/// One finding in the audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Severity bucket.
    pub severity: Severity,
    /// Category (`"injection"`, `"crypto"`, …).
    pub category: String,
    /// Short title.
    pub title: String,
    /// Source file path, if known.
    pub file_path: Option<String>,
    /// Free-text detail.
    pub detail: String,
}

/// Top-level audit result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityAuditResult {
    /// One-line summary.
    pub summary: String,
    /// All findings.
    pub findings: Vec<Finding>,
    /// Count of `Severity::Critical` findings.
    pub critical_count: usize,
    /// Count of `Severity::High` findings.
    pub high_count: usize,
}

/// Run the audit. Stub.
pub async fn audit(_scan: &ScanResult) -> anyhow::Result<SecurityAuditResult> {
    Ok(SecurityAuditResult::default())
}
