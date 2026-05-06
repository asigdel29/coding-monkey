/*
   File: crates/core/src/ids.rs

   Purpose
   Short, prefixed, sortable IDs for tasks/workers/sessions. Uses
   UUID v7 (time-ordered) trimmed to 12 hex chars after the prefix.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/core/src/utils/id.ts
*/

use uuid::Uuid;

/// Generate a short prefixed identifier. Output shape: `<prefix>_<20 hex>`.
///
/// Time-ordered (uuid v7) so IDs sort lexicographically by creation time —
/// useful for log scanning without a separate timestamp column. The 20-hex
/// suffix is 12 chars of millisecond timestamp + 8 chars of random, which
/// makes within-millisecond collisions vanishingly rare while keeping IDs
/// short.
///
/// # Examples
/// ```
/// use monkey_core::ids::generate_id;
/// let id = generate_id("task");
/// assert!(id.starts_with("task_"));
/// assert_eq!(id.len(), "task_".len() + 20);
/// ```
pub fn generate_id(prefix: &str) -> String {
    let uuid = Uuid::now_v7();
    let hex = uuid.simple().to_string();
    // uuid v7 layout (32 hex chars):
    //   [0..12]  unix_ts_ms       — preserved up front for sortability
    //   [12]     version nibble ('7')
    //   [13..16] rand_a (12 bits)
    //   [16]     variant nibble
    //   [17..32] rand_b (62 bits)
    // Pull 8 chars from rand_b to break ties within the same ms.
    format!("{}_{}{}", prefix, &hex[..12], &hex[20..28])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let a = generate_id("test");
        let b = generate_id("test");
        assert_ne!(a, b);
    }

    #[test]
    fn prefix_preserved() {
        assert!(generate_id("worker").starts_with("worker_"));
    }

    #[test]
    fn ids_are_time_ordered() {
        let mut ids: Vec<_> = (0..10).map(|_| generate_id("t")).collect();
        let original = ids.clone();
        ids.sort();
        assert_eq!(ids, original, "uuid v7 IDs should already be sorted");
    }
}
