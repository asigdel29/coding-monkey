/*
   File: crates/runtime/src/scheduler.rs

   Purpose
   Run many native agents under bounded concurrency. Each submitted job runs
   as a tokio task gated by a per-class semaphore so that long-lived scoped
   agents and the shared throughput queue each have guaranteed slots and
   neither starves the other. A memory watchdog gates admission so a burst
   can't push a small host into swap, and an outstanding-work cap rejects
   submissions past the queue depth instead of growing unboundedly. Every
   job carries a cancellation token for cooperative stop.

   The scheduler is generic over the work: a job is a closure producing a
   future given its cancel token. `native_agent_job` builds one that runs
   `run_agent` through a limiter-gated backend.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — class-quota scheduler + admission
*/

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::FutureExt;

use monkey_core::{MemoryWatchdog, ModelSpec};
use thiserror::Error;
use tokio::sync::mpsc::Sender;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::agent::{run_agent, LimitedBackend};
use crate::event::AgentEvent;
use crate::limiter::ProviderLimiter;
use crate::llm::NativeLlm;
use crate::state::AgentConfig;
use crate::tool::ToolRegistry;

/// A job's future, produced once it is admitted.
pub type JobFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Whether a job is a long-lived scoped agent or a shared-queue worker.
/// Each class draws from its own slot pool so neither starves the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkClass {
    /// Long-lived agent bound to a scope/tentacle.
    Scoped,
    /// Short-lived worker pulling from the shared throughput queue.
    Shared,
}

/// A unit of work for the scheduler.
pub struct AgentJob {
    /// Stable id for cancellation and tracking.
    pub id: String,
    /// Slot pool to draw from.
    pub class: WorkClass,
    /// Produces the job's future given its cancellation token.
    pub run: Box<dyn FnOnce(CancellationToken) -> JobFuture + Send>,
}

impl std::fmt::Debug for AgentJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentJob")
            .field("id", &self.id)
            .field("class", &self.class)
            .finish_non_exhaustive()
    }
}

/// Why a submission was refused.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SubmitError {
    /// Outstanding work has reached the configured cap.
    #[error("scheduler queue is full")]
    QueueFull,
    /// The memory watchdog denied admission.
    #[error("admission denied: host is at its memory floor")]
    MemoryFloor,
}

/// Scheduler tunables.
#[derive(Debug, Clone, Copy)]
pub struct SchedulerConfig {
    /// Total concurrent agents (split across classes).
    pub max_agents: usize,
    /// Slots reserved for the scoped class; the rest go to shared.
    pub reserved_scoped: usize,
    /// Extra submissions allowed to queue beyond `max_agents`.
    pub queue_capacity: usize,
}

impl SchedulerConfig {
    /// Derive a config from a native-agent ceiling, reserving a quarter of
    /// the slots for scoped agents and queueing up to one extra ceiling.
    pub fn from_max_agents(max_agents: usize) -> Self {
        let max_agents = max_agents.max(1);
        Self {
            max_agents,
            reserved_scoped: (max_agents / 4).max(1),
            queue_capacity: max_agents,
        }
    }
}

/// Point-in-time scheduler counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerStats {
    /// Jobs accepted.
    pub submitted: u64,
    /// Jobs finished.
    pub completed: u64,
    /// Submissions refused.
    pub rejected: u64,
    /// Jobs currently tracked (queued or running).
    pub running: u64,
}

/// Runs native agents under bounded, class-partitioned concurrency.
#[derive(Debug)]
pub struct Scheduler {
    scoped_sem: Arc<Semaphore>,
    shared_sem: Arc<Semaphore>,
    watchdog: Arc<MemoryWatchdog>,
    jobs: Arc<Mutex<HashMap<String, CancellationToken>>>,
    max_outstanding: usize,
    submitted: AtomicU64,
    // Shared with spawned tasks so each can record its own completion.
    completed: Arc<AtomicU64>,
    rejected: AtomicU64,
}

