/*
   File: crates/cli/src/commands/compliance.rs
   Purpose: SOC 2 status / verify / evidence.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; verify wired to audit chain
*/

use clap::{Args as ClapArgs, Subcommand};
use monkey_agents::verify_audit_log;
use std::path::PathBuf;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Action {
    /// Run automated checks for every control in CONTROL_MATRIX.json.
    Status {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Walk every audit-log hash chain in .monkey/sessions/.
    Verify {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Bundle audit logs + matrix + policies into an auditor-ready tar.gz.
    Evidence {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    match args.action {
        Action::Status { cwd } => {
            let _ = cwd; // TODO: load CONTROL_MATRIX.json and run checks.
            eprintln!("compliance status: stub (matrix loader pending)");
            Ok(())
        }
        Action::Verify { cwd } => {
            let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            let dir = cwd.join(".monkey").join("sessions");
            let mut bad = 0usize;
            if dir.is_dir() {
                for entry in std::fs::read_dir(&dir)? {
                    let p = entry?.path();
                    if !p.is_file() { continue; }
                    if p.extension().and_then(|e| e.to_str()) != Some("log") { continue; }
                    match verify_audit_log(&p) {
                        Ok(()) => eprintln!("✓ {}", p.display()),
                        Err(e) => { bad += 1; eprintln!("✗ {} — {e}", p.display()); }
                    }
                }
            }
            std::process::exit(if bad == 0 { 0 } else { 1 });
        }
        Action::Evidence { .. } => {
            eprintln!("compliance evidence: stub (tar.gz builder pending)");
            Ok(())
        }
    }
}
