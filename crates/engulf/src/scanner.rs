/*
   File: crates/engulf/src/scanner.rs

   Purpose
   Filesystem scan: stack, deps, env vars, git info, CI configs, API
   routes, schemas, and security hints. Emits a `ScanResult` consumed
   by every subsequent engulf phase (security audit, deployer, vault).

   Implementation notes
   - Walk uses a hand-rolled recursive function rather than walkdir so
     SKIP_DIRS pruning happens before descending — important on large
     trees where node_modules/target/.next would otherwise dominate.
   - Files are read on demand. `collect_files` only stat()s entries.
   - Code-file scans (env vars, secrets, routes) are size-capped so
     a vendored library file can't make the scan unbounded.
   - All errors below the public surface are swallowed: a partial scan
     beats a hard failure when the user just wants engulf to "do its
     best" against a noisy repo.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/engulf/src/scanner.ts
*/

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

// ─── Public types ───────────────────────────────────────────────────────────

/// What a single scan run produces. Every downstream engulf phase reads
/// this; treat it as a stable contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanResult {
    /// Absolute path to the scanned root.
    pub root_path: PathBuf,
    /// Detected primary stack + framework + tools.
    pub tech_stack: TechStackInfo,
    /// Every file kept by the walker (post-SKIP_DIRS, post-dotfile rules).
    pub files: Vec<FileInfo>,
    /// Project dependencies parsed from manifests.
    pub dependencies: Vec<DependencyInfo>,
    /// Env vars discovered in `.env.example` and source code.
    pub env_vars: Vec<EnvVarInfo>,
    /// Information from `git`. Empty if git is unavailable or the dir
    /// is not a repo.
    pub git_info: GitInfo,
    /// Detected CI / deploy configs.
    pub ci_configs: Vec<CIConfig>,
    /// Detected HTTP routes.
    pub api_routes: Vec<APIRoute>,
    /// Files matching schema/migration/model name patterns.
    pub schemas: Vec<SchemaInfo>,
    /// Security signals (committed secrets, missing files, etc.).
    pub security_hints: Vec<SecurityHint>,
}

/// Stack-detection result. Optional fields are `None` when no signal
/// is strong enough to commit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TechStackInfo {
    /// One-word descriptor used in headers ("Next.js", "Rust", …).
    pub primary: String,
    /// Framework, if detected (often the same as `primary`).
    pub framework: Option<String>,
    /// Source language (decided by file-extension counting).
    pub language: String,
    /// Package manager (`pnpm`, `cargo`, …).
    pub package_manager: Option<String>,
    /// Test framework (`Vitest`, `pytest`, …).
    pub test_framework: Option<String>,
    /// Deploy target (`Vercel`, `Fly.io`, `Docker`, …).
    pub deploy_target: Option<String>,
}

/// One file from the walker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Path relative to the scan root, with `/` separators.
    pub relative_path: String,
    /// Size in bytes (0 if stat failed).
    pub size_bytes: u64,
    /// Lowercased extension including the dot, or empty.
    pub extension: String,
    /// Whether the basename matches a recognized project config file
    /// (e.g. `package.json`, `Cargo.toml`, `Dockerfile`, `.env`).
    pub is_config: bool,
}

/// Source of a parsed dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepSource {
    /// `package.json`.
    Npm,
    /// `requirements.txt` (best-effort; pyproject.toml lands later).
    Pip,
    /// `Cargo.toml`.
    Cargo,
    /// `go.mod`.
    Go,
    /// `Gemfile`.
    Gem,
    /// `composer.json`.
    Composer,
}

/// Dependency category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepKind {
    /// Production / runtime dependency.
    Production,
    /// Development-only (test runner, types, linters).
    Dev,
    /// Peer dependency.
    Peer,
}

/// One parsed dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyInfo {
    /// Package name as written in the manifest.
    pub name: String,
    /// Version range or `*` if unknown.
    pub version: String,
    /// Production / dev / peer.
    pub kind: DepKind,
    /// Where we parsed it from.
    pub source: DepSource,
}

