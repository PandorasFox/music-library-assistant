//! FsThreadHandle — Witch-side API for the filesystem observer thread.
//!
//! The fs-thread owns filesystem monitoring and inode state tracking.
//! It runs its own single-threaded tokio runtime, performing initial directory
//! walks at startup and persistent inotify-based monitoring for steady-state
//! change detection.
//!
//! ## Channel Topology
//!
//! ```text
//!                    WatcherMessage
//!   fs-thread ─────────────────────► Witch (drains in select!)
//!       ▲
//!       │          WatcherCommand
//!       └──────────────────────── Witch (start, shutdown)
//! ```
//!
//! The fs-thread has NO database access. It reports raw filesystem state;
//! the Witch queues computations for semantic comparison against DB state.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;

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

/// A zone to watch, with optional subdirectory filter.
///
/// When `allowed_subdirs` is set, only those top-level subdirectories of `root`
/// are walked. Used for the Library zone to restrict scanning to configured
/// deployment directories (e.g., "music", "soundtracks") rather than walking
/// everything under the library root.
pub(crate) struct WatchedZone {
    pub zone: Zone,
    pub root: PathBuf,
    pub allowed_subdirs: Option<Vec<String>>,
}

/// Command from Witch to fs-thread.
pub(super) enum WatcherCommand {
    /// Begin watching zone roots. Triggers initial directory walk.
    Start {
        zones: Vec<WatchedZone>,
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
    /// Shut down the fs-thread.
    Shutdown,
}

/// Message from fs-thread to Witch.
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
// FsThreadHandle
// ============================================================================

/// Handle held by the Witch for communicating with the fs-thread.
pub struct FsThreadHandle {
    /// Send commands to fs-thread (start, shutdown).
    command_tx: mpsc::UnboundedSender<WatcherCommand>,
    /// Receive messages from fs-thread (scan results + events).
    pub(super) message_rx: mpsc::UnboundedReceiver<WatcherMessage>,
    /// Join handle for the fs-thread.
    handle: Option<JoinHandle<()>>,
}

impl FsThreadHandle {
    /// Spawn the fs-thread with its own single-threaded tokio runtime.
    ///
    /// The thread blocks on its runtime until it receives a Start command,
    /// then walks zone directories and sets up inotify watches for
    /// steady-state monitoring.
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("fs-thread: failed to create tokio runtime");
            rt.block_on(run_fs_thread(command_rx, message_tx));
        });

        Self {
            command_tx,
            message_rx,
            handle: Some(handle),
        }
    }

    /// Request the fs-thread to start scanning zone roots.
    pub fn start(&self, zones: Vec<WatchedZone>, db_cache: HashMap<i64, CachedInodeState>) {
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

}

impl super::types::ManagedThread for FsThreadHandle {
    fn send_shutdown(&self) {
        let _ = self.command_tx.send(WatcherCommand::Shutdown);
    }

    fn take_handle(&mut self) -> Option<JoinHandle<()>> {
        self.handle.take()
    }
}

impl Drop for FsThreadHandle {
    fn drop(&mut self) {
        use super::types::ManagedThread;
        self.shutdown();
    }
}

// ============================================================================
// fs-thread Internal State
// ============================================================================

/// Per-file cached state held by the fs-thread.
#[derive(Debug, Clone)]
struct CachedFileState {
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: i64,
}

/// fs-thread's in-memory state for one zone.
struct ZoneState {
    zone: Zone,
    root: PathBuf,
    /// inode → (relative_path, cached mtime/size)
    files: HashMap<i64, (String, CachedFileState)>,
    /// path → inode (reverse index for event lookup)
    path_to_inode: HashMap<PathBuf, i64>,
    /// When set, only paths under these top-level subdirs of `root` are accepted.
    /// Used for Library zone to ignore events from non-deployment directories.
    allowed_subdirs: Option<Vec<String>>,
}

impl ZoneState {
    fn new(zone: Zone, root: PathBuf, allowed_subdirs: Option<Vec<String>>) -> Self {
        Self {
            zone,
            root,
            files: HashMap::new(),
            path_to_inode: HashMap::new(),
            allowed_subdirs,
        }
    }

