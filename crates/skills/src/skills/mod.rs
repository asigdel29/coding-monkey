/*
   File: crates/skills/src/skills/mod.rs

   Purpose
   Re-export the four built-in skills. Each lives in its own module so
   it can grow without inflating the registry.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

mod cso;
mod investigate;
mod review;
mod ship;

pub use cso::Cso;
pub use investigate::Investigate;
pub use review::Review;
pub use ship::Ship;
