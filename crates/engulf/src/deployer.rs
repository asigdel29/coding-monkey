/*
   File: crates/engulf/src/deployer.rs

   Purpose
   Build a production-ready deployment runbook from a scan + the
   detected platform target (Vercel, Fly.io, Railway, generic).

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use serde::{Deserialize, Serialize};

use crate::scanner::ScanResult;

/// One step in a runbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStep {
    /// Short title for the step.
    pub title: String,
    /// Whether the step is manual (vs. automated by CI).
    pub is_manual: bool,
    /// Markdown body.
    pub body: String,
}

/// Top-level runbook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeploymentRunbook {
    /// Detected platform.
    pub platform: String,
    /// Ordered steps.
    pub steps: Vec<DeployStep>,
    /// Required env-vars not yet present in `.env.example`.
    pub env_vars_required: Vec<String>,
}

/// Build a runbook. Stub.
pub async fn build(_scan: &ScanResult) -> anyhow::Result<DeploymentRunbook> {
    Ok(DeploymentRunbook::default())
}
