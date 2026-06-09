/*
   File: crates/runtime/src/tools/mod.rs

   Purpose
   The built-in tools a native agent can call. Each submodule defines one
   tool implementing the `Tool` trait; this module re-exports them and will
   grow a registry-builder as the set fills out (write_file, search,
   run_command, finish in later changes).

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — read_file, list_dir
   2026-06-09   Anubhav Sigdel  add write_file, search
   2026-06-09   Anubhav Sigdel  add run_command (allowlisted)
   2026-06-09   Anubhav Sigdel  add finish + default_tools registry
*/

use std::sync::Arc;

use crate::tool::ToolRegistry;

/// Terminal "task complete" tool.
pub mod finish;
/// Directory listing tool.
pub mod list_dir;
/// File reading tool.
pub mod read_file;
/// Allowlisted external command tool.
pub mod run_command;
/// Regex search tool.
pub mod search;
/// File writing tool.
pub mod write_file;

pub use finish::Finish;
pub use list_dir::ListDir;
pub use read_file::ReadFile;
pub use run_command::RunCommand;
pub use search::Search;
pub use write_file::WriteFile;

/// A registry of the built-in tools every native agent gets by default:
/// read_file, list_dir, write_file, search, run_command (default
/// allowlist), and finish.
pub fn default_tools() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Arc::new(ReadFile));
    r.register(Arc::new(ListDir));
    r.register(Arc::new(WriteFile));
    r.register(Arc::new(Search));
    r.register(Arc::new(RunCommand::with_defaults()));
    r.register(Arc::new(Finish));
    r
}
