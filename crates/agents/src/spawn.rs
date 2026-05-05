/*
   File: crates/agents/src/spawn.rs

   Purpose
   PTY-spawn the chosen agent CLI with the assembled system prompt
   piped through stdin. The returned `AgentTerminal` lets a UI layer
   (CLI REPL, deck WebSocket) read/write/resize/kill the underlying
   process.

   The PTY abstraction is `portable-pty`, which works on macOS, Linux,
   and Windows ConPTY without our caring about platform differences.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port from packages/agents/src/spawn.ts
*/

use anyhow::{anyhow, Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::context::{assemble_context, AssembledContext};
use crate::doctor::{doctor, pick_auto};
use crate::types::{AgentKind, SpawnOpts};

/// Returned to the caller of [`spawn_agent`]. Holds the live PTY plus
/// metadata about which agent was chosen and what context was loaded.
#[derive(Debug)]
pub struct SpawnResult {
    /// The live agent terminal — read/write/resize/kill via this handle.
    pub terminal: AgentTerminal,
    /// Resolved kind (auto → claude or codex).
    pub kind: AgentKind,
    /// Path of the binary that was launched.
    pub binary: String,
    /// Context assembled from `.monkey/`.
    pub context: AssembledContext,
}

/// Live PTY connection to a running agent process.
#[derive(Debug)]
pub struct AgentTerminal {
    /// Stable id (uuid v7 prefixed by `term_`).
    pub id: String,
    inner: Arc<Mutex<TerminalInner>>,
}

#[derive(Debug)]
struct TerminalInner {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl AgentTerminal {
    /// Write bytes to the agent's stdin.
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut g = self.inner.lock().expect("terminal lock");
        g.writer
            .write_all(data)
            .context("write to agent terminal failed")?;
        Ok(())
    }

    /// Resize the underlying PTY. Call on host TTY resize.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let g = self.inner.lock().expect("terminal lock");
        g.master
            .resize(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("resize failed: {}", e))
    }

    /// Send SIGKILL (or platform equivalent) to the child.
    pub fn kill(&self) -> Result<()> {
        let mut g = self.inner.lock().expect("terminal lock");
        g.child.kill().context("kill agent terminal failed")?;
        Ok(())
    }

    /// Wait for the child to exit and return its exit code.
    pub fn wait(&self) -> Result<i32> {
        let mut g = self.inner.lock().expect("terminal lock");
        let status = g.child.wait().context("wait failed")?;
        Ok(status.exit_code() as i32)
    }
}

/// Spawn an agent. The returned `SpawnResult` includes the assembled
/// context so the caller can surface "loaded N files, M KB" telemetry.
///
/// `on_data` is invoked on every chunk read from the PTY's stdout —
/// callers should pipe it to their UI (REPL stdout, WebSocket frame,
/// audit log after redaction, etc.).
pub fn spawn_agent<F>(opts: SpawnOpts, mut on_data: F) -> Result<SpawnResult>
where
    F: FnMut(&[u8]) + Send + 'static,
{
    // 1. Resolve `Auto` to a concrete kind via the doctor report.
    let report = doctor();
    if !report.ok {
        let notes = report.notes.join("; ");
        return Err(anyhow!("doctor failed: {notes}"));
    }
    let kind = match opts.kind {
        AgentKind::Auto => pick_auto(&report)
            .ok_or_else(|| anyhow!("no agent CLI available"))?,
        explicit => explicit,
    };
    let binary = match kind {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
        AgentKind::Auto => unreachable!("Auto resolved above"),
    };

    // 2. Assemble the system prompt.
    let context = assemble_context(&opts.cwd, kind, &opts.tentacle_id);

    // 3. Set up the PTY.
    let pty_system = native_pty_system();
    let (cols, rows) = opts.size.unwrap_or((100, 30));
    let pair = pty_system
        .openpty(PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| anyhow!("openpty failed: {}", e))?;

    // 4. Build the child command.
    let mut cmd = CommandBuilder::new(binary);
    cmd.cwd(&opts.cwd);
    for arg in &opts.extra_args {
        cmd.arg(arg);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| anyhow!("spawn {binary} failed: {}", e))?;
    drop(pair.slave); // close our copy of the slave

    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| anyhow!("take_writer failed: {}", e))?;

    // 5. Pipe the assembled prompt as the first stdin chunk so the agent
    //    sees it as its initial system context.
    if !context.prompt.is_empty() {
        writeln!(writer, "{}", context.prompt).context("write initial prompt failed")?;
    }

    let inner = Arc::new(Mutex::new(TerminalInner {
        master: pair.master,
        writer,
        child,
    }));

    // 6. Reader thread — pumps PTY → on_data callback. Keeps a clone of
    //    the master via the Arc so the lifetime is correct.
    {
        let inner = Arc::clone(&inner);
        thread::Builder::new()
            .name("monkey-agent-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    let mut reader = match inner.lock() {
                        Ok(g) => match g.master.try_clone_reader() {
                            Ok(r) => r,
                            Err(_) => return,
                        },
                        Err(_) => return,
                    };
                    match reader.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => on_data(&buf[..n]),
                        Err(_) => return,
                    }
                }
            })
            .context("spawn reader thread failed")?;
    }

    let id = monkey_core::generate_id("term");
    Ok(SpawnResult {
        terminal: AgentTerminal { id, inner },
        kind,
        binary: binary.to_string(),
        context,
    })
}
