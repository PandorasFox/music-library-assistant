//! FsWatcherHandle — Witch-side API for the filesystem watcher thread.
//!
//! The watcher thread owns filesystem monitoring and inode state tracking.
//! It performs initial directory walks at startup and persistent inotify-based
//! monitoring for steady-state change detection.
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
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::types::Zone;

// ============================================================================
// Public Types
// ============================================================================

/// A file observed on disk during initial scan or steady-state monitoring.
///
/// Carries FS-level metadata and optionally tags (from DB cache or disk read).
/// Tags are populated during initial scan when the DB cache provides a matching
/// mtime, or from disk reads for changed files during steady-state monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedFile {
    pub path: String, // relative to zone root
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
    /// Tags for audio files. None for non-audio files or when tags weren't read.
    pub tags: Option<crate::corpus::tags::TagSet>,
}

// Re-export from mm-meta
pub use mm_meta::computations::types::ObservedImage;

/// DB-cached state for one inode, seeded by the Witch at startup.
///
/// Watcher compares disk mtime against this cached mtime. If they match,
/// the DB's tags are current and can be carried through the initial scan
/// without reading tags from disk.
#[derive(Clone)]
pub(crate) struct CachedInodeState {
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub tags: crate::corpus::tags::TagSet,
}

/// Command from Witch to watcher thread.
pub(super) enum WatcherCommand {
    /// Begin watching zone roots. Triggers initial directory walk.
    Start {
        zones: Vec<(Zone, PathBuf)>,
        /// DB-cached state seeded by the Witch. Watcher skips tag reads
        /// for files whose disk mtime matches the cached mtime.
        db_cache: HashMap<i64, CachedInodeState>,
    },
    /// Immediate re-walk (polling mode only). Carries fresh DB cache
    /// and updated poll interval from config.
    Poll {
        db_cache: HashMap<i64, CachedInodeState>,
        poll_interval_secs: u64,
    },
    /// Shut down the watcher thread.
    Shutdown,
}

/// Message from watcher thread to Witch.
#[derive(Debug)]
pub(super) enum WatcherMessage {
    /// Initial scan complete for one zone. Full inode→ObservedFile map.
    InitialScanComplete {
        zone: Zone,
        inodes: HashMap<i64, ObservedFile>,
    },
    /// All zones' initial scans done.
    AllInitialScansComplete,
    /// File changed (mtime differs from watcher's cached state).
    /// Watcher has already read tags from the file.
    FileChanged {
        zone: Zone,
        inode: i64,
        path: PathBuf,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
        disk_tags: crate::corpus::tags::TagSet,
    },
    /// New file appeared (not in watcher's inode set).
    FileCreated {
        zone: Zone,
        inode: i64,
        path: PathBuf,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
    },
    /// File removed from disk.
    FileRemoved {
        zone: Zone,
        inode: i64,
        path: PathBuf,
    },
    /// Image file observed with extracted metadata (dimensions, format, role).
    /// Watcher has already read the image — Witch queues DB writes.
    ImageFileObserved(ObservedImage),
    /// inotify watches established — watcher is actively monitoring.
    /// Sent after AllInitialScansComplete, once inotify is fully set up.
    MonitoringActive,
    /// inotify watch limit exceeded or creation failed. Watcher has fallen
    /// back to periodic polling. Witch should clear observed state and
    /// note degraded mode — scan results arrive via InitialScanComplete.
    InotifyFailed,
}

// ============================================================================
// FsWatcherHandle
// ============================================================================

/// Handle held by the Witch for communicating with the watcher thread.
pub struct FsWatcherHandle {
    /// Send commands to watcher (start, shutdown).
    command_tx: Sender<WatcherCommand>,
    /// Receive messages from watcher (scan results + events).
    message_rx: Receiver<WatcherMessage>,
    /// Join handle for the watcher thread.
    handle: Option<JoinHandle<()>>,
}

impl FsWatcherHandle {
    /// Spawn the watcher thread.
    ///
    /// The watcher sleeps until it receives a Start command, then walks
    /// zone directories and sets up inotify watches for steady-state monitoring.
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
    pub fn start(&self, zones: Vec<(Zone, PathBuf)>, db_cache: HashMap<i64, CachedInodeState>) {
        let _ = self
            .command_tx
            .send(WatcherCommand::Start { zones, db_cache });
    }