/// One environment variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarInfo {
    /// Variable name (`UPPER_SNAKE_CASE`).
    pub name: String,
    /// True if it appears in `.env.example` (or `.env.sample`).
    pub has_example: bool,
    /// True if it's referenced from code (`process.env.X` / `os.environ["X"]`).
    pub found_in_code: bool,
    /// Trailing-comment description from `.env.example`, if any.
    pub description: Option<String>,
}

/// Git state at scan time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitInfo {
    /// Current branch name.
    pub branch: Option<String>,
    /// `origin` URL.
    pub remote_url: Option<String>,
    /// Last 10 commits in `--oneline` form.
    pub recent_commits: Vec<String>,
    /// Up to 10 unique authors.
    pub authors: Vec<String>,
    /// Whether `git status --porcelain` had any output.
    pub has_uncommitted_changes: bool,
}

/// Recognized CI / deploy targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CIType {
    /// `.github/workflows/*.yml`.
    GithubActions,
    /// `vercel.json` / `.vercel/`.
    Vercel,
    /// `railway.toml` / `railway.json`.
    Railway,
    /// `fly.toml`.
    Fly,
    /// `Dockerfile` / `docker-compose.yml`.
    Docker,
    /// `netlify.toml`.
    Netlify,
    /// `Procfile`.
    Heroku,
    /// Catch-all.
    Other,
}

/// One CI / deploy config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CIConfig {
    /// CI platform.
    pub ci_type: CIType,
    /// Path relative to scan root.
    pub file_path: String,
    /// Inferred deploy target, if known.
    pub deploy_target: Option<String>,
    /// Build command, if extractable from the config.
    pub build_command: Option<String>,
}

/// One detected HTTP route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APIRoute {
    /// HTTP method, uppercased.
    pub method: String,
    /// Logical path. `/` for root.
    pub path: String,
    /// Source file containing the handler.
    pub file_path: String,
    /// 1-indexed line number where the handler was found.
    pub line_number: usize,
}

/// Schema/migration/model file kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaKind {
    /// SQL migration.
    Database,
    /// JSON / OpenAPI / GraphQL schema.
    Api,
    /// Config schema (zod, JSON Schema for config).
    Config,
}

/// One schema-shaped file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaInfo {
    /// Basename of the file.
    pub name: String,
    /// Schema bucket.
    pub kind: SchemaKind,
    /// Path relative to scan root.
    pub file_path: String,
}

/// Severity classes for [`SecurityHint`]. Same order as the rest of
/// the workspace so cross-crate comparisons are total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HintSeverity {
    /// Informational.
    Info,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Critical.
    Critical,
}

/// One security signal raised by the scanner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityHint {
    /// Severity bucket.
    pub severity: HintSeverity,
    /// Short kind tag (`hardcoded-secret`, `committed-secrets`, …).
    pub kind: String,
    /// Free-text detail.
    pub description: String,
    /// Source file path, if applicable.
    pub file_path: Option<String>,
    /// 1-indexed line number, if applicable.
    pub line_number: Option<usize>,
}

// ─── Constants ──────────────────────────────────────────────────────────────

static SKIP_DIRS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "node_modules",
        ".git",
        "dist",
        "build",
        ".next",
        "__pycache__",
        "target",
        ".cache",
        ".turbo",
        "coverage",
        ".nyc_output",
        "venv",
        ".venv",
        "env",
        ".env",
        "vendor",
        "bower_components",
    ]
    .into_iter()
    .collect()
});

static CONFIG_FILES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "package.json",
        "tsconfig.json",
        "vite.config.ts",
        "next.config.js",
        "next.config.ts",
        "vercel.json",
        "fly.toml",
        "railway.toml",
        "docker-compose.yml",
        "Dockerfile",
        // `.env` is intentionally listed alongside `.env.example`: the
        // hidden-file filter would drop it otherwise, and we need it
        // visible to the committed-secret hint.
        ".env",
        ".env.example",
        "pyproject.toml",
        "Cargo.toml",
        "go.mod",
        "Makefile",
        "justfile",
    ]
    .into_iter()
    .collect()
});

