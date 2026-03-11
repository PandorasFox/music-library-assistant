//! ExternalFetchHandle -- Witch-side API for the scheduler thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::config::SharedConfig;

use super::types::{FetchCommand, FetchOutcome, SchedulerMessage};

/// Handle held by the Witch for communicating with the scheduler thread.
pub struct ExternalFetchHandle {
    /// Send commands to scheduler (start, shutdown).
    command_tx: Sender<FetchCommand>,
    /// Receive messages from scheduler (task requests + status).
    message_rx: Receiver<SchedulerMessage>,
    /// Send outcomes back to scheduler (for chain-emit).
    outcome_tx: Sender<FetchOutcome>,
    /// Join handle for the scheduler thread.
    handle: Option<JoinHandle<()>>,
    /// Whether the scheduler is currently active.
    batch_active: bool,
}

impl ExternalFetchHandle {
    /// Spawn the scheduler thread.
    ///
    /// The scheduler sleeps until it receives a Start command, then
    /// dispatches tasks to the Witch via the message channel.
    pub fn spawn(shared_config: SharedConfig) -> Self {
        // Witch -> Scheduler: commands
        let (command_tx, command_rx) = mpsc::channel();
        // Scheduler -> Witch: task requests + status
        let (message_tx, message_rx) = mpsc::channel();
        // Witch -> Scheduler: outcomes for chain-emit
        let (outcome_tx, outcome_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            super::run_scheduler(command_rx, message_tx, outcome_rx, shared_config);
        });

        Self {
            command_tx,
            message_rx,
            outcome_tx,
            handle: Some(handle),
            batch_active: false,
        }
    }

    /// Request an external metadata fetch for the given directories.
    ///
    /// Populates both the AcoustID queue (fingerprint lookups) and
    /// the MB queue (recording/artist/release enrichment) upfront.
    pub fn request_fetch(&mut self, eligible_dirs: Vec<PathBuf>) {
        if self.batch_active {
            return; // Don't stack requests
        }
        self.batch_active = true;
        let _ = self.command_tx.send(FetchCommand::Start { eligible_dirs });
    }

    /// Drain available messages from the scheduler (non-blocking).
    ///
    /// Returns messages received since last drain. Also clears batch_active
    /// when AllDone is received.
    pub(in crate::witch) fn drain_messages(&mut self) -> Vec<SchedulerMessage> {
        let mut msgs = Vec::new();
        while let Ok(msg) = self.message_rx.try_recv() {
            if matches!(msg, SchedulerMessage::AllDone) {
                self.batch_active = false;
            }
            msgs.push(msg);
        }
        msgs
    }

    /// Send an outcome back to the scheduler for chain-emit decisions.
    pub(in crate::witch) fn send_outcome(&self, outcome: FetchOutcome) {
        let _ = self.outcome_tx.send(outcome);
    }

    /// Whether the scheduler is currently active.
    pub fn is_batch_active(&self) -> bool {
        self.batch_active
    }
}

impl crate::witch::types::ManagedThread for ExternalFetchHandle {
    fn send_shutdown(&self) {
        let _ = self.command_tx.send(FetchCommand::Shutdown);
    }

    fn take_handle(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.handle.take()
    }
}

impl Drop for ExternalFetchHandle {
    fn drop(&mut self) {
        use crate::witch::types::ManagedThread;
        self.shutdown();
    }
}