    /// Request an immediate re-walk in polling mode with fresh DB cache
    /// and updated poll interval.
    pub fn poll(&self, db_cache: HashMap<i64, CachedInodeState>, poll_interval_secs: u64) {
        let _ = self
            .command_tx
            .send(WatcherCommand::Poll { db_cache, poll_interval_secs });
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
// Watcher Thread State
// ============================================================================

/// Per-file cached state held by the watcher.
#[derive(Debug, Clone)]
struct CachedFileState {
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: i64,
}

/// Watcher's in-memory state for one zone.
struct ZoneState {
    zone: Zone,
    root: PathBuf,
    /// inode → (relative_path, cached mtime/size)
    files: HashMap<i64, (String, CachedFileState)>,
    /// path → inode (reverse index for event lookup)
    path_to_inode: HashMap<PathBuf, i64>,
}

impl ZoneState {
    fn new(zone: Zone, root: PathBuf) -> Self {
        Self {
            zone,
            root,
            files: HashMap::new(),
            path_to_inode: HashMap::new(),
        }
    }

    /// Insert a file into the cached state.
    fn insert(&mut self, inode: i64, relative_path: String, state: CachedFileState) {
        let abs_path = self.root.join(&relative_path);
        self.path_to_inode.insert(abs_path, inode);
        self.files.insert(inode, (relative_path, state));
    }

    /// Remove a file by inode.
    fn remove_by_inode(&mut self, inode: i64) -> Option<String> {
        if let Some((path, _)) = self.files.remove(&inode) {
            let abs_path = self.root.join(&path);
            self.path_to_inode.remove(&abs_path);
            Some(path)
        } else {
            None
        }
    }

    /// Find the zone for an absolute path, returning the inode if it's tracked.
    fn inode_for_path(&self, path: &Path) -> Option<i64> {
        self.path_to_inode.get(path).copied()
    }

}

/// Debounce window for coalescing rapid FS events (e.g. editor write patterns).
const DEBOUNCE_DURATION: Duration = Duration::from_millis(200);

// ============================================================================
// Watcher Thread Main Loop
// ============================================================================

/// Watcher main loop. Waits for commands, performs directory walks,
/// then enters steady-state inotify monitoring.
fn run_watcher(command_rx: Receiver<WatcherCommand>, message_tx: Sender<WatcherMessage>) {
    crate::logging::log_general("[FS_WATCHER] Watcher thread started");

    while let Ok(command) = command_rx.recv() {
        match command {
            WatcherCommand::Shutdown => break,
            WatcherCommand::Start { zones, db_cache } => {
                // Run initial scan + steady-state monitoring.
                // Returns true if shutdown was requested during monitoring.
                if run_scan_and_monitor(&zones, &db_cache, &command_rx, &message_tx) {
                    break;
                }
            }
            WatcherCommand::Poll { .. } => {
                // Poll before Start is a no-op — we have no zones to walk.
            }
        }
    }

    crate::logging::log_general("[FS_WATCHER] Watcher thread exiting");
}

/// Perform initial scan, set up inotify, enter monitoring loop.
/// Returns true if shutdown requested.
fn run_scan_and_monitor(
    zones: &[(Zone, PathBuf)],
    db_cache: &HashMap<i64, CachedInodeState>,
    command_rx: &Receiver<WatcherCommand>,
    message_tx: &Sender<WatcherMessage>,
) -> bool {
    // Phase 1: Initial scan — walk and report
    let mut zone_states = Vec::new();

    for (zone, root) in zones {
        if !root.exists() {
            crate::logging::log_general(format!(
                "[FS_WATCHER] Zone {:?} root does not exist: {:?}, skipping",
                zone, root
            ));
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

        let mut zone_state = ZoneState::new(*zone, root.clone());
        let raw_inodes = walk_zone_root(root, db_cache);

        crate::logging::log_general(format!(
            "[FS_WATCHER] Zone {:?} initial scan complete: {} tracked files found",
            zone,
            raw_inodes.len()
        ));

        // Populate zone state and send image metadata for image files
        let mut image_count = 0;
        for (inode, observed) in &raw_inodes {
            zone_state.insert(
                *inode,
                observed.path.clone(),
                CachedFileState {
                    mtime_secs: observed.mtime_secs,
                    mtime_nanos: observed.mtime_nanos,
                    file_size: observed.file_size,
                },
            );

            // For image files, report to Witch (no content reads — just FS-level data)
            if crate::meta::computations::helpers::is_image_file_ext_from_path(&observed.path) {
                let _ = message_tx.send(WatcherMessage::ImageFileObserved(ObservedImage {
                    zone: *zone,
                    inode: *inode,
                    path: observed.path.clone(),
                    mtime_secs: observed.mtime_secs,
                    mtime_nanos: observed.mtime_nanos,
                    file_size: observed.file_size,
                }));
                image_count += 1;
            }
        }

        if image_count > 0 {
            crate::logging::log_general(format!(
                "[FS_WATCHER] Zone {:?}: {} image files observed with metadata",
                zone, image_count
            ));
        }

        let _ = message_tx.send(WatcherMessage::InitialScanComplete {
            zone: *zone,
            inodes: raw_inodes,
        });

        zone_states.push(zone_state);
    }

    let _ = message_tx.send(WatcherMessage::AllInitialScansComplete);

    // Phase 2: Set up inotify watcher and enter monitoring loop
    let (notify_tx, notify_rx) = std::sync::mpsc::channel();

    let mut watcher = match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        match res {
            Ok(event) => { let _ = notify_tx.send(NotifyEvent::Event(event)); }
            Err(e) => { let _ = notify_tx.send(NotifyEvent::Error(e)); }
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            crate::logging::log_error(format!(
                "[FS_WATCHER] Failed to create inotify watcher: {}. \
                 Falling back to polling mode.",
                e
            ));
            let _ = message_tx.send(WatcherMessage::InotifyFailed);
            return run_polling_loop(
                zones, db_cache.clone(),
                Duration::from_secs(900),
                command_rx, message_tx,
            );
        }
    };

    // Set up recursive watches on zone roots
    for zs in &zone_states {
        if let Err(e) = notify::Watcher::watch(&mut watcher, &zs.root, notify::RecursiveMode::Recursive) {
            crate::logging::log_error(format!(
                "[FS_WATCHER] Failed to watch {:?}: {}",
                zs.root, e
            ));
        } else {
            crate::logging::log_general(format!(
                "[FS_WATCHER] Watching zone {:?} at {:?}",
                zs.zone, zs.root
            ));
        }
    }

    // All inotify watches established — signal the Witch
    let _ = message_tx.send(WatcherMessage::MonitoringActive);

    // Monitoring loop: drain notify events + check for commands
    let mut debounce_map: HashMap<PathBuf, Instant> = HashMap::new();
    let mut pending_paths: Vec<PathBuf> = Vec::new();

    loop {
        // Check for commands (non-blocking)
        match command_rx.try_recv() {
            Ok(WatcherCommand::Shutdown) => return true,
            Ok(WatcherCommand::Start { zones, db_cache: rescan_cache }) => {
                // Re-scan requested: drop current watcher, re-run
                drop(watcher);

                // Re-scan zones
                zone_states.clear();
                for (zone, root) in &zones {
                    if !root.exists() {
                        let _ = message_tx.send(WatcherMessage::InitialScanComplete {
                            zone: *zone,
                            inodes: HashMap::new(),
                        });
                        continue;
                    }

                    let mut zone_state = ZoneState::new(*zone, root.clone());
                    let raw_inodes = walk_zone_root(root, &rescan_cache);

                    for (inode, observed) in &raw_inodes {
                        zone_state.insert(
                            *inode,
                            observed.path.clone(),
                            CachedFileState {
                                mtime_secs: observed.mtime_secs,
                                mtime_nanos: observed.mtime_nanos,
                                file_size: observed.file_size,
                            },
                        );
                    }

                    let _ = message_tx.send(WatcherMessage::InitialScanComplete {
                        zone: *zone,
                        inodes: raw_inodes,
                    });

                    zone_states.push(zone_state);
                }

                let _ = message_tx.send(WatcherMessage::AllInitialScansComplete);

                // Return false to re-enter command loop, which will re-run
                // run_scan_and_monitor with fresh inotify watches
                return false;
            }
            Ok(WatcherCommand::Poll { .. }) => {
                // Poll is for polling mode only; ignored in inotify mode.
            }
            Err(_) => {} // No command
        }

        // Drain notify events (non-blocking)
        let mut had_events = false;
        loop {
            match notify_rx.try_recv() {
                Ok(NotifyEvent::Event(event)) => {
                    had_events = true;
                    process_notify_event(
                        &event,
                        &mut zone_states,
                        &mut debounce_map,
                        &mut pending_paths,
                    );
                }
                Ok(NotifyEvent::Error(e)) => {
                    if handle_notify_error(e) {
                        let _ = message_tx.send(WatcherMessage::InotifyFailed);
                        drop(watcher);
                        // Default poll interval; Witch will send updated interval via Poll.
                        return run_polling_loop(
                            zones, db_cache.clone(),
                            Duration::from_secs(900),
                            command_rx, message_tx,
                        );
                    }
                }
                Err(_) => break,
            }
        }

        // Process debounced events that have settled
        let now = Instant::now();
        let mut i = 0;
        while i < pending_paths.len() {
            let path = &pending_paths[i];
            if let Some(last_event) = debounce_map.get(path) {
                if now.duration_since(*last_event) >= DEBOUNCE_DURATION {
                    let path = pending_paths.swap_remove(i);
                    debounce_map.remove(&path);
                    process_settled_event(&path, &mut zone_states, message_tx);
                    // Don't increment i — swap_remove moved the last element here
                    continue;
                }
            } else {
                // No debounce entry — remove from pending
                pending_paths.swap_remove(i);
                continue;
            }
            i += 1;
        }

        // Sleep briefly to avoid busy-spinning (50ms for responsive shutdown)
        if !had_events && pending_paths.is_empty() {
            thread::sleep(Duration::from_millis(50));
        } else if !pending_paths.is_empty() {
            // Events are pending debounce — sleep shorter
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Polling fallback when inotify is unavailable.
///
/// Periodically re-walks all zone roots and sends InitialScanComplete/
/// AllInitialScansComplete messages, mimicking a fresh scan each cycle.
/// No recursion, no stack growth.
///
/// Returns true if shutdown requested, false if a Start command arrived
/// (caller should retry inotify via the outer run_watcher loop).
fn run_polling_loop(
    zones: &[(Zone, PathBuf)],
    mut db_cache: HashMap<i64, CachedInodeState>,
    mut poll_interval: Duration,
    command_rx: &Receiver<WatcherCommand>,
    message_tx: &Sender<WatcherMessage>,
) -> bool {
    use std::sync::mpsc::RecvTimeoutError;

    crate::logging::log_general(format!(
        "[FS_WATCHER] Entering polling mode (interval: {}s)",
        poll_interval.as_secs()
    ));

    loop {
        match command_rx.recv_timeout(poll_interval) {
            Ok(WatcherCommand::Shutdown) => return true,
            Ok(WatcherCommand::Start { .. }) => {
                // Start = retry inotify. Return false to re-enter
                // run_watcher's outer loop → run_scan_and_monitor.
                // If inotify fails again, we'll re-enter polling.
                crate::logging::log_general(
                    "[FS_WATCHER] Start received in polling mode — retrying inotify"
                );
                return false;
            }
            Ok(WatcherCommand::Poll { db_cache: new_cache, poll_interval_secs }) => {
                // Operator-initiated rescan OR config change.
                // Update cache + interval, do immediate walk (fall through).
                db_cache = new_cache;
                poll_interval = Duration::from_secs(poll_interval_secs);
                crate::logging::log_general(format!(
                    "[FS_WATCHER] Poll command received — immediate re-walk (interval: {}s)",
                    poll_interval_secs
                ));
            }
            Err(RecvTimeoutError::Timeout) => { /* time to poll */ }
            Err(RecvTimeoutError::Disconnected) => return true,
        }

        // Re-walk all zones
        for (zone, root) in zones {
            let inodes = if root.exists() {
                walk_zone_root(root, &db_cache)
            } else {
                HashMap::new()
            };
            let _ = message_tx.send(WatcherMessage::InitialScanComplete {
                zone: *zone,
                inodes,
            });
        }
        let _ = message_tx.send(WatcherMessage::AllInitialScansComplete);
    }
}

/// Internal wrapper for notify events.
enum NotifyEvent {
    Event(notify::Event),
    Error(notify::Error),
}

// ============================================================================
// Event Processing
// ============================================================================

/// Record a notify event into the debounce map.
fn process_notify_event(
    event: &notify::Event,
    zone_states: &mut [ZoneState],
    debounce_map: &mut HashMap<PathBuf, Instant>,
    pending_paths: &mut Vec<PathBuf>,
) {
    use notify::EventKind;

    // Only care about file-level events that could affect audio files
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
        _ => return,
    }

    let now = Instant::now();
    for path in &event.paths {
        // Only process files in our zone roots
        let in_zone = zone_states.iter().any(|zs| path.starts_with(&zs.root));
        if !in_zone {
            continue;
        }

        // Skip directories (we watch recursively, so new dirs are auto-watched)
        if path.is_dir() {
            continue;
        }

        // Only care about tracked files (audio + images)
        if !is_tracked_file(path) {
            continue;
        }

        let is_new = !debounce_map.contains_key(path);
        debounce_map.insert(path.clone(), now);
        if is_new {
            pending_paths.push(path.clone());
        }
    }
}

/// Process a settled (debounced) event for a single path.
fn process_settled_event(
    path: &Path,
    zone_states: &mut [ZoneState],
    message_tx: &Sender<WatcherMessage>,
) {
    // Find which zone this path belongs to
    let zone_idx = match zone_states.iter().position(|zs| path.starts_with(&zs.root)) {
        Some(i) => i,
        None => return,
    };

    let zone = zone_states[zone_idx].zone;
    let existing_inode = zone_states[zone_idx].inode_for_path(path);

    // Stat the file to see its current state
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let inode = metadata.ino() as i64;
            let (mtime_secs, mtime_nanos) = crate::corpus::paths::read_mtime(&metadata);
            let file_size = metadata.len() as i64;

            if let Some(existing) = existing_inode {
                if existing == inode {
                    // Same inode at same path — check if mtime/size changed
                    let cached = &zone_states[zone_idx].files[&inode].1;
                    if cached.mtime_secs == mtime_secs
                        && cached.mtime_nanos == mtime_nanos
                        && cached.file_size == file_size
                    {
                        // No actual change (e.g. editor open/close without save)
                        return;
                    }

                    // Mtime changed — read tags (audio files only) and report
                    let tags = if is_audio_file(path) {
                        read_tags(path)
                    } else {
                        crate::corpus::tags::TagSet::new(std::iter::empty())
                    };

                    // Update cached state
                    let entry = zone_states[zone_idx].files.get_mut(&inode).unwrap();
                    entry.1 = CachedFileState {
                        mtime_secs,
                        mtime_nanos,
                        file_size,
                    };

                    let _ = message_tx.send(WatcherMessage::FileChanged {
                        zone,
                        inode,
                        path: path.to_path_buf(),
                        mtime_secs,
                        mtime_nanos,
                        file_size,
                        disk_tags: tags,
                    });

                    // For images: also send updated metadata
                    maybe_send_image_observed(path, zone, inode, mtime_secs, mtime_nanos, file_size, &zone_states[zone_idx].root, message_tx);
                } else {
                    // Different inode at same path — file was replaced
                    // Remove old inode, add new one
                    zone_states[zone_idx].remove_by_inode(existing);

                    let rel_path = path
                        .strip_prefix(&zone_states[zone_idx].root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string();

                    // Report old inode as removed
                    let _ = message_tx.send(WatcherMessage::FileRemoved {
                        zone,
                        inode: existing,
                        path: path.to_path_buf(),
                    });

                    // Report new inode as created
                    zone_states[zone_idx].insert(
                        inode,
                        rel_path,
                        CachedFileState {
                            mtime_secs,
                            mtime_nanos,
                            file_size,
                        },
                    );

                    let _ = message_tx.send(WatcherMessage::FileCreated {
                        zone,
                        inode,
                        path: path.to_path_buf(),
                        mtime_secs,
                        mtime_nanos,
                        file_size,
                    });
                    maybe_send_image_observed(path, zone, inode, mtime_secs, mtime_nanos, file_size, &zone_states[zone_idx].root, message_tx);
                }
            } else {
                // New file (not in our cached state)
                let rel_path = path
                    .strip_prefix(&zone_states[zone_idx].root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                zone_states[zone_idx].insert(
                    inode,
                    rel_path,
                    CachedFileState {
                        mtime_secs,
                        mtime_nanos,
                        file_size,
                    },
                );

                let _ = message_tx.send(WatcherMessage::FileCreated {
                    zone,
                    inode,
                    path: path.to_path_buf(),
                    mtime_secs,
                    mtime_nanos,
                    file_size,
                });
                maybe_send_image_observed(path, zone, inode, mtime_secs, mtime_nanos, file_size, &zone_states[zone_idx].root, message_tx);
            }
        }
        Err(_) => {
            // File doesn't exist anymore — it was removed
            if let Some(inode) = existing_inode {
                zone_states[zone_idx].remove_by_inode(inode);

                let _ = message_tx.send(WatcherMessage::FileRemoved {
                    zone,
                    inode,
                    path: path.to_path_buf(),
                });
            }
            // If we didn't know about it, ignore
        }
    }
}

/// Handle a notify error (inotify overflow, watch limit, etc.)
///
/// Returns `true` if the caller should fall back to polling mode
/// (watch limit exhaustion — inotify is no longer viable).
fn handle_notify_error(error: notify::Error) -> bool {
    crate::logging::log_error(format!(
        "[FS_WATCHER] Notify error: {}",
        error
    ));

    // Only rescan on watch limit exhaustion — the one error that means
    // we're definitively missing events. Generic errors are logged but not
    // worth a full re-walk (which could itself trigger more errors).
    if matches!(error.kind, notify::ErrorKind::MaxFilesWatch) {
        crate::logging::log_general(
            "[FS_WATCHER] inotify overflow/limit — requesting rescan from Witch"
        );
        return true;
    }

    false
}

/// Read tags from an audio file. Returns empty vec on error.
fn read_tags(path: &Path) -> crate::corpus::tags::TagSet {
    match crate::corpus::tags::from_file(path) {
        Ok(tagset) => tagset,
        Err(e) => {
            crate::logging::log_error(format!(
                "[FS_WATCHER] Failed to read tags from {:?}: {}",
                path, e
            ));
            crate::corpus::tags::TagSet::new(std::iter::empty())
        }
    }
}

/// If `path` is an image file, send an `ImageFileObserved` message.
#[allow(clippy::too_many_arguments)]
fn maybe_send_image_observed(
    path: &Path,
    zone: Zone,
    inode: i64,
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: i64,
    zone_root: &Path,
    message_tx: &Sender<WatcherMessage>,
) {
    if !crate::meta::computations::helpers::is_image_file(path) {
        return;
    }

    let rel_path = path
        .strip_prefix(zone_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();

    let _ = message_tx.send(WatcherMessage::ImageFileObserved(ObservedImage {
        zone,
        inode,
        path: rel_path,
        mtime_secs,
        mtime_nanos,
        file_size,
    }));
}

// ============================================================================
// Directory Walking
// ============================================================================

/// Walk a zone root and collect all tracked files (audio + images) with metadata.
///
/// For audio files, checks `db_cache` for matching mtime — if matched, carries
/// the cached tags through without a disk read. Otherwise tags are left as None
/// (they'll be read on first change via steady-state monitoring).
fn walk_zone_root(root: &Path, db_cache: &HashMap<i64, CachedInodeState>) -> HashMap<i64, ObservedFile> {
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
        collect_tracked_files(dir, root, db_cache, &mut result);
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
                        crate::logging::log_error(format!(
                            "[WATCHER] Nested mount point detected at {:?} \
                             (expected device {}, found {}). Skipping.",
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

/// Collect tracked files (audio + images) from a single directory into the result map.
///
/// Paths are stored relative to `root`. For audio files, checks `db_cache` for
/// matching mtime and uses cached tags if available.
fn collect_tracked_files(
    dir: &Path,
    root: &Path,
    db_cache: &HashMap<i64, CachedInodeState>,
    result: &mut HashMap<i64, ObservedFile>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() || path.is_symlink() {
            continue;
        }

        if is_tracked_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                let (mtime_secs, mtime_nanos) = crate::corpus::paths::read_mtime(&metadata);
                let file_size = metadata.len() as i64;

                // For audio files, try to use DB-cached tags if mtime matches
                let tags = if is_audio_file(&path) {
                    if let Some(cached) = db_cache.get(&inode) {
                        if cached.mtime_secs == mtime_secs && cached.mtime_nanos == mtime_nanos {
                            Some(cached.tags.clone())
                        } else {
                            // Mtime differs — tags might be stale, leave as None.
                            // Derivation will schedule verification for mismatched files.
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Store path relative to zone root (for internal abs_path reconstruction)
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();

                result.insert(inode, ObservedFile {
                    path: relative,
                    mtime_secs,
                    mtime_nanos,
                    file_size,
                    tags,
                });
            }
        }
    }
}

/// Check if a file has an audio extension.
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(crate::config::is_audio_extension)
        .unwrap_or(false)
}

/// Check if a file is one we track: audio files or image files.
///
/// The watcher observes everything indexed in the `files` table.
/// Audio files are the primary corpus content; image files (cover art, etc.)
/// are also indexed and must be in the observed inode set or derivation
/// will emit spurious MissingFileSignals for them.
fn is_tracked_file(path: &Path) -> bool {
    // Skip macOS resource fork files (._filename.ext)
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with("._") {
            return false;
        }
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            crate::config::is_audio_extension(ext)
                || crate::meta::computations::helpers::is_image_file_ext(ext)
        })
        .unwrap_or(false)
}