impl Scheduler {
    /// Build a scheduler with `cfg` and a memory `watchdog`.
    pub fn new(cfg: SchedulerConfig, watchdog: Arc<MemoryWatchdog>) -> Self {
        let scoped = cfg.reserved_scoped.min(cfg.max_agents).max(1);
        let shared = cfg.max_agents.saturating_sub(scoped).max(1);
        Self {
            scoped_sem: Arc::new(Semaphore::new(scoped)),
            shared_sem: Arc::new(Semaphore::new(shared)),
            watchdog,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            max_outstanding: cfg.max_agents + cfg.queue_capacity,
            submitted: AtomicU64::new(0),
            completed: Arc::new(AtomicU64::new(0)),
            rejected: AtomicU64::new(0),
        }
    }

    /// Admit and spawn `job`, or reject it. Returns the job id on success.
    pub fn submit(&self, job: AgentJob) -> Result<String, SubmitError> {
        if self.watchdog.admit().is_err() {
            self.rejected.fetch_add(1, Ordering::SeqCst);
            return Err(SubmitError::MemoryFloor);
        }
        let outstanding = self
            .submitted
            .load(Ordering::SeqCst)
            .saturating_sub(self.completed.load(Ordering::SeqCst));
        if outstanding as usize >= self.max_outstanding {
            self.rejected.fetch_add(1, Ordering::SeqCst);
            return Err(SubmitError::QueueFull);
        }

        let id = job.id.clone();
        let token = CancellationToken::new();
        self.jobs
            .lock()
            .expect("jobs lock")
            .insert(id.clone(), token.clone());
        self.submitted.fetch_add(1, Ordering::SeqCst);

        let sem = match job.class {
            WorkClass::Scoped => Arc::clone(&self.scoped_sem),
            WorkClass::Shared => Arc::clone(&self.shared_sem),
        };
        let jobs = Arc::clone(&self.jobs);
        let completed = Arc::clone(&self.completed);
        let run = job.run;
        let task_id = id.clone();

        tokio::spawn(async move {
            // Queuing happens here: wait for a class slot.
            let _permit = sem.acquire_owned().await.expect("semaphore never closed");
            if !token.is_cancelled() {
                // Contain a panicking agent (release builds use panic=unwind)
                // so it can't take down the runtime and so the cleanup below
                // always runs. One bad agent must not kill the other 99.
                if AssertUnwindSafe(run(token.clone()))
                    .catch_unwind()
                    .await
                    .is_err()
                {
                    tracing::error!(agent_id = %task_id, "native agent task panicked");
                }
            }
            jobs.lock().expect("jobs lock").remove(&task_id);
            completed.fetch_add(1, Ordering::SeqCst);
        });
        Ok(id)
    }

