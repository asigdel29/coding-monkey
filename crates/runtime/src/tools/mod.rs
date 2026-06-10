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
*/

/// Directory listing tool.
pub mod list_dir;
/// File reading tool.
pub mod read_file;

pub use list_dir::ListDir;
pub use read_file::ReadFile;
