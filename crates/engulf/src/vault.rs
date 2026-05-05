/*
   File: crates/engulf/src/vault.rs

   Purpose
   Write an Obsidian-shaped Markdown knowledge graph under
   `.monkey/vault/`. Each note has YAML front-matter and `[[wikilinks]]`
   to other notes.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use std::path::Path;

use crate::scanner::ScanResult;
use crate::security::SecurityAuditResult;
use crate::deployer::DeploymentRunbook;

/// Write the vault under `output_path`. Stub.
pub async fn write_vault(
    _output_path: &Path,
    _scan: &ScanResult,
    _audit: &SecurityAuditResult,
    _runbook: &DeploymentRunbook,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    Ok(Vec::new())
}
