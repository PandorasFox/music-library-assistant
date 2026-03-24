//! ExternalFetchHandle -- Witch-side API for the scheduler thread.

use std::thread::{self, JoinHandle};

use tokio::sync::mpsc::{self, UnboundedSender};

use crate::config::SharedConfig;

use super::types::{FetchCommand, SchedulerMessage};

/// Handle held by the Witch for communicating with the scheduler thread.
pub struct ExternalFetchHandle {
    /// Send commands to scheduler (start, shutdown).
    command_tx: UnboundedSender<FetchCommand>,
    /// Join handle for the scheduler thread.
    handle: Option<JoinHandle<()>>,
    /// Whether the metadata fetch scheduler is currently active.
    batch_active: bool,
    /// Whether a cover art fetch is currently active.
    cover_art_active: bool,
}

impl ExternalFetchHandle {
    /// Spawn the scheduler thread.
    ///
    /// The scheduler runs a single-threaded tokio runtime with an event-driven
    /// select! loop. HTTP calls are async, dispatched via JoinSet.
    pub(in crate::witch) fn spawn(shared_config: SharedConfig) -> (Self, tokio::sync::mpsc::UnboundedReceiver<SchedulerMessage>) {
        // Witch -> Scheduler: commands (tokio unbounded — send is sync)
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        // Scheduler -> Witch: progress + status
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        let handle = thread::spawn(move || {
            super::run_scheduler(command_rx, message_tx, shared_config);
        });

        (Self {
            command_tx,
            handle: Some(handle),
            batch_active: false,
            cover_art_active: false,
        }, message_rx)
    }

    /// Request an external metadata fetch.
    ///
    /// The scheduler reads config to determine eligible dirs and populates
    /// both the AcoustID queue (fingerprint lookups) and the MB queue
    /// (recording/artist/release enrichment).
    pub fn request_fetch(&mut self) {
        if self.batch_active {
            return; // Don't stack requests
        }
        self.batch_active = true;
        let _ = self.command_tx.send(FetchCommand::Start);
    }

    /// Mark the current batch as done (called by Witch when it sees AllDone).
    pub(in crate::witch) fn mark_batch_done(&mut self) {
        self.batch_active = false;
    }

    /// Whether the metadata fetch scheduler is currently active.
    pub fn is_batch_active(&self) -> bool {
        self.batch_active
    }

    /// Request a cover art fetch from the Cover Art Archive.
    pub fn request_cover_art(&mut self) {
        if self.cover_art_active {
            return;
        }
        self.cover_art_active = true;
        let _ = self.command_tx.send(FetchCommand::StartCoverArt);
    }

    /// Mark the cover art fetch as done.
    pub(in crate::witch) fn mark_cover_art_done(&mut self) {
        self.cover_art_active = false;
    }

    /// Whether a cover art fetch is currently active.
    pub fn is_cover_art_active(&self) -> bool {
        self.cover_art_active
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
