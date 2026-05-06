/*
   File: crates/engulf/src/prompts.rs

   Purpose
   Centralized prompt templates for the security and docs phases. Kept
   in one place so they can be reviewed for prompt-injection safety.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

/// System prompt for the security audit.
pub const SECURITY_SYSTEM: &str = "You are a security auditor. Output JSON only.";

/// System prompt for the docs draft phase.
pub const DOCS_SYSTEM: &str =
    "You are a senior staff engineer writing READMEs. Output Markdown only.";