    /// Insert a file into the cached state.
    ///
    /// If the inode already exists at a different path, the old path→inode
    /// mapping is cleaned up (directory renames move inodes to new paths
    /// without explicit per-file remove events).
    fn insert(&mut self, inode: i64, relative_path: String, state: CachedFileState) {
        // Clean up stale path_to_inode entry if this inode was previously at a different path.
        if let Some((old_rel, _)) = self.files.get(&inode) {
            if *old_rel != relative_path {
                let old_abs = self.root.join(&*old_rel);
                self.path_to_inode.remove(&old_abs);
            }
        }
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

    /// Check if an absolute path is within this zone's allowed scope.
    /// Returns true if no subdirectory filter is set, or if the path falls
    /// under one of the allowed top-level subdirectories.
    fn accepts_path(&self, path: &Path) -> bool {
        let Some(ref subdirs) = self.allowed_subdirs else {
            return path.starts_with(&self.root);
        };
        // path must be under root/{allowed_subdir}/...
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        let Some(first_component) = rel.components().next() else {
            return false;
        };
        let first = first_component.as_os_str().to_string_lossy();
        subdirs.iter().any(|s| s == first.as_ref())
    }
}

/// Debounce window for coalescing rapid FS events (e.g. editor write patterns).
const DEBOUNCE_DURATION: Duration = Duration::from_millis(200);

// ============================================================================
// fs-thread Main Loop
// ============================================================================

/// Async entry point for the fs-thread. Waits for commands, performs directory
/// walks, then enters steady-state inotify monitoring.
async fn run_fs_thread(
    mut command_rx: mpsc::UnboundedReceiver<WatcherCommand>,
    message_tx: mpsc::UnboundedSender<WatcherMessage>,
) {
    crate::logging::log_general("[FS_THREAD] Thread started");

    while let Some(command) = command_rx.recv().await {
        match command {
            WatcherCommand::Shutdown => break,
            WatcherCommand::Start { zones, db_cache } => {
                // Run initial scan + steady-state monitoring.
                // Returns true if shutdown was requested during monitoring.
                if run_scan_and_monitor(&zones, &db_cache, &mut command_rx, &message_tx).await {
                    break;
                }
            }
            WatcherCommand::Poll { .. } => {
                // Poll before Start is a no-op — we have no zones to walk.
            }
        }
    }

    crate::logging::log_general("[FS_THREAD] Thread exiting");
}

/// Perform initial scan, set up inotify, enter async monitoring loop.
/// Returns true if shutdown requested.
async fn run_scan_and_monitor(
    zones: &[WatchedZone],
    db_cache: &HashMap<i64, CachedInodeState>,
    command_rx: &mut mpsc::UnboundedReceiver<WatcherCommand>,
    message_tx: &mpsc::UnboundedSender<WatcherMessage>,
) -> bool {
    // Phase 1: Initial scan — walk and report (blocking I/O, fine on dedicated thread)
    let mut zone_states = Vec::new();

    for wz in zones {
        if !wz.root.exists() {
            crate::logging::log_general(format!(
                "[FS_THREAD] Zone {:?} root does not exist: {:?}, skipping",
                wz.zone, wz.root
            ));
            let _ = message_tx.send(WatcherMessage::InitialScanComplete {
                zone: wz.zone,
                inodes: HashMap::new(),
            });
            continue;
        }

        crate::logging::log_general(format!(
            "[FS_THREAD] Starting initial scan for zone {:?} at {:?}",
            wz.zone, wz.root
        ));

        let mut zone_state = ZoneState::new(wz.zone, wz.root.clone(), wz.allowed_subdirs.clone());
        let raw_inodes = walk_zone_root(&wz.root, db_cache, wz.allowed_subdirs.as_deref());

        crate::logging::log_general(format!(
            "[FS_THREAD] Zone {:?} initial scan complete: {} tracked files found",
            wz.zone,
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
                    zone: wz.zone,
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
                "[FS_THREAD] Zone {:?}: {} image files observed with metadata",
                wz.zone, image_count
            ));
        }

        let _ = message_tx.send(WatcherMessage::InitialScanComplete {
            zone: wz.zone,
            inodes: raw_inodes,
        });

        zone_states.push(zone_state);
    }

    let _ = message_tx.send(WatcherMessage::AllInitialScansComplete);

    // Phase 2: Set up inotify watcher, bridging events into a tokio channel
    let (notify_tx, mut notify_rx) = mpsc::unbounded_channel();

