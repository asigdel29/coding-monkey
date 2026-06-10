# Contributing

Short guide so you can land a PR cleanly. The CI gauntlet on every push
mirrors what's expected of merged code.

## Local setup

```bash
rustup default stable               # toolchain pinned in rust-toolchain.toml
rustup component add rustfmt clippy
rustup target add wasm32-unknown-unknown
cargo install cargo-deny --locked   # license + advisory checks
cargo install trunk wasm-bindgen-cli # WASM frontend (crates/web)
```

## Pre-commit gauntlet

The gauntlet that CI runs, in order. Run it locally before opening a PR
to keep iterations short.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build  --workspace --all-targets --locked
cargo test   --workspace --all-targets --locked
cargo doc    --workspace --no-deps --exclude monkey-web
cargo deny   check --workspace
cargo build  -p monkey-web --target wasm32-unknown-unknown --locked
```

A one-liner version:

```bash
cargo fmt --all -- --check && \
  cargo clippy --workspace --all-targets --all-features -- -D warnings && \
  cargo test --workspace --all-targets --locked
```

## File header convention

Every Rust source file begins with the header pattern documented in the
README — a `/* File: … Purpose … History … */` block before the first
`use`. Update the History row when you make a non-trivial change to a
file you're already touching.

## Pull requests

- **Small, stacked PRs.** One cohesive change per PR — aim for **~20 files /
  ~200 LOC** of meaningful diff (excluding lockfiles and generated artifacts).
  If a change is bigger, split it into several PRs.
- **Dependency order.** When a PR builds on another, base its branch on the
  parent branch and merge parents first (`gh pr create --base <parent-branch>`).
- **Vertical slices.** Land plumbing first (types + wiring), then one UI surface
  per PR.
- **Branch from `main`** (`git checkout main && git pull origin main`), one
  feature branch per slice, open the PR against `origin/main`.
- **Clean up after merge** — delete the branch on the remote
  (`git push origin --delete <branch>`) and locally (`git branch -d <branch>`).
- **No AI authorship attribution** anywhere in commits, PR titles/bodies, or
  docs (drop any `Co-Authored-By` / "Generated with" footers).

## Commit style

Conventional Commits. The first line is `<type>(<scope>): <subject>`.

```
feat(skills): add codegen skill with diff-aware prompt
fix(deck): clamp WS rate-limit refill to wall-clock dt
chore(deps): bump tokio to 1.42
docs: clarify .monkey/context/ load order
```

Body explains the *why*, includes any file paths the change touches,
and links the commit to a changelog entry when the change is
user-visible.

## Tests

- Unit tests live alongside the code in `#[cfg(test)] mod tests {}`.
- Integration tests under `crates/<name>/tests/` use real PTYs / real
  HTTP servers via `reqwest::Client`. Avoid mocking what we own.
- Tempdir fixtures via `tempfile::tempdir()` so tests don't pollute the
  workspace.

## Security

- `cargo audit` and `cargo deny check` run on every push.
- `monkey pentest --cwd .` runs in CI as a self-scan.
- Never commit secrets — gitleaks runs as a final-line defense.

## Cutting a release

```bash
# pick a new version, run the ship gauntlet
monkey ship --bump minor

# tag and push — release.yml builds binaries for 5 targets and
# attaches them to the GitHub Release
git tag v0.2.0
git push origin v0.2.0
```