// Cap pages so a vendored mega-file can't dominate the scan budget.
const MAX_CODE_FILES_FOR_ENV_SCAN: usize = 50;
const MAX_ROUTE_FILES: usize = 30;
const MAX_FILES_FOR_SECRET_SCAN: usize = 40;
const MAX_FILE_SIZE_FOR_ENV_SCAN: u64 = 500_000;
const MAX_FILE_SIZE_FOR_ROUTES: u64 = 200_000;
const MAX_FILE_SIZE_FOR_SECRETS: u64 = 300_000;

// ─── Scanner ────────────────────────────────────────────────────────────────

/// Top-level driver. Stateless except for the pinned root path.
#[derive(Debug, Clone)]
pub struct CodebaseScanner {
    root_path: PathBuf,
}

impl CodebaseScanner {
    /// Construct a scanner pointed at `root_path`. The path is resolved
    /// to absolute form so subsequent file reads are unambiguous.
    pub fn new(root_path: impl AsRef<Path>) -> Self {
        let root = root_path.as_ref();
        let abs = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        Self { root_path: abs }
    }

    /// Run the full scan.
    pub fn scan(&self) -> ScanResult {
        let files = self.collect_files();
        let tech_stack = self.detect_tech_stack(&files);
        let dependencies = self.extract_dependencies(&files);
        let env_vars = self.extract_env_vars(&files);
        let git_info = self.extract_git_info();
        let ci_configs = self.detect_ci_configs(&files);
        let api_routes = self.extract_api_routes(&files);
        let schemas = self.extract_schemas(&files);
        let security_hints = self.detect_security_hints(&files);

        ScanResult {
            root_path: self.root_path.clone(),
            tech_stack,
            files,
            dependencies,
            env_vars,
            git_info,
            ci_configs,
            api_routes,
            schemas,
            security_hints,
        }
    }

    // ── walker ─────────────────────────────────────────────────────────────

    fn collect_files(&self) -> Vec<FileInfo> {
        let mut out = Vec::new();
        self.collect_files_into(&self.root_path, "", &mut out);
        out
    }