    let mut watcher = match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        match res {
            Ok(event) => { let _ = notify_tx.send(NotifyEvent::Event(event)); }
            Err(e) => { let _ = notify_tx.send(NotifyEvent::Error(e)); }
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            crate::logging::log_error(format!(
                "[FS_THREAD] Failed to create inotify watcher: {}. \
                 Falling back to polling mode.",
                e
            ));
            let _ = message_tx.send(WatcherMessage::InotifyFailed);
            return run_polling_loop(
                zones, db_cache.clone(),
                Duration::from_secs(900),
                command_rx, message_tx,
            ).await;
        }
    };

    // Set up recursive watches on zone roots
    for zs in &zone_states {
        if let Err(e) = notify::Watcher::watch(&mut watcher, &zs.root, notify::RecursiveMode::Recursive) {
            crate::logging::log_error(format!(
                "[FS_THREAD] Failed to watch {:?}: {}",
                zs.root, e
            ));
        } else {
            crate::logging::log_general(format!(
                "[FS_THREAD] Watching zone {:?} at {:?}",
                zs.zone, zs.root
            ));
        }
    }

    // All inotify watches established — signal the Witch
    let _ = message_tx.send(WatcherMessage::MonitoringActive);

    // Monitoring loop: select! on commands, notify events, and debounce deadlines
    let mut debounce_map: HashMap<PathBuf, Instant> = HashMap::new();
    let mut pending_paths: Vec<PathBuf> = Vec::new();

    loop {
        // Compute next debounce deadline (earliest pending path + DEBOUNCE_DURATION)
        let deadline = next_debounce_deadline(&debounce_map);

        tokio::select! {
            cmd = command_rx.recv() => {
                match cmd {
                    Some(WatcherCommand::Shutdown) | None => return true,
                    Some(WatcherCommand::Start { zones, db_cache: rescan_cache }) => {
                        // Re-scan requested: drop current watcher, re-run
                        drop(watcher);

                        // Re-scan zones
                        zone_states.clear();
                        for wz in &zones {
                            if !wz.root.exists() {
                                let _ = message_tx.send(WatcherMessage::InitialScanComplete {
                                    zone: wz.zone,
                                    inodes: HashMap::new(),
                                });
                                continue;
                            }

                            let mut zone_state = ZoneState::new(wz.zone, wz.root.clone(), wz.allowed_subdirs.clone());
                            let raw_inodes = walk_zone_root(&wz.root, &rescan_cache, wz.allowed_subdirs.as_deref());

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
                                zone: wz.zone,
                                inodes: raw_inodes,
                            });

                            zone_states.push(zone_state);
                        }

                        let _ = message_tx.send(WatcherMessage::AllInitialScansComplete);

                        // Return false to re-enter command loop, which will re-run
                        // run_scan_and_monitor with fresh inotify watches
                        return false;
                    }
                    Some(WatcherCommand::Poll { .. }) => {
                        // Poll is for polling mode only; ignored in inotify mode.
                    }
                }
            }

            event = notify_rx.recv() => {
                match event {
                    Some(NotifyEvent::Event(e)) => {
                        process_notify_event(
                            &e, &mut zone_states, &mut debounce_map, &mut pending_paths,
                        );
                        // Drain any buffered events
                        while let Ok(ev) = notify_rx.try_recv() {
                            match ev {
                                NotifyEvent::Event(e) => {
                                    process_notify_event(
                                        &e, &mut zone_states, &mut debounce_map, &mut pending_paths,
                                    );
                                }
                                NotifyEvent::Error(e) => {
                                    if handle_notify_error(e) {
                                        let _ = message_tx.send(WatcherMessage::InotifyFailed);
                                        drop(watcher);
                                        return run_polling_loop(
                                            zones, db_cache.clone(),
                                            Duration::from_secs(900),
                                            command_rx, message_tx,
                                        ).await;
                                    }
                                }
                            }
                        }
                    }
                    Some(NotifyEvent::Error(e)) => {
                        if handle_notify_error(e) {
                            let _ = message_tx.send(WatcherMessage::InotifyFailed);
                            drop(watcher);
                            return run_polling_loop(
                                zones, db_cache.clone(),
                                Duration::from_secs(900),
                                command_rx, message_tx,
                            ).await;
                        }
                    }
                    None => {
                        // Notify channel closed — watcher was dropped externally
                        return true;
                    }
                }
            }

            _ = tokio::time::sleep_until(deadline), if !pending_paths.is_empty() => {
                drain_settled_events(
                    &mut debounce_map, &mut pending_paths, &mut zone_states, message_tx,
                );
            }
        }
    }
}

/// Compute the earliest debounce deadline from the pending set.
///
/// Returns a far-future instant if the map is empty (the caller guards
/// the sleep arm with `if !pending_paths.is_empty()`).
fn next_debounce_deadline(debounce_map: &HashMap<PathBuf, Instant>) -> Instant {
    debounce_map
        .values()
        .map(|t| *t + DEBOUNCE_DURATION)
        .min()
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400))
}

