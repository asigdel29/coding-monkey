/*
   File: crates/engulf/src/deployer.rs

   Purpose
   Build a production-ready deployment runbook from a `ScanResult`.
   Detects the target platform (Vercel / Docker / Fly.io / Railway /
   generic) from CI configs + tech-stack signals, picks a step list
   tuned to that platform, optionally calls an LLM for stack-specific
   "pro tips", and renders the whole thing to markdown.

   Step lists are deliberately concrete: every command is the real
   command we'd want a junior dev to run, not a placeholder. Manual
   steps are flagged so the markdown can highlight them.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/engulf/src/deployer.ts
*/

use serde::{Deserialize, Serialize};

use crate::llm::{complete, PromptRequest, Provider as LlmProvider};
use crate::scanner::{CIType, ScanResult};
use crate::Provider;

/// Result of [`generate_deployment_runbook`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeploymentRunbook {
    /// Detected platform (`"Vercel"`, `"Fly.io"`, `"Docker"`, `"Railway"`, `"Unknown"`).
    pub platform: String,
    /// Pre-rendered markdown ready to write to `.monkey/context/DEPLOYMENT.md`.
    pub markdown: String,
    /// Ordered steps used to render the markdown.
    pub steps: Vec<DeployStep>,
    /// Env-var names that lack a `.env.example` entry (the deployer
    /// flags these so the user knows to add them before going live).
    pub env_vars_required: Vec<String>,
    /// Coarse time estimate.
    pub estimated_time: String,
}

/// One step in a runbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStep {
    /// Short title.
    pub title: String,
    /// Optional shell command. `None` for instructional / manual steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Free-text explanation.
    pub description: String,
    /// True if the step needs human input (login, dashboard config).
    pub is_manual: bool,
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Generate the runbook. Uses Anthropic by default for the LLM tips
/// step; pass [`RunbookOptions`] to override.
pub async fn generate_deployment_runbook(scan: &ScanResult) -> anyhow::Result<DeploymentRunbook> {
    generate_with(scan, RunbookOptions::default()).await
}

/// Knobs for [`generate_with`].
#[derive(Debug, Clone, Default)]
pub struct RunbookOptions {
    /// Provider for the LLM tips step.
    pub provider: Option<Provider>,
    /// API key override.
    pub api_key: Option<String>,
    /// Skip the LLM tips step entirely.
    pub skip_llm: bool,
    /// Model id override.
    pub model: Option<String>,
}

/// `generate_deployment_runbook` with explicit options.
pub async fn generate_with(
    scan: &ScanResult,
    opts: RunbookOptions,
) -> anyhow::Result<DeploymentRunbook> {
    let platform = detect_platform(scan);
    let steps = match platform.as_str() {
        "Vercel" => build_vercel_steps(scan),
        "Docker" => build_docker_steps(scan),
        "Fly.io" => build_fly_steps(scan),
        "Railway" => build_railway_steps(scan),
        _ => build_generic_steps(scan),
    };

    let tips = if opts.skip_llm {
        String::new()
    } else {
        match enhance_with_llm(scan, &platform, &opts).await {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!("engulf deploy tips skipped: {err}");
                String::new()
            }
        }
    };

    let markdown = build_markdown(scan, &platform, &steps, &tips);
    let env_vars_required: Vec<String> = scan
        .env_vars
        .iter()
        .filter(|e| !e.has_example)
        .map(|e| e.name.clone())
        .collect();
    let estimated_time = if steps.len() > 5 {
        "30-60 minutes"
    } else {
        "15-30 minutes"
    }
    .to_string();

    Ok(DeploymentRunbook {
        platform,
        markdown,
        steps,
        env_vars_required,
        estimated_time,
    })
}

// ─── Platform detection ─────────────────────────────────────────────────────

fn detect_platform(scan: &ScanResult) -> String {
    let has_ci = |t: CIType| scan.ci_configs.iter().any(|c| c.ci_type == t);
    let target = scan.tech_stack.deploy_target.as_deref();

    if has_ci(CIType::Vercel) || target == Some("Vercel") {
        return "Vercel".into();
    }
    if has_ci(CIType::Fly) || target == Some("Fly.io") {
        return "Fly.io".into();
    }
    if has_ci(CIType::Railway) || target == Some("Railway") {
        return "Railway".into();
    }
    if has_ci(CIType::Netlify) || target == Some("Netlify") {
        return "Netlify".into();
    }
    if has_ci(CIType::Docker) || target == Some("Docker") {
        return "Docker".into();
    }
    // Framework-based fallback.
    let primary = scan.tech_stack.primary.as_str();
    if primary == "Next.js" || primary == "SvelteKit" {
        return "Vercel".into();
    }
    "Unknown".into()
}