    #[allow(clippy::only_used_in_recursion)]
    fn collect_files_into(&self, dir: &Path, base: &str, out: &mut Vec<FileInfo>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let abs = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // SKIP_DIRS only applies to directories — a *file* called
            // `.env` must reach the security-hint pass so committed
            // secrets are surfaced, not silently dropped because some
            // virtualenv directories share the same name.
            if file_type.is_dir() && SKIP_DIRS.contains(name_str.as_ref()) {
                continue;
            }
            // Hidden entries that aren't known config files are skipped,
            // for both files and directories.
            if name_str.starts_with('.') && !CONFIG_FILES.contains(name_str.as_ref()) {
                continue;
            }
            let rel = if base.is_empty() {
                name_str.to_string()
            } else {
                format!("{base}/{name_str}")
            };
            if file_type.is_dir() {
                self.collect_files_into(&abs, &rel, out);
            } else if file_type.is_file() {
                let size_bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
                let extension = extension_of(&name_str);
                out.push(FileInfo {
                    relative_path: rel,
                    size_bytes,
                    extension,
                    is_config: CONFIG_FILES.contains(name_str.as_ref()),
                });
            }
        }
    }

    // ── tech stack ─────────────────────────────────────────────────────────

    fn detect_tech_stack(&self, files: &[FileInfo]) -> TechStackInfo {
        let basenames: HashSet<&str> = files
            .iter()
            .map(|f| {
                f.relative_path
                    .rsplit_once('/')
                    .map(|(_, n)| n)
                    .unwrap_or(&f.relative_path)
            })
            .collect();
        let has = |name: &str| basenames.contains(name);

        let count_ext = |needle: &str| files.iter().filter(|f| f.extension == needle).count();
        let ts = count_ext(".ts") + count_ext(".tsx");
        let js = count_ext(".js") + count_ext(".jsx");
        let py = count_ext(".py");
        let rs = count_ext(".rs");
        let go = count_ext(".go");

        let language: String = if ts > js && ts > py {
            "TypeScript".into()
        } else if js > py {
            "JavaScript".into()
        } else if py > 0 {
            "Python".into()
        } else if rs > 0 {
            "Rust".into()
        } else if go > 0 {
            "Go".into()
        } else {
            "unknown".into()
        };

        let mut framework: Option<String> = None;
        let mut primary = language.clone();

        if has("next.config.js") || has("next.config.ts") || has("next.config.mjs") {
            framework = Some("Next.js".into());
            primary = "Next.js".into();
        } else if has("vite.config.ts") || has("vite.config.js") {
            framework = Some("Vite".into());
        } else if has("svelte.config.js") || has("svelte.config.ts") {
            framework = Some("SvelteKit".into());
            primary = "SvelteKit".into();
        } else if has("nuxt.config.ts") || has("nuxt.config.js") {
            framework = Some("Nuxt".into());
            primary = "Nuxt".into();
        } else if has("angular.json") {
            framework = Some("Angular".into());
            primary = "Angular".into();
        } else if has("pyproject.toml") {
            framework = Some("Python".into());
            primary = "Python".into();
        } else if has("Cargo.toml") {
            framework = Some("Rust/Cargo".into());
            primary = "Rust".into();
        } else if has("go.mod") {
            framework = Some("Go".into());
            primary = "Go".into();
        }

        let package_manager = if has("pnpm-workspace.yaml") || has("pnpm-lock.yaml") {
            Some("pnpm".into())
        } else if has("yarn.lock") {
            Some("yarn".into())
        } else if has("bun.lockb") {
            Some("bun".into())
        } else if has("package-lock.json") {
            Some("npm".into())
        } else if has("Cargo.lock") {
            Some("cargo".into())
        } else if has("go.sum") {
            Some("go".into())
        } else {
            None
        };

        let deploy_target =
            if has("vercel.json") || files.iter().any(|f| f.relative_path.contains(".vercel")) {
                Some("Vercel".into())
            } else if has("fly.toml") {
                Some("Fly.io".into())
            } else if has("railway.toml") || has("railway.json") {
                Some("Railway".into())
            } else if has("Dockerfile") {
                Some("Docker".into())
            } else if has("netlify.toml") || has("netlify.json") {
                Some("Netlify".into())
            } else if has("Procfile") {
                Some("Heroku".into())
            } else {
                None
            };

        let test_framework = if has("vitest.config.ts") || has("vitest.config.js") {
            Some("Vitest".into())
        } else if has("jest.config.js") || has("jest.config.ts") {
            Some("Jest".into())
        } else if has("pytest.ini") || has("pyproject.toml") {
            Some("pytest".into())
        } else {
            None
        };

        TechStackInfo {
            primary,
            framework,
            language,
            package_manager,
            test_framework,
            deploy_target,
        }
    }

    // ── dependencies ───────────────────────────────────────────────────────

    fn extract_dependencies(&self, files: &[FileInfo]) -> Vec<DependencyInfo> {
        let mut deps = Vec::new();
        if files.iter().any(|f| f.relative_path == "package.json") {
            self.parse_package_json(&mut deps);
        }
        if files.iter().any(|f| f.relative_path == "requirements.txt") {
            self.parse_requirements_txt(&mut deps);
        }
        if files.iter().any(|f| f.relative_path == "Cargo.toml") {
            self.parse_cargo_toml(&mut deps);
        }
        deps
    }

    fn parse_package_json(&self, deps: &mut Vec<DependencyInfo>) {
        let Ok(raw) = std::fs::read_to_string(self.root_path.join("package.json")) else {
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return;
        };
        for (kind, key) in [
            (DepKind::Production, "dependencies"),
            (DepKind::Dev, "devDependencies"),
            (DepKind::Peer, "peerDependencies"),
        ] {
            if let Some(map) = json.get(key).and_then(|v| v.as_object()) {
                for (name, version) in map.iter() {
                    deps.push(DependencyInfo {
                        name: name.clone(),
                        version: version
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| version.to_string()),
                        kind,
                        source: DepSource::Npm,
                    });
                }
            }
        }
    }

    fn parse_requirements_txt(&self, deps: &mut Vec<DependencyInfo>) {
        let Ok(raw) = std::fs::read_to_string(self.root_path.join("requirements.txt")) else {
            return;
        };
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let (name, version) = match trimmed.split_once("==") {
                Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
                None => (trimmed.to_string(), "*".into()),
            };
            deps.push(DependencyInfo {
                name,
                version,
                kind: DepKind::Production,
                source: DepSource::Pip,
            });
        }
    }

    fn parse_cargo_toml(&self, deps: &mut Vec<DependencyInfo>) {
        let Ok(raw) = std::fs::read_to_string(self.root_path.join("Cargo.toml")) else {
            return;
        };
        // Use a permissive section-and-key extraction so we don't need
        // a real TOML parser pulled in just for this. Match keys under
        // [dependencies] up to the next [section].
        static DEPS_SECTION: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(?s)\[dependencies\]\n(.*?)(\n\[|\z)").expect("re"));
        static DEP_KEY: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(?m)^([A-Za-z][A-Za-z0-9_\-]*)\s*=").expect("re"));

        if let Some(caps) = DEPS_SECTION.captures(&raw) {
            let section = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            for c in DEP_KEY.captures_iter(section) {
                deps.push(DependencyInfo {
                    name: c[1].to_string(),
                    version: "*".into(),
                    kind: DepKind::Production,
                    source: DepSource::Cargo,
                });
            }
        }
    }

    // ── env vars ───────────────────────────────────────────────────────────

    fn extract_env_vars(&self, files: &[FileInfo]) -> Vec<EnvVarInfo> {
        let mut by_name: HashMap<String, EnvVarInfo> = HashMap::new();

        // From .env.example or .env.sample.
        let example = files
            .iter()
            .find(|f| f.relative_path == ".env.example" || f.relative_path == ".env.sample");
        if let Some(file) = example {
            if let Ok(raw) = std::fs::read_to_string(self.root_path.join(&file.relative_path)) {
                static EXAMPLE_RE: Lazy<Regex> =
                    Lazy::new(|| Regex::new(r"^([A-Z][A-Z0-9_]+)\s*=").expect("re"));
                for line in raw.lines() {
                    let trimmed = line.trim();
                    if let Some(c) = EXAMPLE_RE.captures(trimmed) {
                        let name = c[1].to_string();
                        let description = trimmed
                            .split_once('#')
                            .map(|(_, rest)| rest.trim().to_string())
                            .filter(|s| !s.is_empty());
                        by_name.entry(name.clone()).or_insert(EnvVarInfo {
                            name,
                            has_example: true,
                            found_in_code: false,
                            description,
                        });
                    }
                }
            }
        }

        // Scan source files for process.env.X / os.environ.get("X").
        static CODE_RE: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r#"process\.env\.([A-Z][A-Z0-9_]+)|os\.environ\.get\(['"]([A-Z][A-Z0-9_]+)"#)
                .expect("re")
        });
        let code_files: Vec<&FileInfo> = files
            .iter()
            .filter(|f| {
                matches!(
                    f.extension.as_str(),
                    ".ts" | ".tsx" | ".js" | ".jsx" | ".py" | ".go"
                ) && f.size_bytes < MAX_FILE_SIZE_FOR_ENV_SCAN
            })
            .take(MAX_CODE_FILES_FOR_ENV_SCAN)
            .collect();

        for file in code_files {
            let Ok(raw) = std::fs::read_to_string(self.root_path.join(&file.relative_path)) else {
                continue;
            };
            for c in CODE_RE.captures_iter(&raw) {
                let name = c
                    .get(1)
                    .or_else(|| c.get(2))
                    .map(|m| m.as_str().to_string());
                if let Some(name) = name {
                    by_name
                        .entry(name.clone())
                        .and_modify(|e| e.found_in_code = true)
                        .or_insert(EnvVarInfo {
                            name,
                            has_example: false,
                            found_in_code: true,
                            description: None,
                        });
                }
            }
        }

        let mut out: Vec<_> = by_name.into_values().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    // ── git ────────────────────────────────────────────────────────────────

    fn extract_git_info(&self) -> GitInfo {
        let exec = |args: &[&str]| -> String {
            Command::new("git")
                .current_dir(&self.root_path)
                .args(args)
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        };

        let branch = exec(&["rev-parse", "--abbrev-ref", "HEAD"]);
        let remote_url = exec(&["config", "--get", "remote.origin.url"]);
        let recent_raw = exec(&["log", "--oneline", "-10"]);
        let recent_commits: Vec<_> = recent_raw
            .lines()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let authors_raw = exec(&["log", "--format=%an"]);
        let mut seen: HashSet<String> = HashSet::new();
        let mut authors = Vec::new();
        for line in authors_raw.lines() {
            let line = line.trim();
            if !line.is_empty() && seen.insert(line.to_string()) {
                authors.push(line.to_string());
                if authors.len() >= 10 {
                    break;
                }
            }
        }
        let status_raw = exec(&["status", "--porcelain"]);

        GitInfo {
            branch: empty_to_none(branch),
            remote_url: empty_to_none(remote_url),
            recent_commits,
            authors,
            has_uncommitted_changes: !status_raw.is_empty(),
        }
    }

    // ── CI configs ─────────────────────────────────────────────────────────

    fn detect_ci_configs(&self, files: &[FileInfo]) -> Vec<CIConfig> {
        let mut out = Vec::new();
        for f in files {
            let p = &f.relative_path;
            if p.starts_with(".github/workflows/") {
                out.push(CIConfig {
                    ci_type: CIType::GithubActions,
                    file_path: p.clone(),
                    deploy_target: None,
                    build_command: None,
                });
            } else if p == "vercel.json" {
                out.push(CIConfig {
                    ci_type: CIType::Vercel,
                    file_path: p.clone(),
                    deploy_target: Some("Vercel".into()),
                    build_command: None,
                });
            } else if p == "fly.toml" {
                out.push(CIConfig {
                    ci_type: CIType::Fly,
                    file_path: p.clone(),
                    deploy_target: Some("Fly.io".into()),
                    build_command: None,
                });
            } else if p == "railway.toml" || p == "railway.json" {
                out.push(CIConfig {
                    ci_type: CIType::Railway,
                    file_path: p.clone(),
                    deploy_target: Some("Railway".into()),
                    build_command: None,
                });
            } else if p == "Dockerfile" || p == "docker-compose.yml" {
                out.push(CIConfig {
                    ci_type: CIType::Docker,
                    file_path: p.clone(),
                    deploy_target: None,
                    build_command: None,
                });
            } else if p == "netlify.toml" {
                out.push(CIConfig {
                    ci_type: CIType::Netlify,
                    file_path: p.clone(),
                    deploy_target: Some("Netlify".into()),
                    build_command: None,
                });
            }
        }
        out
    }

    // ── API routes ─────────────────────────────────────────────────────────

    fn extract_api_routes(&self, files: &[FileInfo]) -> Vec<APIRoute> {
        static APP_ROUTER: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r"export\s+(?:async\s+)?function\s+(GET|POST|PUT|DELETE|PATCH)\s*\(")
                .expect("re")
        });
        static EXPRESS: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r#"(?:app|router)\.(get|post|put|delete|patch)\s*\(['"](\/[^'"]*)"#)
                .expect("re")
        });

        let route_files: Vec<&FileInfo> = files
            .iter()
            .filter(|f| {
                (f.relative_path.contains("/api/") || f.relative_path.contains("/routes/"))
                    && matches!(f.extension.as_str(), ".ts" | ".tsx" | ".js" | ".jsx")
                    && f.size_bytes < MAX_FILE_SIZE_FOR_ROUTES
            })
            .take(MAX_ROUTE_FILES)
            .collect();

        let mut out = Vec::new();
        for file in route_files {
            let Ok(raw) = std::fs::read_to_string(self.root_path.join(&file.relative_path)) else {
                continue;
            };
            for c in APP_ROUTER.captures_iter(&raw) {
                let m = c.get(0).expect("group 0");
                let line_number = raw[..m.start()].lines().count() + 1;
                let path = next_app_router_path(&file.relative_path);
                out.push(APIRoute {
                    method: c[1].to_uppercase(),
                    path,
                    file_path: file.relative_path.clone(),
                    line_number,
                });
            }
            for c in EXPRESS.captures_iter(&raw) {
                let m = c.get(0).expect("group 0");
                let line_number = raw[..m.start()].lines().count() + 1;
                out.push(APIRoute {
                    method: c[1].to_uppercase(),
                    path: c[2].to_string(),
                    file_path: file.relative_path.clone(),
                    line_number,
                });
            }
        }
        out
    }

    // ── schemas ────────────────────────────────────────────────────────────

    fn extract_schemas(&self, files: &[FileInfo]) -> Vec<SchemaInfo> {
        let mut out = Vec::new();
        for f in files {
            let base = f
                .relative_path
                .rsplit_once('/')
                .map(|(_, n)| n)
                .unwrap_or(&f.relative_path)
                .to_lowercase();
            let kind = if base.contains("migration") {
                Some(SchemaKind::Database)
            } else if base.contains("schema") {
                Some(SchemaKind::Api)
            } else if base.contains("model") {
                Some(SchemaKind::Config)
            } else {
                None
            };
            if let Some(kind) = kind {
                out.push(SchemaInfo {
                    name: base,
                    kind,
                    file_path: f.relative_path.clone(),
                });
            }
        }
        out
    }

    // ── security hints ─────────────────────────────────────────────────────

    fn detect_security_hints(&self, files: &[FileInfo]) -> Vec<SecurityHint> {
        let mut hints = Vec::new();

        // .env committed.
        if files.iter().any(|f| f.relative_path == ".env") {
            hints.push(SecurityHint {
                severity: HintSeverity::Critical,
                kind: "committed-secrets".into(),
                description: ".env file is present in the repository — may contain real secrets"
                    .into(),
                file_path: Some(".env".into()),
                line_number: None,
            });
        }

        static API_SECRET: Lazy<Regex> = Lazy::new(|| {
            Regex::new(
                r#"(?i)(?:api_?key|apikey|secret|password|token)\s*[:=]\s*['"][^'"]{10,}['"]"#,
            )
            .expect("re")
        });
        static OPENAI_KEY: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"sk-[a-zA-Z0-9]{20,}").expect("re"));
        static JWT: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"eyJ[A-Za-z0-9_\-]{20,}\.[A-Za-z0-9_\-]{10,}").expect("re"));

        let code_files: Vec<&FileInfo> = files
            .iter()
            .filter(|f| {
                matches!(
                    f.extension.as_str(),
                    ".ts" | ".tsx" | ".js" | ".jsx" | ".py" | ".go" | ".env"
                ) && f.size_bytes < MAX_FILE_SIZE_FOR_SECRETS
                    && !f.relative_path.contains("test")
                    && !f.relative_path.contains("spec")
            })
            .take(MAX_FILES_FOR_SECRET_SCAN)
            .collect();

        for file in code_files {
            let Ok(raw) = std::fs::read_to_string(self.root_path.join(&file.relative_path)) else {
                continue;
            };
            for (re, kind) in [
                (&*API_SECRET, "hardcoded-secret"),
                (&*OPENAI_KEY, "openai-key"),
                (&*JWT, "jwt-token"),
            ] {
                for m in re.find_iter(&raw) {
                    let line_number = raw[..m.start()].lines().count() + 1;
                    hints.push(SecurityHint {
                        severity: HintSeverity::Critical,
                        kind: kind.into(),
                        description: format!(
                            "Potential {} found in {}:{}",
                            kind, file.relative_path, line_number
                        ),
                        file_path: Some(file.relative_path.clone()),
                        line_number: Some(line_number),
                    });
                }
            }
        }

        if !files.iter().any(|f| f.relative_path == ".gitignore") {
            hints.push(SecurityHint {
                severity: HintSeverity::High,
                kind: "missing-gitignore".into(),
                description: "No .gitignore file found".into(),
                file_path: None,
                line_number: None,
            });
        }

        hints
    }
}