/// Process all debounced events whose settle window has elapsed.
fn drain_settled_events(
    debounce_map: &mut HashMap<PathBuf, Instant>,
    pending_paths: &mut Vec<PathBuf>,
    zone_states: &mut [ZoneState],
    message_tx: &mpsc::UnboundedSender<WatcherMessage>,
) {
    let now = Instant::now();
    let mut i = 0;
    while i < pending_paths.len() {
        let path = &pending_paths[i];
        if let Some(last_event) = debounce_map.get(path) {
            if now.duration_since(*last_event) >= DEBOUNCE_DURATION {
                let path = pending_paths.swap_remove(i);
                debounce_map.remove(&path);
                process_settled_event(&path, zone_states, message_tx);
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
}

/// Polling fallback when inotify is unavailable.
///
/// Periodically re-walks all zone roots and sends InitialScanComplete/
/// AllInitialScansComplete messages, mimicking a fresh scan each cycle.
///
/// Returns true if shutdown requested, false if a Start command arrived
/// (caller should retry inotify via the outer run_fs_thread loop).
async fn run_polling_loop(
    zones: &[WatchedZone],
    mut db_cache: HashMap<i64, CachedInodeState>,
    mut poll_interval: Duration,
    command_rx: &mut mpsc::UnboundedReceiver<WatcherCommand>,
    message_tx: &mpsc::UnboundedSender<WatcherMessage>,
) -> bool {
    crate::logging::log_general(format!(
        "[FS_THREAD] Entering polling mode (interval: {}s)",
        poll_interval.as_secs()
    ));

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                match cmd {
                    Some(WatcherCommand::Shutdown) | None => return true,
                    Some(WatcherCommand::Start { .. }) => {
                        // Start = retry inotify. Return false to re-enter
                        // run_fs_thread's outer loop → run_scan_and_monitor.
                        crate::logging::log_general(
                            "[FS_THREAD] Start received in polling mode — retrying inotify"
                        );
                        return false;
                    }
                    Some(WatcherCommand::Poll { db_cache: new_cache, poll_interval_secs }) => {
                        // Operator-initiated rescan OR config change.
                        // Update cache + interval, fall through to re-walk.
                        db_cache = new_cache;
                        poll_interval = Duration::from_secs(poll_interval_secs);
                        crate::logging::log_general(format!(
                            "[FS_THREAD] Poll command received — immediate re-walk (interval: {}s)",
                            poll_interval_secs
                        ));
                    }
                }
            }
            _ = tokio::time::sleep(poll_interval) => {
                // Timer expired — fall through to re-walk
            }
        }

        // Re-walk all zones
        for wz in zones {
            let inodes = if wz.root.exists() {
                walk_zone_root(&wz.root, &db_cache, wz.allowed_subdirs.as_deref())
            } else {
                HashMap::new()
            };
            let _ = message_tx.send(WatcherMessage::InitialScanComplete {
                zone: wz.zone,
                inodes,
            });
        }
        let _ = message_tx.send(WatcherMessage::AllInitialScansComplete);
    }
}

/// Internal wrapper for notify events, bridged into the tokio channel.
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
        // Only process files in our zone roots (respecting subdir filters)
        let in_zone = zone_states.iter().any(|zs| zs.accepts_path(path));
        if !in_zone {
            continue;
        }

        // Directory events: a directory was created, renamed, or removed.
        // When a directory is renamed, inotify fires events for the directory
        // but NOT for individual files inside it — their paths change silently.
        // We synthesize per-file events so the watcher discovers the new paths.
        if path.is_dir() {
            // Directory exists here (renamed TO / created). Walk it and
            // schedule tracked files for debounced processing.
            synthesize_directory_children(path, zone_states, debounce_map, pending_paths, now);
            continue;
        }

        if !path.exists() && !is_tracked_file(path) {
            // Path doesn't exist and isn't a tracked file — might be a
            // directory that was renamed FROM here. Find tracked children
            // under this prefix so their stale paths get stat-checked
            // (stat will fail → FileRemoved).
            synthesize_stale_children(path, zone_states, debounce_map, pending_paths, now);
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
    message_tx: &mpsc::UnboundedSender<WatcherMessage>,
) {
    // Find which zone this path belongs to (respecting subdir filters)
    let zone_idx = match zone_states.iter().position(|zs| zs.accepts_path(path)) {
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

/// Walk a directory that appeared on disk (renamed TO or created) and schedule
/// its tracked children for debounced processing. This lets `process_settled_event`
/// discover new paths for inodes that moved due to a directory rename.
fn synthesize_directory_children(
    dir: &Path,
    zone_states: &[ZoneState],
    debounce_map: &mut HashMap<PathBuf, Instant>,
    pending_paths: &mut Vec<PathBuf>,
    now: Instant,
) {
    // Recursive walk — directory renames move the entire subtree.
    let mut stack = vec![dir.to_path_buf()];
    let mut file_count = 0usize;
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
            } else if is_tracked_file(&path) {
                let in_zone = zone_states.iter().any(|zs| zs.accepts_path(&path));
                if in_zone {
                    let is_new = !debounce_map.contains_key(&path);
                    debounce_map.insert(path.clone(), now);
                    if is_new {
                        pending_paths.push(path);
                        file_count += 1;
                    }
                }
            }
        }
    }
    if file_count > 0 {
        crate::logging::log_general(format!(
            "[FS_THREAD] Directory event at {:?}: synthesized events for {} tracked files",
            dir, file_count
        ));
    }
}

