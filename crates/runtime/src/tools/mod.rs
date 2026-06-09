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
*/

/// Directory listing tool.
pub mod list_dir;
/// File reading tool.
pub mod read_file;
/// Regex search tool.
pub mod search;
/// File writing tool.
pub mod write_file;

pub use list_dir::ListDir;
pub use read_file::ReadFile;
pub use search::Search;
pub use write_file::WriteFile;
