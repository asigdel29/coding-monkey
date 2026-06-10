/*
   File: crates/runtime/src/lib.rs

   Purpose
   `monkey-runtime` — the native, in-process agent engine. Where
   `monkey-agents` spawns a heavyweight external CLI in a PTY, this crate
   runs an agent as a lightweight async task: assemble context, call the
   LLM, execute tool calls, iterate. Network-bound and ~12 MiB each, so a
   small host (a Raspberry Pi) can run 100+ at once.

   This change lands the public contract — tools, transcript/state, and the
   event stream. The LLM client, concrete tools, agent loop, provider
   limiter, and scheduler arrive in subsequent changes.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial skeleton — tool/state/event types
   2026-06-09   Anubhav Sigdel  add llm module (tool-calling chat client)
   2026-06-09   Anubhav Sigdel  add fs_guard + read_file/list_dir tools
*/

#![deny(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(missing_docs)]

//! Native in-process agent runtime for the coding-monkey workspace.
//!
//! - [`tool`] — the `Tool` trait, execution context, result, and registry
//! - [`state`] — transcript messages, run config, and outcome
//! - [`event`] — the progress event stream an agent emits
//! - [`llm`] — tool-calling chat client over a shared HTTP pool
//! - [`fs_guard`] — working-directory path jail for file tools
//! - [`tools`] — built-in agent tools

/// Agent progress event stream.
pub mod event;
/// Working-directory path jail for filesystem tools.
pub mod fs_guard;
/// Tool-calling LLM chat client.
pub mod llm;
/// Conversation state: transcript, config, outcome.
pub mod state;
/// Tool interface, execution context, and registry.
pub mod tool;
/// Built-in agent tools.
pub mod tools;

pub use event::AgentEvent;
pub use fs_guard::{FsError, FsGuard};
pub use llm::{ChatResult, LlmError, NativeLlm};
pub use state::{AgentConfig, AgentOutcome, AgentState, Message, Role, ToolCall};
pub use tool::{Tool, ToolCtx, ToolRef, ToolRegistry, ToolResult};
