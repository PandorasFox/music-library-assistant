//! FsWatcherHandle — Witch-side API for the filesystem watcher thread.
//!
//! The watcher thread owns filesystem monitoring and inode state tracking.
//! It performs initial directory walks at startup and (in future phases)
//! persistent inotify-based monitoring for steady-state change detection.
//!
//! ## Channel Topology
//!
//! ```text
//!                    WatcherMessage
//!   Watcher ─────────────────────► Witch (drains in tick())
//!       ▲
//!       │          WatcherCommand
//!       └──────────────────────── Witch (start, shutdown)
//! ```
//!
//! The watcher has NO database access. It reports raw filesystem state;
//! the Witch queues computations for semantic comparison against DB state.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::db::types::Zone;

// ============================================================================
// Public Types
// ============================================================================

/// Command from Witch to watcher thread.
pub(super) enum WatcherCommand {
    /// Begin watching zone roots. Triggers initial directory walk.
    Start {
        zones: Vec<(Zone, PathBuf)>,
        force_check: bool,
    },
    /// Shut down the watcher thread.
    Shutdown,
}

/// Message from watcher thread to Witch.
#[derive(Debug)]
pub(super) enum WatcherMessage {
    /// Initial scan complete for one zone. Full inode→(path, mtime_s, mtime_ns, size) map.
    InitialScanComplete {
        zone: Zone,
        inodes: HashMap<i64, (String, i64, i64, i64)>,
    },
    /// All zones' initial scans done.
    AllInitialScansComplete,
}

// ============================================================================
// FsWatcherHandle
// ============================================================================

/// Handle held by the Witch for communicating with the watcher thread.
pub struct FsWatcherHandle {
    /// Send commands to watcher (start, shutdown).
    command_tx: Sender<WatcherCommand>,
    /// Receive messages from watcher (scan results).
    message_rx: Receiver<WatcherMessage>,
    /// Join handle for the watcher thread.
    handle: Option<JoinHandle<()>>,
}

impl FsWatcherHandle {
    /// Spawn the watcher thread.
    ///
    /// The watcher sleeps until it receives a Start command, then walks
    /// zone directories and reports inode maps back to the Witch.
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (message_tx, message_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            run_watcher(command_rx, message_tx);
        });

        Self {
            command_tx,
            message_rx,
            handle: Some(handle),
        }
    }

    /// Request the watcher to start scanning zone roots.
    pub fn start(&self, zones: Vec<(Zone, PathBuf)>, force_check: bool) {
        let _ = self
            .command_tx
            .send(WatcherCommand::Start { zones, force_check });
    }

    /// Drain available messages from the watcher (non-blocking).
    pub(super) fn drain_messages(&self) -> Vec<WatcherMessage> {
        let mut msgs = Vec::new();
        while let Ok(msg) = self.message_rx.try_recv() {
            msgs.push(msg);
        }
        msgs
    }
}

impl super::types::ManagedThread for FsWatcherHandle {
    fn send_shutdown(&self) {
        let _ = self.command_tx.send(WatcherCommand::Shutdown);
    }

    fn take_handle(&mut self) -> Option<JoinHandle<()>> {
        self.handle.take()
    }
}

impl Drop for FsWatcherHandle {
    fn drop(&mut self) {
        use super::types::ManagedThread;
        self.shutdown();
    }
}

// ============================================================================
// Watcher Thread Main Loop
// ============================================================================

/// Watcher main loop. Waits for commands, performs directory walks.
fn run_watcher(command_rx: Receiver<WatcherCommand>, message_tx: Sender<WatcherMessage>) {
    crate::logging::log_general("[FS_WATCHER] Watcher thread started");

    while let Ok(command) = command_rx.recv() {
        match command {
            WatcherCommand::Shutdown => break,
            WatcherCommand::Start {
                zones,
                force_check,
            } => {
                run_initial_scan(&zones, force_check, &message_tx);
            }
        }
    }

    crate::logging::log_general("[FS_WATCHER] Watcher thread exiting");
}

