//! Hades — Pipeline overseer thread.
//!
//! Hades owns the rayon thread pool and manages task dispatch/result collection.
//! The Witch sends dispatch commands; Hades executes tasks on its pool,
//! catches panics, and forwards results back.
//!
//! ## Responsibilities
//! - Owns a rayon `ThreadPool` instance (not the global pool)
//! - Dispatches tasks with panic catching
//! - Drains rayon results and forwards to Witch
//! - Handles pool rebuild on config changes (thread count)
//! - Propagates cache_size changes to thread-local connections

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::config::{self as config_mod, PerformanceOpinions};

use super::execution::execute_task;
use super::types::{ManagedThread, TaskKind, TaskResult, Task};

// ============================================================================
// Protocol
// ============================================================================

enum HadesCommand {
    Dispatch { task: Task, label: String },
    UpdateConfig { opinions: PerformanceOpinions },
    Shutdown,
}

pub(super) enum HadesMessage {
    Result(TaskResult),
}

// ============================================================================
// HadesHandle (Witch-side)
// ============================================================================

/// Witch-side handle for the Hades pipeline thread.
pub(super) struct HadesHandle {
    command_tx: Sender<HadesCommand>,
    message_rx: Receiver<HadesMessage>,
    handle: Option<JoinHandle<()>>,
}

impl HadesHandle {
    /// Spawn the Hades thread. Creates a rayon ThreadPool sized per global config.
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (message_tx, message_rx) = mpsc::channel();

        let initial_threads = config_mod::get_worker_thread_count();

        let handle = std::thread::Builder::new()
            .name("mm-hades".to_string())
            .spawn(move || {
                run_hades(command_rx, message_tx, initial_threads);
            })
            .expect("failed to spawn Hades thread");

        crate::logging::log_general(format!(
            "[HADES] Spawned with {} worker threads",
            initial_threads
        ));

        Self {
            command_tx,
            message_rx,
            handle: Some(handle),
        }
    }

    /// Dispatch a task for execution on the rayon pool.
    pub fn dispatch(&self, task: Task, label: String) {
        let _ = self.command_tx.send(HadesCommand::Dispatch { task, label });
    }

    /// Non-blocking drain of completed results.
    pub fn drain_results(&self) -> impl Iterator<Item = TaskResult> + '_ {
        std::iter::from_fn(move || match self.message_rx.try_recv() {
            Ok(HadesMessage::Result(r)) => Some(r),
            Err(_) => None,
        })
    }

    /// Send updated performance config to Hades for pool rebuild.
    pub fn update_config(&self, opinions: &PerformanceOpinions) {
        let _ = self.command_tx.send(HadesCommand::UpdateConfig {
            opinions: opinions.clone(),
        });
    }
}

impl ManagedThread for HadesHandle {
    fn send_shutdown(&self) {
        let _ = self.command_tx.send(HadesCommand::Shutdown);
    }

    fn take_handle(&mut self) -> Option<JoinHandle<()>> {
        self.handle.take()
    }
}

impl Drop for HadesHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// Thread Function
// ============================================================================

/// Build a rayon ThreadPool with the given thread count.
fn build_pool(num_threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .thread_name(|i| format!("mm-worker-{}", i))
        .build()
        .expect("failed to build rayon thread pool")
}

/// Close thread-local DB connections on all threads in the pool.
fn close_pool_connections(pool: &rayon::ThreadPool) {
    pool.broadcast(|_| {
        crate::meta::computations::close_thread_local_connection();
    });
}

fn run_hades(
    command_rx: Receiver<HadesCommand>,
    message_tx: Sender<HadesMessage>,
    initial_threads: usize,
) {
    crate::logging::log_general("[HADES] Thread started");

    let mut pool = build_pool(initial_threads);

    // Internal channel: rayon workers → Hades loop
    let (result_tx, result_rx) = mpsc::channel::<TaskResult>();

    loop {
        // Check for commands (non-blocking with short timeout)
        match command_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(HadesCommand::Dispatch { task, label }) => {
                let tx = result_tx.clone();
                let label_for_panic = label.clone();
                let kind_for_panic = TaskKind::from_task(&task);
                pool.spawn(move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            execute_task(task, label)
                        }));
                    let result = match result {
                        Ok(r) => r,
                        Err(e) => {
                            let panic_msg = if let Some(s) = e.downcast_ref::<&str>() {
                                s.to_string()
                            } else if let Some(s) = e.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "Unknown panic".to_string()
                            };
                            TaskResult {
                                success: false,
                                error: Some(format!("Task panicked: {}", panic_msg)),
                                label: label_for_panic,
                                kind: kind_for_panic,
                                spawn: Vec::new(),
                                spawn_mutations: Vec::new(),
                                config_update: None,
                                recomputation_scope:
                                    crate::meta::recomputation::RecomputationScope::EMPTY,
                                fetch_result: None,
                                deferred_phases: VecDeque::new(),
                            }
                        }
                    };
                    let _ = tx.send(result);
                });
            }
            Ok(HadesCommand::UpdateConfig { opinions }) => {
                let new_thread_count = opinions
                    .worker_threads
                    .unwrap_or_else(|| {
                        std::thread::available_parallelism()
                            .map(|n| n.get() * 2)
                            .unwrap_or(8)
                    });

                let old_thread_count = pool.current_num_threads();

                // Update the atomic cache_kb so thread-local connections pick it up lazily
                let new_cache_kb = -(opinions.db_cache_mb as i64 * 1024);
                config_mod::set_db_cache_kb(new_cache_kb);

                if new_thread_count != old_thread_count {
                    crate::logging::log_general(format!(
                        "[HADES] Rebuilding pool: {} → {} threads",
                        old_thread_count, new_thread_count
                    ));
                    // Close thread-local DB connections on old pool
                    close_pool_connections(&pool);
                    // Build new pool
                    pool = build_pool(new_thread_count);
                }
            }
            Ok(HadesCommand::Shutdown) => {
                crate::logging::log_general("[HADES] Shutdown requested");
                close_pool_connections(&pool);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Normal — fall through to drain results
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                crate::logging::log_general("[HADES] Command channel disconnected, shutting down");
                close_pool_connections(&pool);
                break;
            }
        }

        // Drain completed results from rayon workers → forward to Witch
        while let Ok(result) = result_rx.try_recv() {
            let _ = message_tx.send(HadesMessage::Result(result));
        }
    }

    crate::logging::log_general("[HADES] Thread exiting");
}
