/*
   File: crates/cli/src/commands/mod.rs

   Purpose
   One module per subcommand. Each exports `Args` (clap-derived) and
   `pub async fn run(args: Args) -> anyhow::Result<()>`. Dispatched from
   main.rs.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

pub mod chat;
pub mod compliance;
pub mod cso;
pub mod deck;
pub mod doctor;
pub mod engulf;
pub mod init;
pub mod investigate;
pub mod models;
pub mod orchestrate;
pub mod pentest;
pub mod review;
pub mod ship;
pub mod skill;