/// Perform the initial directory scan for all zones.
///
/// Walks each zone root, collects audio files with inode/mtime/size,
/// and sends per-zone `InitialScanComplete` messages followed by
/// `AllInitialScansComplete`.
fn run_initial_scan(
    zones: &[(Zone, PathBuf)],
    _force_check: bool,
    message_tx: &Sender<WatcherMessage>,
) {
    for (zone, root) in zones {
        if !root.exists() {
            crate::logging::log_general(format!(
                "[FS_WATCHER] Zone {:?} root does not exist: {:?}, skipping",
                zone, root
            ));
            // Send empty scan result so the Witch knows this zone was processed
            let _ = message_tx.send(WatcherMessage::InitialScanComplete {
                zone: *zone,
                inodes: HashMap::new(),
            });
            continue;
        }

        crate::logging::log_general(format!(
            "[FS_WATCHER] Starting initial scan for zone {:?} at {:?}",
            zone, root
        ));

        let inodes = walk_zone_root(root);

        crate::logging::log_general(format!(
            "[FS_WATCHER] Zone {:?} initial scan complete: {} audio files found",
            zone,
            inodes.len()
        ));

        let _ = message_tx.send(WatcherMessage::InitialScanComplete {
            zone: *zone,
            inodes,
        });
    }

    let _ = message_tx.send(WatcherMessage::AllInitialScansComplete);
}

// ============================================================================
// Directory Walking (reuses observation helpers' logic)
// ============================================================================

/// Walk a zone root and collect all audio files with metadata.
///
/// Returns inode → (relative_path, mtime_secs, mtime_nanos, file_size).
fn walk_zone_root(root: &Path) -> HashMap<i64, (String, i64, i64, i64)> {
    let mut result = HashMap::new();

    // Enumerate all directories (including root itself)
    let (directories, symlink_count) = enumerate_directories(root);

    if symlink_count > 0 {
        crate::logging::log_general(format!(
            "[FS_WATCHER] Skipped {} directory symlinks in {:?}",
            symlink_count, root
        ));
    }

    // Collect audio files from each directory
    for dir in &directories {
        collect_audio_files(dir, root, &mut result);
    }

    result
}

/// Enumerate all directories under root, including root itself.
///
/// Returns (directories, symlink_count). Checks mount boundaries.
fn enumerate_directories(root: &Path) -> (Vec<PathBuf>, usize) {
    let mut directories = Vec::new();
    let mut symlink_count = 0;
    enumerate_directories_recursive(root, &mut directories, &mut symlink_count);
    directories.push(root.to_path_buf());
    (directories, symlink_count)
}

/// Recursively enumerate directories, checking mount boundaries.
fn enumerate_directories_recursive(
    dir: &Path,
    directories: &mut Vec<PathBuf>,
    symlink_count: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let expected_dev = crate::corpus::paths::get_expected_device_id();

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_symlink() {
            if path.is_dir() {
                *symlink_count += 1;
            }
            continue;
        }

        if path.is_dir() {
            // Check for mount boundary violation
            if let Some(expected) = expected_dev {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let actual_dev = metadata.dev();
                    if actual_dev != expected {
                        crate::witch::report_mount_violation(format!(
                            "Nested mount point detected at {:?}\n\
                             Expected device: {}, found device: {}\n\
                             \n\
                             The corpus and libraries must not contain nested mount points.\n\
                             Please unmount the nested filesystem and restart MM.",
                            path, expected, actual_dev
                        ));
                        continue;
                    }
                }
            }

            directories.push(path.clone());
            enumerate_directories_recursive(&path, directories, symlink_count);
        }
    }
}

/// Collect audio files from a single directory into the result map.
///
/// Paths are stored relative to `root`.
fn collect_audio_files(dir: &Path, root: &Path, result: &mut HashMap<i64, (String, i64, i64, i64)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() || path.is_symlink() {
            continue;
        }

        if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                let (mtime_secs, mtime_nanos) = crate::corpus::paths::read_mtime(&metadata);
                let file_size = metadata.len() as i64;

                // Store path relative to zone root
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();

                result.insert(inode, (relative, mtime_secs, mtime_nanos, file_size));
            }
        }
    }
}

/// Check if a file has an audio extension (delegates to canonical list in mm-utils).
fn is_audio_file(path: &Path) -> bool {
    // Skip macOS resource fork files (._filename.ext)
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with("._") {
            return false;
        }
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| crate::config::is_audio_extension(ext))
        .unwrap_or(false)
}