    /// Cancel a running/queued job by id. Returns whether it was found.
    pub fn cancel(&self, id: &str) -> bool {
        if let Some(tok) = self.jobs.lock().expect("jobs lock").get(id) {
            tok.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel everything currently tracked.
    pub fn cancel_all(&self) {
        for tok in self.jobs.lock().expect("jobs lock").values() {
            tok.cancel();
        }
    }

    /// A snapshot of the counters.
    pub fn stats(&self) -> SchedulerStats {
        SchedulerStats {
            submitted: self.submitted.load(Ordering::SeqCst),
            completed: self.completed.load(Ordering::SeqCst),
            rejected: self.rejected.load(Ordering::SeqCst),
            running: self.jobs.lock().expect("jobs lock").len() as u64,
        }
    }
}

/// Build an [`AgentJob`] that runs a native agent: `run_agent` driven by a
/// limiter-gated `NativeLlm`, emitting progress on `events`.
#[allow(clippy::too_many_arguments)]
pub fn native_agent_job(
    id: String,
    class: WorkClass,
    cfg: AgentConfig,
    llm: Arc<NativeLlm>,
    limiter: Arc<ProviderLimiter>,
    tools: Arc<ToolRegistry>,
    model: ModelSpec,
    events: Sender<AgentEvent>,
) -> AgentJob {
    let job_id = id.clone();
    let run = Box::new(move |cancel: CancellationToken| -> JobFuture {
        Box::pin(async move {
            let backend = Arc::new(LimitedBackend::new(llm, limiter));
            let _ = run_agent(job_id, cfg, tools, backend, model, events, cancel).await;
        })
    });
    AgentJob { id, class, run }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    fn watchdog() -> Arc<MemoryWatchdog> {
        Arc::new(MemoryWatchdog::new(0)) // floor 0: always admits
    }

    fn trivial_job(id: &str, flag: Arc<AtomicU32>) -> AgentJob {
        AgentJob {
            id: id.into(),
            class: WorkClass::Shared,
            run: Box::new(move |_cancel| {
                Box::pin(async move {
                    flag.fetch_add(1, Ordering::SeqCst);
                })
            }),
        }
    }

    #[tokio::test]
    async fn runs_submitted_jobs() {
        let sched = Scheduler::new(SchedulerConfig::from_max_agents(8), watchdog());
        let ran = Arc::new(AtomicU32::new(0));
        for i in 0..5 {
            sched
                .submit(trivial_job(&format!("j{i}"), Arc::clone(&ran)))
                .unwrap();
        }
        // Let the spawned tasks complete.
        for _ in 0..50 {
            if ran.load(Ordering::SeqCst) == 5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(ran.load(Ordering::SeqCst), 5);
        assert_eq!(sched.stats().submitted, 5);
    }

    #[tokio::test]
    async fn rejects_past_outstanding_cap() {
        // 1 agent, no queue → only one outstanding allowed at a time.
        let cfg = SchedulerConfig {
            max_agents: 1,
            reserved_scoped: 1,
            queue_capacity: 0,
        };
        let sched = Scheduler::new(cfg, watchdog());
        let gate = Arc::new(AtomicU32::new(0));
        // A job that blocks until told, holding the single slot.
        let blocker = AgentJob {
            id: "block".into(),
            class: WorkClass::Scoped,
            run: {
                let gate = Arc::clone(&gate);
                Box::new(move |_c| {
                    Box::pin(async move {
                        while gate.load(Ordering::SeqCst) == 0 {
                            tokio::time::sleep(Duration::from_millis(2)).await;
                        }
                    })
                })
            },
        };
        sched.submit(blocker).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Fill the one scoped slot's outstanding budget, then overflow.
        let mut rejected = false;
        for i in 0..5 {
            if let Err(SubmitError::QueueFull) =
                sched.submit(trivial_job(&format!("x{i}"), Arc::new(AtomicU32::new(0))))
            {
                rejected = true;
                break;
            }
        }
        gate.store(1, Ordering::SeqCst);
        assert!(rejected, "expected a QueueFull rejection past the cap");
    }

    #[tokio::test]
    async fn contains_panicking_job() {
        let sched = Scheduler::new(SchedulerConfig::from_max_agents(4), watchdog());
        let panic_job = AgentJob {
            id: "boom".into(),
            class: WorkClass::Shared,
            run: Box::new(|_c| Box::pin(async { panic!("kaboom") })),
        };
        sched.submit(panic_job).unwrap();
        // A normal job submitted afterwards still runs — the panic was contained.
        let ran = Arc::new(AtomicU32::new(0));
        sched.submit(trivial_job("ok", Arc::clone(&ran))).unwrap();
        for _ in 0..100 {
            if ran.load(Ordering::SeqCst) == 1 && sched.stats().completed == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        // Both jobs counted complete — the panicker's cleanup ran too.
        assert_eq!(sched.stats().completed, 2);
    }

    #[tokio::test]
    async fn cancel_marks_token() {
        let sched = Scheduler::new(SchedulerConfig::from_max_agents(4), watchdog());
        let observed = Arc::new(AtomicU32::new(0));
        let job = AgentJob {
            id: "c".into(),
            class: WorkClass::Scoped,
            run: {
                let observed = Arc::clone(&observed);
                Box::new(move |cancel| {
                    Box::pin(async move {
                        cancel.cancelled().await;
                        observed.fetch_add(1, Ordering::SeqCst);
                    })
                })
            },
        };
        sched.submit(job).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(sched.cancel("c"));
        for _ in 0..50 {
            if observed.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(observed.load(Ordering::SeqCst), 1);
        assert!(!sched.cancel("missing"));
    }
}