// ─── Step builders ──────────────────────────────────────────────────────────

fn install_command(pm: &str) -> String {
    match pm {
        "pnpm" => "pnpm install".into(),
        "yarn" => "yarn".into(),
        "bun" => "bun install".into(),
        other => format!("{other} install"),
    }
}

fn build_vercel_steps(scan: &ScanResult) -> Vec<DeployStep> {
    let pm = scan.tech_stack.package_manager.as_deref().unwrap_or("npm");
    let install = install_command(pm);
    let env_block = if scan.env_vars.is_empty() {
        "  (none detected)".to_string()
    } else {
        scan.env_vars
            .iter()
            .map(|e| format!("  - {}", e.name))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let env_command = scan
        .env_vars
        .first()
        .map(|e| format!("vercel env add {}", e.name));

    vec![
        DeployStep {
            title: "Install Vercel CLI".into(),
            command: Some("npm install -g vercel".into()),
            description: "Install the Vercel CLI globally".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Login to Vercel".into(),
            command: Some("vercel login".into()),
            description: "Authenticate with your Vercel account".into(),
            is_manual: true,
        },
        DeployStep {
            title: "Install Dependencies".into(),
            command: Some(install),
            description: "Install project dependencies".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Set Environment Variables".into(),
            command: env_command,
            description: format!(
                "Set the following env vars in Vercel dashboard or via CLI:\n{env_block}"
            ),
            is_manual: true,
        },
        DeployStep {
            title: "Deploy to Preview".into(),
            command: Some("vercel".into()),
            description: "Deploy to a preview URL for testing".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Deploy to Production".into(),
            command: Some("vercel --prod".into()),
            description: "Deploy to production URL".into(),
            is_manual: false,
        },
    ]
}

fn build_docker_steps(scan: &ScanResult) -> Vec<DeployStep> {
    let project = scan
        .root_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".into());
    vec![
        DeployStep {
            title: "Build Docker Image".into(),
            command: Some(format!("docker build -t {project}:latest .")),
            description: "Build the Docker image from the Dockerfile".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Run Locally".into(),
            command: Some(format!(
                "docker run -p 3000:3000 --env-file .env {project}:latest"
            )),
            description: "Test the image locally with your .env file".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Push to Registry".into(),
            command: Some(format!(
                "docker tag {project}:latest your-registry/{project}:latest\ndocker push your-registry/{project}:latest"
            )),
            description: "Push the image to your container registry (Docker Hub, ECR, GCR, etc.)"
                .into(),
            is_manual: true,
        },
        DeployStep {
            title: "Deploy".into(),
            command: None,
            description: "Deploy the image to your container orchestration platform (Kubernetes, ECS, Cloud Run, etc.)"
                .into(),
            is_manual: true,
        },
    ]
}

fn build_fly_steps(scan: &ScanResult) -> Vec<DeployStep> {
    let secrets_block = if scan.env_vars.is_empty() {
        "  (none detected)".to_string()
    } else {
        scan.env_vars
            .iter()
            .map(|e| format!("  fly secrets set {}=<value>", e.name))
            .collect::<Vec<_>>()
            .join("\n")
    };
    vec![
        DeployStep {
            title: "Install Fly CLI".into(),
            command: Some("brew install flyctl".into()),
            description: "Install the Fly.io CLI".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Login to Fly".into(),
            command: Some("fly auth login".into()),
            description: "Authenticate with your Fly.io account".into(),
            is_manual: true,
        },
        DeployStep {
            title: "Launch App".into(),
            command: Some("fly launch".into()),
            description: "Configure and launch your app on Fly.io".into(),
            is_manual: true,
        },
        DeployStep {
            title: "Set Secrets".into(),
            command: None,
            description: format!(
                "Set environment variables as Fly secrets:\n{secrets_block}"
            ),
            is_manual: true,
        },
        DeployStep {
            title: "Deploy".into(),
            command: Some("fly deploy".into()),
            description: "Build and deploy to Fly.io".into(),
            is_manual: false,
        },
    ]
}

fn build_railway_steps(scan: &ScanResult) -> Vec<DeployStep> {
    let vars_block = if scan.env_vars.is_empty() {
        "  (none detected)".to_string()
    } else {
        scan.env_vars
            .iter()
            .map(|e| format!("  railway variables set {}=<value>", e.name))
            .collect::<Vec<_>>()
            .join("\n")
    };
    vec![
        DeployStep {
            title: "Install Railway CLI".into(),
            command: Some("npm install -g @railway/cli".into()),
            description: "Install the Railway CLI".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Login to Railway".into(),
            command: Some("railway login".into()),
            description: "Authenticate with your Railway account".into(),
            is_manual: true,
        },
        DeployStep {
            title: "Initialize Project".into(),
            command: Some("railway init".into()),
            description: "Link this directory to a Railway project".into(),
            is_manual: true,
        },
        DeployStep {
            title: "Set Variables".into(),
            command: None,
            description: format!(
                "Set environment variables in the Railway dashboard or via CLI:\n{vars_block}"
            ),
            is_manual: true,
        },
        DeployStep {
            title: "Deploy".into(),
            command: Some("railway up".into()),
            description: "Deploy to Railway".into(),
            is_manual: false,
        },
    ]
}

fn build_generic_steps(scan: &ScanResult) -> Vec<DeployStep> {
    let pm = scan.tech_stack.package_manager.as_deref().unwrap_or("npm");
    let env_block = if scan.env_vars.is_empty() {
        "  (none detected)".to_string()
    } else {
        scan.env_vars
            .iter()
            .map(|e| {
                if let Some(desc) = &e.description {
                    format!("  - {}: {}", e.name, desc)
                } else {
                    format!("  - {}", e.name)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let build_cmd = match pm {
        "cargo" => "cargo build --release".to_string(),
        "go" => "go build ./...".to_string(),
        _ => format!("{pm} run build"),
    };
    vec![
        DeployStep {
            title: "Install Dependencies".into(),
            command: Some(install_command(pm)),
            description: "Install project dependencies".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Configure Environment".into(),
            command: Some("cp .env.example .env".into()),
            description: format!(
                "Copy .env.example to .env and fill in real values:\n{env_block}"
            ),
            is_manual: true,
        },
        DeployStep {
            title: "Build".into(),
            command: Some(build_cmd),
            description: "Build the production bundle".into(),
            is_manual: false,
        },
        DeployStep {
            title: "Start".into(),
            command: Some(format!("{pm} start")),
            description: "Start the production server".into(),
            is_manual: false,
        },
    ]
}

// ─── LLM tips ───────────────────────────────────────────────────────────────

async fn enhance_with_llm(
    scan: &ScanResult,
    platform: &str,
    opts: &RunbookOptions,
) -> anyhow::Result<String> {
    let provider = opts.provider.unwrap_or(Provider::Anthropic);
    let llm_provider = match provider {
        Provider::Anthropic => LlmProvider::Anthropic,
        Provider::Openai => LlmProvider::Openai,
    };
    let model = opts.model.clone().unwrap_or_else(|| match provider {
        Provider::Anthropic => "claude-haiku-4-5".into(),
        Provider::Openai => "gpt-5-mini".into(),
    });
    let user = format!(
        "For a {primary} project deploying to {platform}, write 2-3 sentences of deployment tips specific to this stack. Focus on common gotchas, environment variable handling, and production best practices. Be concise and practical.",
        primary = scan.tech_stack.primary
    );
    complete(PromptRequest {
        provider: llm_provider,
        model,
        system: None,
        user,
        max_tokens: 800,
        api_key: opts.api_key.clone(),
    })
    .await
}

// ─── Markdown ───────────────────────────────────────────────────────────────

fn build_markdown(
    scan: &ScanResult,
    platform: &str,
    steps: &[DeployStep],
    tips: &str,
) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d");
    let mut steps_md = String::new();
    for (i, s) in steps.iter().enumerate() {
        if i > 0 {
            steps_md.push_str("\n\n---\n\n");
        }
        steps_md.push_str(&format!(
            "### Step {}: {}\n\n{}",
            i + 1,
            s.title,
            s.description
        ));
        if let Some(cmd) = &s.command {
            steps_md.push_str(&format!("\n\n```bash\n{cmd}\n```"));
        }
        if s.is_manual {
            steps_md.push_str("\n\n> **Manual step** — requires your input or credentials");
        }
    }

    let env_table = if scan.env_vars.is_empty() {
        "_No environment variables detected._".to_string()
    } else {
        let mut t = String::from("| Variable | Has Example | Found In Code |\n|----------|------------|---------------|\n");
        for e in &scan.env_vars {
            t.push_str(&format!(
                "| `{}` | {} | {} |\n",
                e.name,
                if e.has_example { "yes" } else { "no" },
                if e.found_in_code { "yes" } else { "no" },
            ));
        }
        t
    };

    let runtime_line = if scan.tech_stack.language == "TypeScript"
        || scan.tech_stack.language == "JavaScript"
    {
        "- Node.js ≥ 20"
    } else {
        ""
    };
    let pm_line = match &scan.tech_stack.package_manager {
        Some(pm) => format!("- Package manager: {pm}"),
        None => String::new(),
    };
    let platform_line = if platform == "Unknown" {
        String::new()
    } else {
        format!("- {platform} account and CLI")
    };
    let tips_section = if tips.trim().is_empty() {
        String::new()
    } else {
        format!("\n## Pro Tips\n\n{tips}\n\n---\n")
    };

    format!(
        "# Deployment Runbook

**Project:** `{root}`
**Platform:** {platform}
**Stack:** {stack}
**Date:** {date}

---

## Requirements

{runtime_line}
{pm_line}
{platform_line}

---

## Environment Variables

{env_table}

---

## Deployment Steps

{steps_md}

---
{tips_section}
*Generated by monkey engulf on {date}*
",
        root = scan.root_path.display(),
        stack = scan.tech_stack.primary,
        date = now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{
        APIRoute, CIConfig, CIType, EnvVarInfo, ScanResult, TechStackInfo,
    };

    fn fake_scan(deploy_target: Option<&str>, ci: Vec<CIType>, primary: &str) -> ScanResult {
        ScanResult {
            root_path: std::path::PathBuf::from("/tmp/proj"),
            tech_stack: TechStackInfo {
                primary: primary.into(),
                framework: None,
                language: "TypeScript".into(),
                package_manager: Some("pnpm".into()),
                test_framework: None,
                deploy_target: deploy_target.map(|s| s.to_string()),
            },
            ci_configs: ci.into_iter().map(|t| CIConfig {
                ci_type: t,
                file_path: format!("{:?}", t),
                deploy_target: None,
                build_command: None,
            }).collect(),
            env_vars: vec![EnvVarInfo {
                name: "API_URL".into(),
                has_example: true,
                found_in_code: true,
                description: None,
            }],
            api_routes: Vec::<APIRoute>::new(),
            ..Default::default()
        }
    }

    #[test]
    fn detects_vercel_from_ci() {
        let scan = fake_scan(None, vec![CIType::Vercel], "Next.js");
        assert_eq!(detect_platform(&scan), "Vercel");
    }

    #[test]
    fn detects_vercel_from_framework_fallback() {
        let scan = fake_scan(None, vec![], "Next.js");
        assert_eq!(detect_platform(&scan), "Vercel");
    }

    #[test]
    fn falls_back_to_unknown_for_plain_node_app() {
        let scan = fake_scan(None, vec![], "TypeScript");
        assert_eq!(detect_platform(&scan), "Unknown");
    }

    #[tokio::test]
    async fn generic_runbook_lists_install_build_start() {
        let scan = fake_scan(None, vec![], "TypeScript");
        let r = generate_with(&scan, RunbookOptions { skip_llm: true, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(r.platform, "Unknown");
        let titles: Vec<_> = r.steps.iter().map(|s| s.title.as_str()).collect();
        assert!(titles.contains(&"Install Dependencies"));
        assert!(titles.contains(&"Build"));
        assert!(titles.contains(&"Start"));
        assert!(r.markdown.contains("Deployment Runbook"));
    }

    #[tokio::test]
    async fn vercel_runbook_more_steps_so_longer_estimate() {
        let scan = fake_scan(None, vec![CIType::Vercel], "Next.js");
        let r = generate_with(&scan, RunbookOptions { skip_llm: true, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(r.platform, "Vercel");
        assert_eq!(r.estimated_time, "30-60 minutes");
        assert!(r.steps.iter().any(|s| s.title == "Deploy to Production"));
    }

    #[tokio::test]
    async fn flags_env_vars_missing_example() {
        let mut scan = fake_scan(None, vec![], "TypeScript");
        scan.env_vars.push(EnvVarInfo {
            name: "SECRET_TOKEN".into(),
            has_example: false,
            found_in_code: true,
            description: None,
        });
        let r = generate_with(&scan, RunbookOptions { skip_llm: true, ..Default::default() })
            .await
            .unwrap();
        assert!(r.env_vars_required.contains(&"SECRET_TOKEN".to_string()));
        assert!(!r.env_vars_required.contains(&"API_URL".to_string()));
    }
}