/// Convenience entry point. `let scan = engulf::scanner::scan(root)?;`
pub fn scan(root: &Path) -> std::io::Result<ScanResult> {
    Ok(CodebaseScanner::new(root).scan())
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => name[i..].to_lowercase(),
        _ => String::new(),
    }
}

fn empty_to_none(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn next_app_router_path(rel: &str) -> String {
    static SRC_APP: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:src/)?app").expect("re"));
    static ROUTE_END: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"/route\.(?:ts|js|tsx|jsx)$").expect("re"));
    static PAGE_END: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"/page\.(?:ts|js|tsx|jsx)$").expect("re"));

    let stripped = SRC_APP.replace(rel, "").to_string();
    let stripped = ROUTE_END.replace(&stripped, "").to_string();
    let stripped = PAGE_END.replace(&stripped, "").to_string();
    if stripped.is_empty() {
        "/".into()
    } else {
        stripped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn collects_files_skipping_node_modules() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(&p.join("src/main.rs"), "fn main() {}");
        touch(&p.join("node_modules/x/index.js"), "module.exports = {};");
        let r = CodebaseScanner::new(p).scan();
        assert!(r.files.iter().any(|f| f.relative_path.ends_with("main.rs")));
        assert!(!r
            .files
            .iter()
            .any(|f| f.relative_path.contains("node_modules")));
    }

    #[test]
    fn detects_rust_stack() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(
            &p.join("Cargo.toml"),
            "[package]\nname = \"x\"\n[dependencies]\nserde = \"1\"\n",
        );
        touch(&p.join("src/main.rs"), "fn main() {}");
        let r = CodebaseScanner::new(p).scan();
        assert_eq!(r.tech_stack.primary, "Rust");
        assert!(r.dependencies.iter().any(|d| d.name == "serde"));
    }

    #[test]
    fn parses_package_json_dependencies() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(
            &p.join("package.json"),
            r#"{"dependencies":{"react":"^18"},"devDependencies":{"vitest":"^1"}}"#,
        );
        let r = CodebaseScanner::new(p).scan();
        assert!(r
            .dependencies
            .iter()
            .any(|d| d.name == "react" && d.kind == DepKind::Production));
        assert!(r
            .dependencies
            .iter()
            .any(|d| d.name == "vitest" && d.kind == DepKind::Dev));
    }

    #[test]
    fn detects_committed_env_as_critical() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(&p.join(".env"), "OPENAI_API_KEY=hidden\n");
        touch(&p.join(".gitignore"), "");
        let r = CodebaseScanner::new(p).scan();
        assert!(r
            .security_hints
            .iter()
            .any(|h| h.kind == "committed-secrets" && h.severity == HintSeverity::Critical));
    }

    #[test]
    fn flags_missing_gitignore() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(&p.join("src/main.rs"), "fn main() {}");
        let r = CodebaseScanner::new(p).scan();
        assert!(r
            .security_hints
            .iter()
            .any(|h| h.kind == "missing-gitignore"));
    }

    #[test]
    fn extracts_env_vars_from_example_and_code() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(
            &p.join(".env.example"),
            "API_URL=  # base URL of the API\nDB_PASSWORD=\n",
        );
        touch(
            &p.join("src/index.ts"),
            r#"const x = process.env.API_URL; const y = process.env.UNDOC_VAR;"#,
        );
        let r = CodebaseScanner::new(p).scan();
        let api = r.env_vars.iter().find(|v| v.name == "API_URL").unwrap();
        assert!(api.has_example);
        assert!(api.found_in_code);
        let undoc = r.env_vars.iter().find(|v| v.name == "UNDOC_VAR").unwrap();
        assert!(!undoc.has_example);
        assert!(undoc.found_in_code);
    }

    #[test]
    fn detects_next_app_router_route() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(
            &p.join("app/api/widgets/route.ts"),
            "export async function GET(req: Request) { return new Response(); }",
        );
        let r = CodebaseScanner::new(p).scan();
        let route = r
            .api_routes
            .iter()
            .find(|r| r.method == "GET")
            .expect("route");
        assert_eq!(route.path, "/api/widgets");
    }
}
