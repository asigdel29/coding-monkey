/*
   File: crates/cli/src/commands/models.rs
   Purpose: list registered AI models with cost tiers.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use monkey_core::ModelRegistry;

pub async fn run() -> anyhow::Result<()> {
    let r = ModelRegistry::with_builtin();
    println!("Available Models");
    for m in r.list_all() {
        let tier = match m.tier {
            monkey_core::ModelTier::Fast => "fast",
            monkey_core::ModelTier::Balanced => "balanced",
            monkey_core::ModelTier::Powerful => "powerful",
        };
        println!(
            "  {:30} {:9}  in: ${:.4}/1k  out: ${:.4}/1k",
            m.display_name, tier, m.input_cost_per_1k, m.output_cost_per_1k,
        );
    }
    Ok(())
}