/// When a path doesn't exist and isn't a tracked file, it may be a directory
/// that was renamed FROM here. Find all tracked file paths under this prefix
/// and schedule them for debounced processing — stat will fail at the stale
/// path, producing `FileRemoved` messages that clean up watcher state.
fn synthesize_stale_children(
    gone_dir: &Path,
    zone_states: &[ZoneState],
    debounce_map: &mut HashMap<PathBuf, Instant>,
    pending_paths: &mut Vec<PathBuf>,
    now: Instant,
) {
    let mut stale_count = 0usize;
    for zs in zone_states {
        if !zs.accepts_path(gone_dir) {
            continue;
        }
        for tracked_path in zs.path_to_inode.keys() {
            if tracked_path.starts_with(gone_dir) {
                let is_new = !debounce_map.contains_key(tracked_path);
                debounce_map.insert(tracked_path.clone(), now);
                if is_new {
                    pending_paths.push(tracked_path.clone());
                    stale_count += 1;
                }
            }
        }
    }
    if stale_count > 0 {
        crate::logging::log_general(format!(
            "[FS_THREAD] Directory gone at {:?}: synthesized events for {} stale tracked files",
            gone_dir, stale_count
        ));
    }
}

/// Handle a notify error (inotify overflow, watch limit, etc.)
///
/// Returns `true` if the caller should fall back to polling mode
/// (watch limit exhaustion — inotify is no longer viable).
fn handle_notify_error(error: notify::Error) -> bool {
    crate::logging::log_error(format!(
        "[FS_THREAD] Notify error: {}",
        error
    ));

    // Only rescan on watch limit exhaustion — the one error that means
    // we're definitively missing events. Generic errors are logged but not
    // worth a full re-walk (which could itself trigger more errors).
    if matches!(error.kind, notify::ErrorKind::MaxFilesWatch) {
        crate::logging::log_general(
            "[FS_THREAD] inotify overflow/limit — requesting rescan from Witch"
        );
        return true;
    }

    false
}

/// Read tags from an audio file. Returns empty TagSet on error.
fn read_tags(path: &Path) -> crate::corpus::tags::TagSet {
    match crate::corpus::tags::from_file(path) {
        Ok(tagset) => tagset,
        Err(e) => {
            crate::logging::log_error(format!(
                "[FS_THREAD] Failed to read tags from {:?}: {}",
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
    message_tx: &mpsc::UnboundedSender<WatcherMessage>,
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
fn walk_zone_root(
    root: &Path,
    db_cache: &HashMap<i64, CachedInodeState>,
    allowed_subdirs: Option<&[String]>,
) -> HashMap<i64, ObservedFile> {
    let mut result = HashMap::new();

    // Enumerate all directories (including root itself)
    let (directories, symlink_count) = enumerate_directories(root, allowed_subdirs);

    if symlink_count > 0 {
        crate::logging::log_general(format!(
            "[FS_THREAD] Skipped {} directory symlinks in {:?}",
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
/// When `allowed_subdirs` is Some, only top-level children of root whose
/// name matches are entered (used for Library zone to skip non-deployment dirs).
fn enumerate_directories(root: &Path, allowed_subdirs: Option<&[String]>) -> (Vec<PathBuf>, usize) {
    let mut directories = Vec::new();
    let mut symlink_count = 0;

    if let Some(subdirs) = allowed_subdirs {
        // Only recurse into allowed top-level subdirectories
        for subdir in subdirs {
            let subdir_path = root.join(subdir);
            if subdir_path.is_dir() && !subdir_path.is_symlink() {
                directories.push(subdir_path.clone());
                enumerate_directories_recursive(&subdir_path, &mut directories, &mut symlink_count);
            }
        }
    } else {
        enumerate_directories_recursive(root, &mut directories, &mut symlink_count);
        directories.push(root.to_path_buf());
    }

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
                            "[FS_THREAD] Nested mount point detected at {:?} \
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
