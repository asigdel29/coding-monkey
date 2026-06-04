/*
   File: crates/agents/src/lib.rs

   Purpose
   The "spawn an agent" primitive. Five responsibilities, all preserved
   from the TS port:

     1. assemble_context — read every `.md` under `.monkey/context/`
        + active tentacle's CONTEXT.md + todo.md, cap at 32 KB, surface
        truncation.
     2. redact          — scrub secrets from agent stdout before logging.
     3. AuditLog        — append-only, hash-chained .monkey/sessions/audit-*.log.
     4. doctor          — check the codex CLI + API keys are available.
     5. spawn_agent     — PTY-spawn the chosen CLI with the assembled prompt.

   Invariants
   - Missing context files are skipped silently.
   - Files larger than `MAX_FILE_BYTES` are trimmed and surfaced via
     `AssembledContext.truncated_files`.
   - Every audit-log line embeds `sha256(prev_line)`; verification walks
     the chain end-to-end.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port from packages/agents/src/
*/

#![deny(missing_debug_implementations)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-agents` — spawn agent CLIs with assembled context, redaction, and
//! tamper-evident audit logging.

/// Append-only, hash-chained audit log for agent lifecycle events.
pub mod audit;
/// Context assembly — gather `.monkey/context/` and tentacle docs into a prompt.
pub mod context;
/// CLI doctor — verify the `codex` CLI + API keys are present and pickable.
pub mod doctor;
/// Stdout redactor — scrub API keys and other secrets before logging.
pub mod redact;
/// PTY-spawn the chosen agent CLI with the assembled prompt.
pub mod spawn;
/// Public types for `monkey-agents` (`AgentKind`, `SpawnOpts`, …).
pub mod types;

pub use audit::{verify_audit_log, AuditEntry, AuditEventType, AuditLogger};
pub use context::{assemble_context, AssembledContext};
pub use doctor::{doctor, pick_auto, DoctorReport};
pub use redact::{redact, redact_object};
pub use spawn::{spawn_agent, AgentTerminal, SpawnResult};
pub use types::{AgentKind, SpawnOpts};
