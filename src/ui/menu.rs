use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use crate::config::{self, ScanSource};
use crate::db::{Database, Track};
use crate::metadata;
use crate::progress::{ScanMessage, ScanProgress};
use crate::reports;
use crate::scanner;

// ASCII eye frames from The Talos Principle
const EYE_OPEN: &str = r#"                     ...',;;:cccccccc:;,..                  
                    ..,;:cccc::::ccccclloooolc;'.               
                 .',;:::;;;;:loodxk0kkxxkxxdocccc;;'..          
               .,;;;,,;:coxldKNWWWMMMMWNNWWNNKkdolcccc:,.       
            .',;;,',;lxo:...dXWMMMMMMMMNkloOXNNNX0koc:coo;.     
         ..,;:;,,,:ldl'   .kWMMMWXXNWMMMMXd..':d0XWWN0d:;lkd,   
       ..,;;,,'':loc.     lKMMMNl. .c0KNWNK:  ..';lx00X0l,cxo,. 
     ..''....'cooc.       c0NMMX;   .l0XWN0;       ,ddx00occl:. 
   ..'..  .':odc.         .x0KKKkolcld000xc.       .cxxxkkdl:,..
 ..''..   ;dxolc;'         .lxx000kkxx00kc.      .;looolllol:'..
..'..    .':lloolc:,..       'lxkkkkk0kd,   ..':clc:::;,,;:;,'..
......   ....',;;;:ccc::;;,''',:loddol:,,;:clllolc:;;,'........ 
    .     ....'''',,,;;:cccccclllloooollllccc:c:::;,'..         
            .......'',,,,,,,,;;::::ccccc::::;;;,,''...          
              ...............''',,,;;;,,''''''......            
                   ............................                 "#;

const EYE_CLOSING: &str = r#"                         ...'',;;;;::;;;,'..                    
                    ..,;:cloodddxxxkkkkkkkkxol;..               
                 .';codxxkkk000000000000kkkkkkxdoc,..           
               .,codxk0000000000000000000000000kkxddoc,..       
            .':ldxk00000000000000000000000000000000kkxxol:'     
         .,:ldxkk000000000K000000000000000K0000000000kkxkkx:.   
      ..,coxkk000000000000000kk000000000000000000000000kxxxxl'  
     .,;codxxkk00000000kkk0KK0XNWWWWWWWWWNX0kkkkk00000kkxdool;. 
   .';::ccldk00KKKK00oc;..,x00KNNXXXXXNNX0000000000kkkkkkxoc:,..
 ..,;,'..,o00000kkxo,       ,lkKKKKKK0K0d,.;ldk000KK0kxxxdoc:'..
..,,'.  .,lk0xxxdol:,..       .,ldddl:,.   .,codkk00kxdollc:,...
..'.......',;:c::cclccc::;,,,',,;::::;,;;:clodddxdol:;::;'......
   .....  ...''',,,;;;:ccllloooooooooooooolllcccc:;;,....       
            .......'',,,,,,;;;:::ccclllccc:::;;;,''...          
              ..............'''',,;;;;;,,,''''......            
                   .............................                "#;

const EYE_CLOSED: &str = r#"                          ...'',;;;;;;;,,...                    
                    ..,:loxkk000000KKKKKK00xdc,..               
                 .,cox000KXXXXXXXXXXXXXXXXXXXK00xo:,..          
              ..;lx000KKKKK000000000000000KKKKXXXXK00xl;..      
           ..,:oxk00000000000000000000000000000000KKKKKK0d:.    
        ..;codxkk000kkkkkkkxxxxxxxxxxxxxxxxkkkkkk000000KK0kl'   
      ..;ldxkkkkkkxxxxxddddddddddddddddddddddddxxxxxxkkk000xc.  
    ..,:oxxkkkkkxxxxdddddddddddddddddddddddddddddddddxxkkkkxl;. 
  ..,;codxxkkkxxddddddddddddddddddddddddddddddddxdxxxk000kxdo:..
 .';::::cldk000kkkxxxxxxdddddddddddddddddddddxxxxxkk0000xkddl;..
.';:;,..,ckXXXKKK0KK000kxk0doddxxdddddddxxxxxxkk0000kkkkkxdoc,..
.',,''..,:oxxxxxxxkkxkkxk00xxk000000000000KKKKKKK000kxdllll:,.. 
 .........',,,:ccllllooooxkxxx000kk0000000000000000kxdoc,'...   
         ......',;;::cc::clllloddoox0xdxxkxxxxddollllc:'.       
              .....'',,,,,,,;;;;;::cllc::ccc::;;,,,'...         
                  ..................'''..'''......              "#;

enum MenuState {
    MainMenu,
    ScanSourceMenu {
        sources: Vec<ScanSource>,
    },
    ReportsMenu,
    DeployMenu,
    CorpusTriageMenu,
    #[allow(dead_code)] // Legacy variants - replaced by TagEditor flow in Phase 6
    DuplicateGroupList {
        groups: Vec<DuplicateGroup>,
    },
    #[allow(dead_code)] // Legacy variants - replaced by TagEditor flow in Phase 6
    DuplicateGroupDetail {
        group: DuplicateGroup,
        selected_track_idx: usize,
    },
    TagEditor {
        tracks: Vec<Track>,
        tag_fields: Vec<Vec<TagField>>, // Tag fields for each track (current state)
        original_tag_fields: Vec<Vec<TagField>>, // Phase 3: Original state for change preview
        current_track_idx: usize,
        current_field_idx: usize,

        // New edit state model (replaces editing_mode)
        field_edit_state: FieldEditState,

        // Separate buffers to preserve both name and value
        name_buffer: String,
        value_buffer: String,
        original_name: String,
        original_value: String,

        // Duplicate workflow tracking
        duplicate_groups: Vec<DuplicateGroupInfo>,
        current_group_idx: Option<usize>,
        // show_write_changes_button removed - Phase 0: using modal instead
        focus_on_value: bool, // true = editing value, false = editing name
    },
    // Phase 2: Modal displayed when user tabs past last track
    SaveConfirmationModal {
        tracks: Vec<Track>,
        tag_fields: Vec<Vec<TagField>>,
        original_tag_fields: Vec<Vec<TagField>>, // For change preview
        selected_button: usize,                  // 0 = Save All, 1 = Save & Next, 2 = Return
        // Keep track of where we were in TagEditor so we can return
        previous_track_idx: usize,
        previous_field_idx: usize,
        // Phase 6: Duplicate workflow tracking
        duplicate_groups: Vec<DuplicateGroupInfo>,
        current_group_idx: Option<usize>,
    },
    // Phase 4: Modal showing diff preview before saving
    ChangePreviewModal {
        tracks: Vec<Track>,
        grouped_changes: Vec<GroupedChange>,
        single_changes: Vec<TagChange>,
        scroll_offset: usize,
        save_and_next: bool, // true if user wants to advance to next set after save
        // Phase 6: Duplicate workflow tracking
        duplicate_groups: Vec<DuplicateGroupInfo>,
        current_group_idx: Option<usize>,
    },
    // Fingerprint deduplication: Directory selection
    FingerprintDedupDirSelect {
        selected_dirs: Vec<PathBuf>,
        current_input: String,
        error_message: Option<String>,
    },
    // Fingerprint deduplication: Conflict resolution
    FingerprintDedupResolve {
        conflict_sets: Vec<crate::deduplication::ConflictSet>,
        current_set_idx: usize,
        session_stats: crate::deduplication::SessionStats,
        input_buffer: String,
        corpus_root: PathBuf,
        lost_found_root: PathBuf,
    },
    // Fingerprint deduplication: End statistics
    FingerprintDedupStats {
        stats: crate::deduplication::SessionStats,
        lost_found_path: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FieldEditState {
    NonEditable,  // Default state - field is not being edited
    EditingName,  // User is editing the tag name
    EditingValue, // User is editing the tag value
}

#[allow(dead_code)] // Legacy struct - used by deprecated DuplicateGroupList variant
#[derive(Debug, Clone)]
struct DuplicateGroup {
    id: i64,
    group_type: DuplicateGroupType,
    tracks: Vec<Track>,
}

#[allow(dead_code)] // Some fields used internally during duplicate workflow
#[derive(Debug, Clone)]
struct DuplicateGroupInfo {
    group_id: i64,
    tracks: Vec<Track>,
    resolved: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum DuplicateGroupType {
    ExactMatch,    // Same file in multiple locations
    MetadataMatch, // Same metadata, different files
}

// Phase 4: Change tracking structs for preview modal
#[derive(Debug, Clone)]
struct TagChange {
    track_idx: usize,
    field_name: String,
    old_value: String,
    new_value: String,
}

#[derive(Debug, Clone)]
struct GroupedChange {
    field_name: String,
    old_value: String,
    new_value: String,
    track_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
struct TagField {
    name: String,
    value: String,
    editable: bool,
    is_unique_per_track: bool, // true for title/track_number - cannot be filled to all
}

// Helper function to convert Track to editable tag fields
// Priority order: track_number, title, artist, album, album_artist, date, genre, isrc, then alphabetically
// Optimized for compilation tagging workflow (common: track_number → title → artist)
// Read-only fields (path, file_type, duration, bitrate) are NOT included - they go in the info pane
fn track_to_tag_fields(track: &Track) -> Vec<TagField> {
    use std::collections::HashSet;
    use std::path::Path;

    let path = Path::new(&track.path);

    // Try to read all tags from the file (as Vec to preserve duplicates)
    let all_tags_vec: Vec<(String, String)> = match metadata::read_all_tags(path) {
        Ok(tags) => tags,
        Err(_) => {
            // If reading fails, fall back to database fields only
            let mut vec = Vec::new();
            if let Some(ref artist) = track.artist {
                vec.push(("artist".to_string(), artist.clone()));
            }
            if let Some(ref album) = track.album {
                vec.push(("album".to_string(), album.clone()));
            }
            if let Some(ref album_artist) = track.album_artist {
                vec.push(("album_artist".to_string(), album_artist.clone()));
            }
            if let Some(ref title) = track.title {
                vec.push(("title".to_string(), title.clone()));
            }
            if let Some(track_num) = track.track_number {
                vec.push(("track_number".to_string(), track_num.to_string()));
            }
            if let Some(ref isrc) = track.isrc {
                vec.push(("isrc".to_string(), isrc.clone()));
            }
            vec
        }
    };

    let mut tag_fields = Vec::new();

    // Priority fields in specific order (name, is_unique_per_track)
    // Optimized for compilation tagging: track_number → title → artist
    let priority_fields = vec![
        ("track_number", true), // Unique per track
        ("title", true),        // Unique per track
        ("artist", false),
        ("album", false),
        ("album_artist", false), // Can have multiple values
        ("date", false),
        ("genre", false),
        ("isrc", false),
    ];

    // Add priority fields first (handle album_artist specially for multiple values)
    for (field_name, is_unique) in &priority_fields {
        let matching_values: Vec<String> = all_tags_vec
            .iter()
            .filter(|(k, _)| k == field_name)
            .map(|(_, v)| v.clone())
            .collect();

        if field_name == &"album_artist" {
            // Allow multiple album_artist entries
            if matching_values.is_empty() {
                // Add one empty field if none exist
                tag_fields.push(TagField {
                    name: field_name.to_string(),
                    value: String::new(),
                    editable: true,
                    is_unique_per_track: *is_unique,
                });
            } else {
                // Add all existing album_artist values
                for value in matching_values {
                    tag_fields.push(TagField {
                        name: field_name.to_string(),
                        value,
                        editable: true,
                        is_unique_per_track: *is_unique,
                    });
                }
            }
        } else {
            // Single value for other fields (use first if multiple exist)
            // Warn if multiple instances found for unique-per-track fields
            if *is_unique && matching_values.len() > 1 {
                let _ = crate::config::log_message(&format!(
                    "Warning: Multiple {} tags found ({}), using first value",
                    field_name,
                    matching_values.len()
                ));
            }
            tag_fields.push(TagField {
                name: field_name.to_string(),
                value: matching_values.first().cloned().unwrap_or_default(),
                editable: true,
                is_unique_per_track: *is_unique,
            });
        }
    }

    // Add other tags alphabetically (excluding priority fields and read-only fields)
    let skip_fields: HashSet<&str> = priority_fields
        .iter()
        .map(|(name, _)| *name)
        .chain(
            ["bitrate", "sample_rate", "duration", "path", "file_type"]
                .iter()
                .copied(),
        )
        .collect();

    let mut other_tags: Vec<_> = all_tags_vec
        .iter()
        .filter(|(key, _)| !skip_fields.contains(key.as_str()))
        .collect();
    other_tags.sort_by_key(|(key, _)| key.as_str());

    for (key, value) in other_tags {
        tag_fields.push(TagField {
            name: key.clone(),
            value: value.clone(),
            editable: true,
            is_unique_per_track: false,
        });
    }

    // Add "New Tag" line as last interactable field
    tag_fields.push(TagField {
        name: "New Tag".to_string(),
        value: "[Press Enter to create]".to_string(),
        editable: true,
        is_unique_per_track: false,
    });

    tag_fields
}

/// Phase 4: Compute all changes between original and current tag fields
fn compute_changes(original: &[Vec<TagField>], current: &[Vec<TagField>]) -> Vec<TagChange> {
    let mut changes = Vec::new();

    for (track_idx, (orig_fields, curr_fields)) in original.iter().zip(current.iter()).enumerate() {
        // Compare field by field
        for (orig_field, curr_field) in orig_fields.iter().zip(curr_fields.iter()) {
            // Skip "New Tag" placeholder
            if orig_field.name == "New Tag" || curr_field.name == "New Tag" {
                continue;
            }

            // Detect changes in value (name changes are rarer but also tracked)
            if orig_field.value != curr_field.value {
                changes.push(TagChange {
                    track_idx,
                    field_name: curr_field.name.clone(),
                    old_value: orig_field.value.clone(),
                    new_value: curr_field.value.clone(),
                });
            }
        }
    }

    changes
}

/// Phase 4: Group changes that are identical across multiple tracks
fn group_common_changes(changes: &[TagChange]) -> (Vec<GroupedChange>, Vec<TagChange>) {
    use std::collections::HashMap;

    // Group by (field_name, old_value, new_value)
    let mut groups: HashMap<(String, String, String), Vec<usize>> = HashMap::new();

    for change in changes {
        let key = (
            change.field_name.clone(),
            change.old_value.clone(),
            change.new_value.clone(),
        );
        groups.entry(key).or_default().push(change.track_idx);
    }

    // Split into grouped (2+ tracks) and single-track changes
    let mut grouped = Vec::new();
    let mut singles = Vec::new();

    for ((field_name, old_value, new_value), track_indices) in groups {
        if track_indices.len() > 1 {
            grouped.push(GroupedChange {
                field_name,
                old_value,
                new_value,
                track_indices,
            });
        } else {
            // Find the original TagChange for this single track
            let track_idx = track_indices[0];
            if let Some(change) = changes
                .iter()
                .find(|c| c.track_idx == track_idx && c.field_name == field_name)
            {
                singles.push(change.clone());
            }
        }
    }

    (grouped, singles)
}

#[derive(Debug, Clone, Copy)]
enum EyeAnimationState {
    Idle,           // Eye open, waiting for next blink
    Closing,        // Transitioning to closed
    Closed,         // Eye closed
    Opening,        // Transitioning back to open
    FlutterClosing, // Rapid flutter blink
    FlutterClosed,  // Rapid flutter closed
    FlutterOpening, // Rapid flutter opening
}

#[derive(Debug, Clone, Copy)]
enum BlinkType {
    Normal,  // Medium speed blink
    Slow,    // Slower, more deliberate blink
    Fast,    // Quick blink
    Flutter, // Multiple rapid blinks
}

struct OperationState {
    operation_type: OperationType,
    progress: ScanProgress,
    cancel_flag: Arc<AtomicBool>,
}

enum OperationType {
    Scanning { source_name: String },
    GeneratingReport { report_type: String },
    Deploying { library_name: String, dry_run: bool },
}

struct App {
    menu_state: ListState,
    menu_items: Vec<MenuItem>,
    should_quit: bool,
    status_message: Option<String>,

    // New fields for scanning
    current_view: MenuState,
    operation_in_progress: Option<OperationState>,
    operation_receiver: Option<mpsc::Receiver<ScanMessage>>,
    config: Option<config::Config>,

    // Eye animation state
    eye_animation_state: EyeAnimationState,
    eye_state_start_time: std::time::Instant,
    next_blink_delay_secs: u64, // Random delay until next blink
    current_blink_type: BlinkType,
    flutter_count: u8, // For counting multiple blinks in flutter mode

    // Tag editing session
    tag_edit_session_id: String,
}

#[derive(Clone)]
struct MenuItem {
    label: String,
    action: MenuAction,
}

#[derive(Clone)]
enum MenuAction {
    ScanSources,
    Reports,
    Deploy,
    CorpusTriage,
    Quit,
}

// Corpus Triage Menu Actions
enum CorpusTriageAction {
    MetadataDeduplication,
    FingerprintDeduplication,
    Back,
}

#[derive(Clone)]
enum ReportType {
    GenerateAll,
    Legacy,
    Deployment,
    Quality,
    Duplicates,
}

impl App {
    fn new() -> Self {
        // Try to load config and capture any error for display
        let (config, initial_status) = match config::load_config() {
            Ok(cfg) => (Some(cfg), None),
            Err(e) => {
                // Log the error to /tmp/mla.log
                let _ = config::log_message(&format!("Config load error: {}", e));

                // Show the error to the user
                let error_msg = format!(
                    "⚠ Config Error: {}\n\nSome features will be unavailable.",
                    e
                );
                (None, Some(error_msg))
            }
        };

        let menu_items = vec![
            MenuItem {
                label: "Scan Corpus/Library".to_string(),
                action: MenuAction::ScanSources,
            },
            MenuItem {
                label: "Generate Reports".to_string(),
                action: MenuAction::Reports,
            },
            MenuItem {
                label: "Deploy to Libraries".to_string(),
                action: MenuAction::Deploy,
            },
            MenuItem {
                label: "Corpus Triage and Operations".to_string(),
                action: MenuAction::CorpusTriage,
            },
            MenuItem {
                label: "Quit".to_string(),
                action: MenuAction::Quit,
            },
        ];

        let mut menu_state = ListState::default();
        menu_state.select(Some(0));

        Self {
            menu_state,
            menu_items,
            should_quit: false,
            status_message: initial_status,
            current_view: MenuState::MainMenu,
            operation_in_progress: None,
            operation_receiver: None,
            config,
            eye_animation_state: EyeAnimationState::Idle,
            eye_state_start_time: std::time::Instant::now(),
            next_blink_delay_secs: Self::random_blink_delay(),
            current_blink_type: BlinkType::Normal,
            flutter_count: 0,
            tag_edit_session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    // Generate random delay between blinks: 30-60 seconds with occasional longer pauses
    fn random_blink_delay() -> u64 {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;

        // Simple pseudo-random using current time
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let hasher = RandomState::new();

        let random_val = hasher.hash_one(now);

        // 30-60 seconds normally, occasionally up to 90 seconds
        let base = 30 + (random_val % 30);
        if random_val % 10 == 0 {
            base + 30 // 10% chance of extra long pause
        } else {
            base
        }
    }

    // Choose random blink type (weighted towards normal)
    fn random_blink_type() -> BlinkType {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let hasher = RandomState::new();

        let random_val = hasher.hash_one(now) % 100;

        match random_val {
            0..=5 => BlinkType::Flutter, // 6% flutter
            6..=20 => BlinkType::Slow,   // 15% slow
            21..=40 => BlinkType::Fast,  // 20% fast
            _ => BlinkType::Normal,      // 59% normal
        }
    }

    fn update_eye_animation(&mut self) {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.eye_state_start_time);

        match self.eye_animation_state {
            EyeAnimationState::Idle => {
                // Wait for random delay before next blink
                if elapsed.as_secs() >= self.next_blink_delay_secs {
                    // Time to blink! Choose blink type and start
                    self.current_blink_type = Self::random_blink_type();
                    self.eye_animation_state = EyeAnimationState::Closing;
                    self.eye_state_start_time = now;

                    // If flutter, prepare for multiple blinks
                    if matches!(self.current_blink_type, BlinkType::Flutter) {
                        self.flutter_count = 2 + ((now.elapsed().as_nanos() % 2) as u8);
                        // 2-3 rapid blinks
                    }
                }
            }
            EyeAnimationState::Closing => {
                let duration = match self.current_blink_type {
                    BlinkType::Slow => 250,   // Slow closing
                    BlinkType::Normal => 120, // Normal speed
                    BlinkType::Fast => 60,    // Quick close
                    BlinkType::Flutter => 40, // Very rapid
                };

                if elapsed.as_millis() >= duration {
                    self.eye_animation_state = EyeAnimationState::Closed;
                    self.eye_state_start_time = now;
                }
            }
            EyeAnimationState::Closed => {
                let duration = match self.current_blink_type {
                    BlinkType::Slow => 200,   // Slow blink stays closed longer
                    BlinkType::Normal => 100, // Brief closure
                    BlinkType::Fast => 50,    // Quick closure
                    BlinkType::Flutter => 30, // Very brief
                };

                if elapsed.as_millis() >= duration {
                    self.eye_animation_state = EyeAnimationState::Opening;
                    self.eye_state_start_time = now;
                }
            }
            EyeAnimationState::Opening => {
                let duration = match self.current_blink_type {
                    BlinkType::Slow => 250,   // Slow opening
                    BlinkType::Normal => 120, // Normal speed
                    BlinkType::Fast => 60,    // Quick open
                    BlinkType::Flutter => 40, // Very rapid
                };

                if elapsed.as_millis() >= duration {
                    // Check if we need to flutter again
                    if matches!(self.current_blink_type, BlinkType::Flutter)
                        && self.flutter_count > 0
                    {
                        self.flutter_count -= 1;
                        self.eye_animation_state = EyeAnimationState::FlutterClosing;
                        self.eye_state_start_time = now;
                    } else {
                        // Back to idle, schedule next blink
                        self.eye_animation_state = EyeAnimationState::Idle;
                        self.eye_state_start_time = now;
                        self.next_blink_delay_secs = Self::random_blink_delay();
                    }
                }
            }
            EyeAnimationState::FlutterClosing => {
                // Rapid flutter blink
                if elapsed.as_millis() >= 35 {
                    self.eye_animation_state = EyeAnimationState::FlutterClosed;
                    self.eye_state_start_time = now;
                }
            }
            EyeAnimationState::FlutterClosed => {
                if elapsed.as_millis() >= 25 {
                    self.eye_animation_state = EyeAnimationState::FlutterOpening;
                    self.eye_state_start_time = now;
                }
            }
            EyeAnimationState::FlutterOpening => {
                if elapsed.as_millis() >= 35 {
                    if self.flutter_count > 0 {
                        // More flutter blinks to go
                        self.flutter_count -= 1;
                        self.eye_animation_state = EyeAnimationState::FlutterClosing;
                        self.eye_state_start_time = now;
                    } else {
                        // Done with flutter, back to idle
                        self.eye_animation_state = EyeAnimationState::Idle;
                        self.eye_state_start_time = now;
                        self.next_blink_delay_secs = Self::random_blink_delay();
                    }
                }
            }
        }
    }

    fn next(&mut self) {
        let len = match &self.current_view {
            MenuState::MainMenu => self.menu_items.len(),
            MenuState::ScanSourceMenu { sources } => sources.len() + 2, // +2 for "Scan All" and "Clean Rescan All"
            MenuState::ReportsMenu => 5, // 5 report options (including Generate All)
            MenuState::DeployMenu => 2,  // Dry Run, Full Deploy
            MenuState::CorpusTriageMenu => 3, // Metadata dedup, Fingerprint dedup, Back
            MenuState::DuplicateGroupList { groups } => groups.len(),
            MenuState::DuplicateGroupDetail { group, .. } => group.tracks.len(),
            MenuState::TagEditor { .. } => return, // Custom navigation handled in handle_key
            MenuState::SaveConfirmationModal { .. } => return, // Phase 2: Custom navigation in modal handler
            MenuState::ChangePreviewModal { .. } => return, // Phase 4: Custom navigation in modal handler
            MenuState::FingerprintDedupDirSelect { .. } => return, // Text input, no navigation
            MenuState::FingerprintDedupResolve { .. } => return, // Custom navigation
            MenuState::FingerprintDedupStats { .. } => return, // No navigation needed
        };

        let i = match self.menu_state.selected() {
            Some(i) => {
                if i >= len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.menu_state.select(Some(i));
    }

    fn previous(&mut self) {
        let len = match &self.current_view {
            MenuState::MainMenu => self.menu_items.len(),
            MenuState::ScanSourceMenu { sources } => sources.len() + 2, // +2 for "Scan All" and "Clean Rescan All"
            MenuState::ReportsMenu => 5, // 5 report options (including Generate All)
            MenuState::DeployMenu => 2,  // Dry Run, Full Deploy
            MenuState::CorpusTriageMenu => 3, // Metadata dedup, Fingerprint dedup, Back
            MenuState::DuplicateGroupList { groups } => groups.len(),
            MenuState::DuplicateGroupDetail { group, .. } => group.tracks.len(),
            MenuState::TagEditor { .. } => return, // Custom navigation handled in handle_key
            MenuState::SaveConfirmationModal { .. } => return, // Phase 2: Custom navigation in modal handler
            MenuState::ChangePreviewModal { .. } => return, // Phase 4: Custom navigation in modal handler
            MenuState::FingerprintDedupDirSelect { .. } => return, // Text input, no navigation
            MenuState::FingerprintDedupResolve { .. } => return, // Custom navigation
            MenuState::FingerprintDedupStats { .. } => return, // No navigation needed
        };

        let i = match self.menu_state.selected() {
            Some(i) => {
                if i == 0 {
                    len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.menu_state.select(Some(i));
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Check if scan is in progress - handle cancellation
        if let Some(ref op) = self.operation_in_progress {
            match key.code {
                KeyCode::Char('c') | KeyCode::Esc => {
                    // Cancel the operation
                    op.cancel_flag.store(true, Ordering::Relaxed);
                    self.status_message = Some("Cancelling scan...".to_string());
                    return;
                }
                _ => {}
            }
        }

        // ========================================
        // PHASE 4: REORDERED KEYBOARD HANDLERS
        // Process in order: Ctrl keys → F/W keys → Character input → Navigation → Enter → Esc
        // ========================================

        // 1. CTRL KEY HANDLERS (must come before character capture)
        match key.code {
            // Ctrl+Shift+S: Check for uppercase 'S' (Shift+s produces 'S')
            KeyCode::Char('S')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                // Ctrl+Shift+S: Save all tracks in current set, re-analyze, proceed to next set
                if let MenuState::TagEditor { ref tracks, .. } = self.current_view {
                    // Save all tracks
                    self.save_all_tags(tracks.clone());

                    // TODO: Re-analyze for duplicates
                    // TODO: If no duplicates, load next duplicate set
                    // TODO: If auto_next_save_all opinion is enabled, automatically proceed

                    self.status_message =
                        Some("All tracks saved. Re-analyzing for duplicates...".to_string());
                    return;
                }
            }
            // DISABLED: Individual track saves not allowed - only batch saves
            // Phase 0: Atomic save operations - removed Ctrl+S single-track save
            // KeyCode::Char('s') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            //     // Ctrl+S: Save current track only
            //     if let MenuState::TagEditor {
            //         ref mut tracks,
            //         ref tag_fields,
            //         current_track_idx,
            //         ..
            //     } = self.current_view {
            //         // ... existing save logic ...
            //     }
            // }
            KeyCode::Char('u')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                // Ctrl+U: Clear field buffer
                if let MenuState::TagEditor {
                    ref mut field_edit_state,
                    ref mut name_buffer,
                    ref mut value_buffer,
                    ..
                } = self.current_view
                {
                    match field_edit_state {
                        FieldEditState::EditingName => {
                            name_buffer.clear();
                            self.status_message = Some("Name buffer cleared".to_string());
                        }
                        FieldEditState::EditingValue => {
                            value_buffer.clear();
                            self.status_message = Some("Value buffer cleared".to_string());
                        }
                        FieldEditState::NonEditable => {
                            // Ignore in non-editable state
                        }
                    }
                    return;
                }
            }
            _ => {}
        }

        // 2. F KEY HANDLER: Fill to all tracks
        if let KeyCode::Char('f') | KeyCode::Char('F') = key.code {
            if let MenuState::TagEditor {
                ref mut tag_fields,
                current_track_idx,
                current_field_idx,
                field_edit_state: FieldEditState::NonEditable,
                ..
            } = self.current_view
            {
                let current_field = &tag_fields[current_track_idx][current_field_idx];

                // Block filling for unique-per-track fields
                if current_field.is_unique_per_track {
                    self.status_message = Some(format!(
                        "Cannot fill '{}' to all tracks (unique per track)",
                        current_field.name
                    ));
                    return;
                }

                let current_field_name = current_field.name.clone();
                let current_name = current_field.name.clone();
                let current_value = current_field.value.clone();

                // Fill BOTH name and value to all tracks
                let mut filled_count = 0;
                for track_fields in tag_fields.iter_mut() {
                    if let Some(field) = track_fields
                        .iter_mut()
                        .find(|f| f.name == current_field_name && f.editable)
                    {
                        field.name = current_name.clone();
                        field.value = current_value.clone();
                        filled_count += 1;
                    }
                }
                self.status_message = Some(format!(
                    "Filled '{}' to {} tracks (W to save)",
                    current_field_name, filled_count
                ));
                return;
            }
        }

        // 3. W KEY HANDLER: Write tag changes
        // DISABLED: Individual saves not allowed - must use end-of-tracks modal
        // Phase 0: Atomic save operations - removed W key batch save
        // if let KeyCode::Char('w') | KeyCode::Char('W') = key.code {
        //     if let MenuState::TagEditor {
        //         ref tracks,
        //         show_write_changes_button: true,
        //         ..
        //     } = self.current_view {
        //         self.save_all_tags(tracks.clone());
        //         return;
        //     }
        // }

        // 4. CHARACTER INPUT (only when editing)
        if let MenuState::TagEditor {
            ref field_edit_state,
            ref mut name_buffer,
            ref mut value_buffer,
            ..
        } = self.current_view
        {
            match field_edit_state {
                FieldEditState::EditingName => match key.code {
                    KeyCode::Char(c) => {
                        name_buffer.push(c);
                        return;
                    }
                    KeyCode::Backspace => {
                        name_buffer.pop();
                        return;
                    }
                    _ => {}
                },
                FieldEditState::EditingValue => match key.code {
                    KeyCode::Char(c) => {
                        value_buffer.push(c);
                        return;
                    }
                    KeyCode::Backspace => {
                        value_buffer.pop();
                        return;
                    }
                    _ => {}
                },
                FieldEditState::NonEditable => {
                    // Fall through to navigation handlers
                }
            }
        }

        // Phase 2: Handle SaveConfirmationModal keyboard input
        if let MenuState::SaveConfirmationModal {
            ref mut selected_button,
            ref tracks,
            ref tag_fields,
            ref original_tag_fields,
            previous_track_idx,
            previous_field_idx,
            ref duplicate_groups,
            current_group_idx,
        } = self.current_view
        {
            match key.code {
                KeyCode::Left => {
                    *selected_button = selected_button.saturating_sub(1);
                    return;
                }
                KeyCode::Right => {
                    *selected_button = (*selected_button + 1).min(2);
                    return;
                }
                KeyCode::Enter => {
                    match *selected_button {
                        0 | 1 => {
                            // "Save all changes" or "Save & Next Set"
                            // Phase 4: Compute changes and show preview modal
                            let changes = compute_changes(original_tag_fields, tag_fields);

                            if changes.is_empty() {
                                // No changes to save
                                self.status_message = Some("No changes to save".to_string());
                                self.current_view = MenuState::MainMenu;
                                return;
                            }

                            let (grouped, singles) = group_common_changes(&changes);

                            self.current_view = MenuState::ChangePreviewModal {
                                tracks: tracks.clone(),
                                grouped_changes: grouped,
                                single_changes: singles,
                                scroll_offset: 0,
                                save_and_next: *selected_button == 1, // true if "Save & Next"
                                duplicate_groups: duplicate_groups.clone(),
                                current_group_idx,
                            };
                            return;
                        }
                        2 => {
                            // "Return to editing" - restore TagEditor state
                            // Need to capture all state to restore properly
                            let tracks_clone = tracks.clone();
                            let tag_fields_clone = tag_fields.clone();
                            let original_tag_fields_clone = original_tag_fields.clone();
                            let prev_track = previous_track_idx;
                            let prev_field = previous_field_idx;

                            // Load buffers from the saved position
                            let name_buf = tag_fields_clone[prev_track][prev_field].name.clone();
                            let value_buf = tag_fields_clone[prev_track][prev_field].value.clone();

                            self.current_view = MenuState::TagEditor {
                                tracks: tracks_clone,
                                tag_fields: tag_fields_clone.clone(),
                                original_tag_fields: original_tag_fields_clone,
                                current_track_idx: prev_track,
                                current_field_idx: prev_field,
                                field_edit_state: FieldEditState::NonEditable,
                                name_buffer: name_buf.clone(),
                                value_buffer: value_buf.clone(),
                                original_name: name_buf,
                                original_value: value_buf,
                                duplicate_groups: Vec::new(), // Phase 2: Empty for now, Phase 6 will populate
                                current_group_idx: None,
                                focus_on_value: false,
                            };
                            self.status_message = Some("Returned to editing".to_string());
                            return;
                        }
                        _ => {}
                    }
                }
                KeyCode::Esc => {
                    // Esc = same as "Return to editing" (button 2)
                    *selected_button = 2;
                    // Trigger the Enter handler by falling through
                    // Actually, just duplicate the logic here
                    let tracks_clone = tracks.clone();
                    let tag_fields_clone = tag_fields.clone();
                    let original_tag_fields_clone = original_tag_fields.clone();
                    let prev_track = previous_track_idx;
                    let prev_field = previous_field_idx;

                    let name_buf = tag_fields_clone[prev_track][prev_field].name.clone();
                    let value_buf = tag_fields_clone[prev_track][prev_field].value.clone();

                    self.current_view = MenuState::TagEditor {
                        tracks: tracks_clone,
                        tag_fields: tag_fields_clone.clone(),
                        original_tag_fields: original_tag_fields_clone,
                        current_track_idx: prev_track,
                        current_field_idx: prev_field,
                        field_edit_state: FieldEditState::NonEditable,
                        name_buffer: name_buf.clone(),
                        value_buffer: value_buf.clone(),
                        original_name: name_buf,
                        original_value: value_buf,
                        duplicate_groups: Vec::new(), // Phase 2: Empty for now, Phase 6 will populate
                        current_group_idx: None,
                        focus_on_value: false,
                    };
                    self.status_message = Some("Returned to editing".to_string());
                    return;
                }
                _ => {}
            }
        }

        // Phase 4: Handle ChangePreviewModal keyboard input
        if let MenuState::ChangePreviewModal {
            ref tracks,
            ref mut scroll_offset,
            save_and_next,
            ref duplicate_groups,
            current_group_idx,
            ..
        } = self.current_view
        {
            match key.code {
                KeyCode::Up => {
                    *scroll_offset = scroll_offset.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    // Max will be clamped in rendering based on content length
                    *scroll_offset += 1;
                    return;
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Phase 6: Clone needed data before mutable borrows
                    let duplicate_groups_clone = duplicate_groups.clone();
                    let current_group_idx_val = current_group_idx;
                    let save_and_next_val = save_and_next;

                    // Confirmed - actually save now
                    self.save_all_tags(tracks.clone());

                    if save_and_next_val {
                        // Phase 6: Mark current group as resolved and load next group
                        if let Some(current_idx) = current_group_idx_val {
                            if current_idx < duplicate_groups_clone.len() {
                                let current_group_id = duplicate_groups_clone[current_idx].group_id;

                                // Mark current group as resolved in database
                                match config::get_db_path() {
                                    Ok(db_path) => {
                                        match Database::open(&db_path) {
                                            Ok(db) => {
                                                if let Err(e) = db
                                                    .mark_duplicate_group_resolved(current_group_id)
                                                {
                                                    self.status_message = Some(format!("Warning: Failed to mark group as resolved: {}", e));
                                                }

                                                // Try to load next unresolved group
                                                let next_idx = current_idx + 1;
                                                if next_idx < duplicate_groups_clone.len() {
                                                    let next_group_id =
                                                        duplicate_groups_clone[next_idx].group_id;

                                                    match db
                                                        .get_duplicate_group_tracks(next_group_id)
                                                    {
                                                        Ok(next_tracks) => {
                                                            if !next_tracks.is_empty() {
                                                                // Load tag fields for next group
                                                                let next_tag_fields: Vec<
                                                                    Vec<TagField>,
                                                                > = next_tracks
                                                                    .iter()
                                                                    .map(track_to_tag_fields)
                                                                    .collect();

                                                                let next_original_tag_fields =
                                                                    next_tag_fields.clone();

                                                                // Transition back to TagEditor with next group
                                                                self.current_view = MenuState::TagEditor {
                                                                    tracks: next_tracks,
                                                                    tag_fields: next_tag_fields.clone(),
                                                                    original_tag_fields: next_original_tag_fields,
                                                                    current_track_idx: 0,
                                                                    current_field_idx: 0,
                                                                    field_edit_state: FieldEditState::NonEditable,
                                                                    name_buffer: String::new(),
                                                                    value_buffer: String::new(),
                                                                    original_name: String::new(),
                                                                    original_value: String::new(),
                                                                    duplicate_groups: duplicate_groups_clone.clone(),
                                                                    current_group_idx: Some(next_idx),
                                                                    focus_on_value: false,
                                                                };
                                                                self.status_message = Some(format!(
                                                                    "Saved. Group {}/{} - {} tracks",
                                                                    next_idx + 1,
                                                                    duplicate_groups_clone.len(),
                                                                    next_tag_fields.len()
                                                                ));
                                                                return;
                                                            }
                                                        }
                                                        Err(e) => {
                                                            self.status_message = Some(format!(
                                                                "Error loading next group: {}",
                                                                e
                                                            ));
                                                        }
                                                    }
                                                } else {
                                                    // No more groups - all done!
                                                    self.status_message = Some(
                                                        "All duplicate groups resolved!"
                                                            .to_string(),
                                                    );
                                                    self.current_view = MenuState::MainMenu;
                                                    return;
                                                }
                                            }
                                            Err(e) => {
                                                self.status_message =
                                                    Some(format!("Database error: {}", e));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        self.status_message = Some(format!("Config error: {}", e));
                                    }
                                }
                            }
                        }
                    } else {
                        self.status_message = Some("Tags saved successfully".to_string());
                    }

                    self.current_view = MenuState::MainMenu;
                    return;
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    // Cancel - return to main menu without saving
                    self.status_message = Some("Save cancelled".to_string());
                    self.current_view = MenuState::MainMenu;
                    return;
                }
                KeyCode::PageUp => {
                    *scroll_offset = scroll_offset.saturating_sub(10);
                    return;
                }
                KeyCode::PageDown => {
                    *scroll_offset += 10;
                    return;
                }
                _ => {}
            }
        }

        // 5. NAVIGATION KEY HANDLERS (commit before moving)
        // Now handle the normal key commands wrapped in match
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                // Only quit from main menu
                if matches!(self.current_view, MenuState::MainMenu) {
                    self.should_quit = true;
                }
            }
            KeyCode::Esc => {
                // Go back to previous menu or commit editing
                match &mut self.current_view {
                    MenuState::ScanSourceMenu { .. }
                    | MenuState::ReportsMenu
                    | MenuState::DeployMenu
                    | MenuState::CorpusTriageMenu => {
                        self.current_view = MenuState::MainMenu;
                        self.menu_state.select(Some(0));
                        self.status_message = None;
                    }
                    MenuState::DuplicateGroupList { .. } => {
                        self.current_view = MenuState::MainMenu;
                        self.menu_state.select(Some(0));
                    }
                    MenuState::DuplicateGroupDetail { .. } => {
                        // Go back to duplicate list - for MVP, just go to main menu
                        self.current_view = MenuState::MainMenu;
                        self.menu_state.select(Some(0));
                    }
                    MenuState::TagEditor {
                        ref mut field_edit_state,
                        ref mut tag_fields,
                        ref name_buffer,
                        ref value_buffer,
                        ref original_name,
                        ref original_value,
                        current_track_idx,
                        current_field_idx,
                        ..
                    } => {
                        // If editing, commit and go to NonEditable
                        match field_edit_state {
                            FieldEditState::EditingName => {
                                if name_buffer != original_name {
                                    tag_fields[*current_track_idx][*current_field_idx].name =
                                        name_buffer.clone();
                                }
                                *field_edit_state = FieldEditState::NonEditable;
                            }
                            FieldEditState::EditingValue => {
                                if value_buffer != original_value {
                                    tag_fields[*current_track_idx][*current_field_idx].value =
                                        value_buffer.clone();
                                }
                                *field_edit_state = FieldEditState::NonEditable;
                            }
                            FieldEditState::NonEditable => {
                                // Exit tag editor, go back to main menu
                                self.current_view = MenuState::MainMenu;
                                self.menu_state.select(Some(0));
                            }
                        }
                    }
                    MenuState::MainMenu => {}
                    MenuState::SaveConfirmationModal { .. } => {
                        // Phase 2: Esc already handled in modal's own handler above
                        // This case should never be reached due to early return
                    }
                    MenuState::ChangePreviewModal { .. } => {
                        // Phase 4: Esc already handled in modal's own handler above
                        // This case should never be reached due to early return
                    }
                    MenuState::FingerprintDedupDirSelect { .. } => {
                        // Cancel and return to corpus triage menu
                        self.current_view = MenuState::CorpusTriageMenu;
                        self.menu_state.select(Some(0));
                        self.status_message = None;
                    }
                    MenuState::FingerprintDedupResolve { .. } => {
                        // Cancel remaining conflicts and show stats
                        // Handler will be in the Enter key section
                    }
                    MenuState::FingerprintDedupStats { .. } => {
                        // Return to main menu
                        self.current_view = MenuState::MainMenu;
                        self.menu_state.select(Some(0));
                    }
                }
            }
            KeyCode::Down => {
                // TagEditor: Commit current buffer → move → load new field → stay in edit mode
                if let MenuState::TagEditor {
                    ref mut current_field_idx,
                    ref mut tag_fields,
                    current_track_idx,
                    ref mut field_edit_state,
                    ref mut name_buffer,
                    ref mut value_buffer,
                    ref mut original_name,
                    ref mut original_value,
                    ..
                } = self.current_view
                {
                    // Commit current buffer if changed
                    let was_editing_name = matches!(field_edit_state, FieldEditState::EditingName);
                    let was_editing_value =
                        matches!(field_edit_state, FieldEditState::EditingValue);

                    match field_edit_state {
                        FieldEditState::EditingName => {
                            if name_buffer != original_name {
                                tag_fields[current_track_idx][*current_field_idx].name =
                                    name_buffer.clone();
                            }
                        }
                        FieldEditState::EditingValue => {
                            if value_buffer != original_value {
                                tag_fields[current_track_idx][*current_field_idx].value =
                                    value_buffer.clone();
                            }
                        }
                        FieldEditState::NonEditable => {}
                    }

                    // Move to next field
                    let max_fields = tag_fields[current_track_idx].len();
                    if *current_field_idx < max_fields.saturating_sub(1) {
                        *current_field_idx += 1;

                        // Load new field values and stay in edit mode if we were editing
                        let field = &tag_fields[current_track_idx][*current_field_idx];
                        *original_name = field.name.clone();
                        *original_value = field.value.clone();

                        // Stay in edit mode for rapid tagging workflow
                        if was_editing_name {
                            *name_buffer = field.name.clone();
                            *field_edit_state = FieldEditState::EditingName;
                        } else if was_editing_value {
                            *value_buffer = field.value.clone();
                            *field_edit_state = FieldEditState::EditingValue;
                        } else {
                            *field_edit_state = FieldEditState::NonEditable;
                        }
                    }
                } else {
                    self.next();
                }
            }
            KeyCode::Up => {
                // TagEditor: Commit current buffer → move → load new field → stay in edit mode
                if let MenuState::TagEditor {
                    ref mut current_field_idx,
                    ref mut tag_fields,
                    current_track_idx,
                    ref mut field_edit_state,
                    ref mut name_buffer,
                    ref mut value_buffer,
                    ref mut original_name,
                    ref mut original_value,
                    ..
                } = self.current_view
                {
                    // Commit current buffer if changed
                    let was_editing_name = matches!(field_edit_state, FieldEditState::EditingName);
                    let was_editing_value =
                        matches!(field_edit_state, FieldEditState::EditingValue);

                    match field_edit_state {
                        FieldEditState::EditingName => {
                            if name_buffer != original_name {
                                tag_fields[current_track_idx][*current_field_idx].name =
                                    name_buffer.clone();
                            }
                        }
                        FieldEditState::EditingValue => {
                            if value_buffer != original_value {
                                tag_fields[current_track_idx][*current_field_idx].value =
                                    value_buffer.clone();
                            }
                        }
                        FieldEditState::NonEditable => {}
                    }

                    // Move to previous field
                    if *current_field_idx > 0 {
                        *current_field_idx -= 1;

                        // Load new field values and stay in edit mode if we were editing
                        let field = &tag_fields[current_track_idx][*current_field_idx];
                        *original_name = field.name.clone();
                        *original_value = field.value.clone();

                        // Stay in edit mode for rapid tagging workflow
                        if was_editing_name {
                            *name_buffer = field.name.clone();
                            *field_edit_state = FieldEditState::EditingName;
                        } else if was_editing_value {
                            *value_buffer = field.value.clone();
                            *field_edit_state = FieldEditState::EditingValue;
                        } else {
                            *field_edit_state = FieldEditState::NonEditable;
                        }
                    }
                } else {
                    self.previous();
                }
            }
            KeyCode::Left => {
                // TagEditor: Toggle focus to name (keep current edit state)
                if let MenuState::TagEditor {
                    ref mut focus_on_value,
                    ..
                } = self.current_view
                {
                    *focus_on_value = false;
                }
            }
            KeyCode::Right => {
                // TagEditor: Toggle focus to value (keep current edit state)
                if let MenuState::TagEditor {
                    ref mut focus_on_value,
                    ..
                } = self.current_view
                {
                    *focus_on_value = true;
                }
            }
            KeyCode::Tab => {
                // TagEditor: Commit both buffers → move to next track → load buffers → stay NonEditable
                if let MenuState::TagEditor {
                    ref mut current_track_idx,
                    ref tracks,
                    ref mut tag_fields,
                    ref original_tag_fields,
                    ref mut current_field_idx,
                    ref mut field_edit_state,
                    ref mut name_buffer,
                    ref mut value_buffer,
                    ref mut original_name,
                    ref mut original_value,
                    ref duplicate_groups,
                    current_group_idx,
                    // show_write_changes_button removed
                    ..
                } = self.current_view
                {
                    // Phase 1: STEP 1 - Clamp field position if on "New Tag" (prevents crash)
                    let current_field = &tag_fields[*current_track_idx][*current_field_idx];
                    if current_field.name == "New Tag" {
                        *current_field_idx = 0; // Move to first guaranteed field
                    }

                    // Phase 1: STEP 2 - Commit both buffers to in-memory tag_fields BEFORE moving
                    if name_buffer != original_name {
                        tag_fields[*current_track_idx][*current_field_idx].name =
                            name_buffer.clone();
                    }
                    if value_buffer != original_value {
                        tag_fields[*current_track_idx][*current_field_idx].value =
                            value_buffer.clone();
                    }

                    // Phase 1: STEP 3 - Move to next track
                    if *current_track_idx < tracks.len().saturating_sub(1) {
                        *current_track_idx += 1;

                        // Phase 1: STEP 4 - Clamp field position to valid range for new track
                        let max_field_idx = tag_fields[*current_track_idx].len().saturating_sub(1);
                        if *current_field_idx > max_field_idx {
                            *current_field_idx = 0; // Reset to first field if out of bounds
                        }

                        // Phase 1: STEP 5 - Load buffers from NEW track's field (FIX THE BUG!)
                        let field = &tag_fields[*current_track_idx][*current_field_idx];
                        *original_name = field.name.clone();
                        *original_value = field.value.clone();
                        *name_buffer = field.name.clone(); // FIX: Load from field, not empty!
                        *value_buffer = field.value.clone(); // FIX: Load from field, not empty!

                        *field_edit_state = FieldEditState::NonEditable;
                    } else {
                        // Phase 2: After last track, enter save confirmation modal
                        self.current_view = MenuState::SaveConfirmationModal {
                            tracks: tracks.clone(),
                            tag_fields: tag_fields.clone(),
                            original_tag_fields: original_tag_fields.clone(),
                            selected_button: 2, // Default to "Return to editing"
                            previous_track_idx: *current_track_idx,
                            previous_field_idx: *current_field_idx,
                            duplicate_groups: duplicate_groups.clone(),
                            current_group_idx,
                        };
                    }
                }
            }
            KeyCode::BackTab => {
                // TagEditor: Commit both buffers → move to previous track → load buffers → stay NonEditable
                if let MenuState::TagEditor {
                    ref mut current_track_idx,
                    ref mut tag_fields,
                    ref mut current_field_idx,
                    ref mut field_edit_state,
                    ref mut name_buffer,
                    ref mut value_buffer,
                    ref mut original_name,
                    ref mut original_value,
                    ..
                } = self.current_view
                {
                    // Phase 1: STEP 1 - Clamp field position if on "New Tag" (prevents crash)
                    let current_field = &tag_fields[*current_track_idx][*current_field_idx];
                    if current_field.name == "New Tag" {
                        *current_field_idx = 0; // Move to first guaranteed field
                    }

                    // Phase 1: STEP 2 - Commit both buffers to in-memory tag_fields BEFORE moving
                    if name_buffer != original_name {
                        tag_fields[*current_track_idx][*current_field_idx].name =
                            name_buffer.clone();
                    }
                    if value_buffer != original_value {
                        tag_fields[*current_track_idx][*current_field_idx].value =
                            value_buffer.clone();
                    }

                    // Phase 1: STEP 3 - Move to previous track
                    if *current_track_idx > 0 {
                        *current_track_idx -= 1;

                        // Phase 1: STEP 4 - Clamp field position to valid range for new track
                        let max_field_idx = tag_fields[*current_track_idx].len().saturating_sub(1);
                        if *current_field_idx > max_field_idx {
                            *current_field_idx = 0; // Reset to first field if out of bounds
                        }

                        // Phase 1: STEP 5 - Load buffers from NEW track's field (FIX THE BUG!)
                        let field = &tag_fields[*current_track_idx][*current_field_idx];
                        *original_name = field.name.clone();
                        *original_value = field.value.clone();
                        *name_buffer = field.name.clone(); // FIX: Load from field, not empty!
                        *value_buffer = field.value.clone(); // FIX: Load from field, not empty!

                        *field_edit_state = FieldEditState::NonEditable;
                    }
                }
            }
            KeyCode::Enter => {
                // 6. ENTER KEY: Toggle edit state (don't move)
                if let MenuState::TagEditor {
                    ref mut tag_fields,
                    current_track_idx,
                    current_field_idx,
                    ref mut field_edit_state,
                    ref mut name_buffer,
                    ref mut value_buffer,
                    ref mut original_name,
                    ref mut original_value,
                    focus_on_value,
                    ..
                } = self.current_view
                {
                    let current_field = &tag_fields[current_track_idx][current_field_idx];

                    // Special handling for "New Tag" line
                    if current_field.name == "New Tag" {
                        // Create new tag for all tracks with empty name/value
                        for track_fields in tag_fields.iter_mut() {
                            // Insert before "New Tag" line
                            let new_tag_idx = track_fields
                                .iter()
                                .position(|f| f.name == "New Tag")
                                .unwrap();
                            track_fields.insert(
                                new_tag_idx,
                                TagField {
                                    name: String::new(),
                                    value: String::new(),
                                    editable: true,
                                    is_unique_per_track: false,
                                },
                            );
                        }
                        // Position stays on the newly created tag (which is now at current_field_idx)
                        // Start editing the name
                        *name_buffer = String::new();
                        *value_buffer = String::new();
                        *original_name = String::new();
                        *original_value = String::new();
                        *field_edit_state = FieldEditState::EditingName;
                        self.status_message = Some(
                            "New tag created. Enter name, then toggle to value with →".to_string(),
                        );
                        return;
                    }

                    // Toggle edit state based on focus
                    if focus_on_value {
                        // Operating on value
                        match field_edit_state {
                            FieldEditState::NonEditable => {
                                // Start editing value
                                *value_buffer = current_field.value.clone();
                                *original_value = current_field.value.clone();
                                *field_edit_state = FieldEditState::EditingValue;
                            }
                            FieldEditState::EditingValue => {
                                // Commit value and go to NonEditable
                                if value_buffer != original_value {
                                    tag_fields[current_track_idx][current_field_idx].value =
                                        value_buffer.clone();
                                    self.status_message =
                                        Some("Value updated (W to save to file)".to_string());
                                }
                                *field_edit_state = FieldEditState::NonEditable;
                            }
                            FieldEditState::EditingName => {
                                // Shouldn't happen, but commit name and switch to editing value
                                if name_buffer != original_name {
                                    tag_fields[current_track_idx][current_field_idx].name =
                                        name_buffer.clone();
                                }
                                *value_buffer = tag_fields[current_track_idx][current_field_idx]
                                    .value
                                    .clone();
                                *original_value = value_buffer.clone();
                                *field_edit_state = FieldEditState::EditingValue;
                            }
                        }
                    } else {
                        // Operating on name
                        match field_edit_state {
                            FieldEditState::NonEditable => {
                                // Start editing name
                                *name_buffer = current_field.name.clone();
                                *original_name = current_field.name.clone();
                                *field_edit_state = FieldEditState::EditingName;
                            }
                            FieldEditState::EditingName => {
                                // Commit name and go to NonEditable
                                if name_buffer != original_name {
                                    tag_fields[current_track_idx][current_field_idx].name =
                                        name_buffer.clone();
                                    self.status_message =
                                        Some("Name updated (W to save to file)".to_string());
                                }
                                *field_edit_state = FieldEditState::NonEditable;
                            }
                            FieldEditState::EditingValue => {
                                // Shouldn't happen, but commit value and switch to editing name
                                if value_buffer != original_value {
                                    tag_fields[current_track_idx][current_field_idx].value =
                                        value_buffer.clone();
                                }
                                *name_buffer = tag_fields[current_track_idx][current_field_idx]
                                    .name
                                    .clone();
                                *original_name = name_buffer.clone();
                                *field_edit_state = FieldEditState::EditingName;
                            }
                        }
                    }
                } else {
                    self.execute_action();
                }
            }
            _ => {}
        }
    }

    fn execute_action(&mut self) {
        match &self.current_view {
            MenuState::MainMenu => {
                if let Some(selected) = self.menu_state.selected() {
                    let action = self.menu_items[selected].action.clone();
                    self.status_message = Some(self.perform_action(action));
                }
            }
            MenuState::ScanSourceMenu { sources } => {
                if let Some(selected) = self.menu_state.selected() {
                    if selected == 0 {
                        // Scan All (normal)
                        self.start_scan_all(sources.clone(), false);
                        self.current_view = MenuState::MainMenu;
                    } else if selected == 1 {
                        // Clean Rescan All (clear cache)
                        self.start_scan_all(sources.clone(), true);
                        self.current_view = MenuState::MainMenu;
                    } else if selected <= sources.len() + 1 {
                        // Scan individual source
                        let source = sources[selected - 2].clone();
                        self.start_scan(source);
                        self.current_view = MenuState::MainMenu;
                    }
                }
            }
            MenuState::ReportsMenu => {
                if let Some(selected) = self.menu_state.selected() {
                    let report_type = match selected {
                        0 => Some(ReportType::GenerateAll),
                        1 => Some(ReportType::Legacy),
                        2 => Some(ReportType::Deployment),
                        3 => Some(ReportType::Quality),
                        4 => Some(ReportType::Duplicates),
                        _ => None,
                    };

                    if let Some(rtype) = report_type {
                        self.start_report(rtype);
                        self.current_view = MenuState::MainMenu;
                    }
                }
            }
            MenuState::DeployMenu => {
                if let Some(selected) = self.menu_state.selected() {
                    match selected {
                        0 => {
                            // Dry Run
                            if let Some(cfg) = &self.config {
                                self.show_dry_run_report(cfg.clone());
                                self.current_view = MenuState::MainMenu;
                            }
                        }
                        1 => {
                            // Full Deploy
                            if let Some(cfg) = &self.config {
                                self.start_deployment(cfg.clone(), false);
                                self.current_view = MenuState::MainMenu;
                            }
                        }
                        _ => {}
                    }
                }
            }
            MenuState::CorpusTriageMenu => {
                if let Some(selected) = self.menu_state.selected() {
                    match selected {
                        0 => {
                            // Metadata Deduplication
                            let msg = self.handle_metadata_deduplication();
                            self.status_message = Some(msg);
                        }
                        1 => {
                            // Fingerprint Deduplication - Pre-flight check
                            if let Some(cfg) = &self.config {
                                if cfg.lost_files_dir.is_none() {
                                    self.status_message = Some(
                                        "ERROR: lost-files directory must be configured in config.kdl.\nAdd: lost-files \"/path/to/lost+found\"".to_string(),
                                    );
                                } else {
                                    // Launch directory selection UI
                                    self.current_view = MenuState::FingerprintDedupDirSelect {
                                        selected_dirs: Vec::new(),
                                        current_input: String::new(),
                                        error_message: None,
                                    };
                                    self.status_message = Some(
                                        "Enter corpus directory paths (Enter on empty to finish)".to_string(),
                                    );
                                }
                            } else {
                                self.status_message = Some("ERROR: Config not loaded".to_string());
                            }
                        }
                        2 => {
                            // Back
                            self.current_view = MenuState::MainMenu;
                            self.menu_state.select(Some(0));
                        }
                        _ => {}
                    }
                }
            }
            MenuState::DuplicateGroupList { .. } => {
                // Enter key opens group detail - handled in handle_key
            }
            MenuState::DuplicateGroupDetail { .. } => {
                // Enter key opens tag editor or other actions - handled in handle_key
            }
            MenuState::TagEditor { .. } => {
                // Enter key starts/commits editing - handled in handle_key
            }
            MenuState::SaveConfirmationModal { .. } => {
                // Phase 2: Enter key handled in modal's own handler
            }
            MenuState::ChangePreviewModal { .. } => {
                // Phase 4: Enter key handled in modal's own handler
            }
            MenuState::FingerprintDedupDirSelect { .. } => {
                // Enter key handled in handle_key
            }
            MenuState::FingerprintDedupResolve { .. } => {
                // Enter key handled in handle_key
            }
            MenuState::FingerprintDedupStats { .. } => {
                // Enter key returns to main menu
                self.current_view = MenuState::MainMenu;
                self.menu_state.select(Some(0));
            }
        }
    }

    fn perform_action(&mut self, action: MenuAction) -> String {
        match action {
            MenuAction::ScanSources => match &self.config {
                Some(cfg) => {
                    let sources = cfg.get_scan_sources();
                    self.current_view = MenuState::ScanSourceMenu { sources };
                    self.menu_state.select(Some(0));
                    "Select source to scan (ESC to go back)".to_string()
                }
                None => "Error: config.kdl not found in $XDG_CONFIG_HOME/mla/ (or ~/.config/mla/)"
                    .to_string(),
            },
            MenuAction::Reports => {
                self.current_view = MenuState::ReportsMenu;
                self.menu_state.select(Some(0));
                "Select report to generate (ESC to go back)".to_string()
            }
            MenuAction::Deploy => {
                match &self.config {
                    Some(cfg) => {
                        // Check if deploy_mappings configured
                        if cfg.deploy_mappings.is_empty() {
                            "Error: No deploy mappings configured in config.kdl. Please add deploy mappings.".to_string()
                        } else {
                            self.current_view = MenuState::DeployMenu;
                            self.menu_state.select(Some(0)); // Select Dry Run by default
                            "Select deployment mode (ESC to go back)".to_string()
                        }
                    }
                    None => {
                        "Error: config.kdl not found in $XDG_CONFIG_HOME/mla/ (or ~/.config/mla/)"
                            .to_string()
                    }
                }
            }
            MenuAction::CorpusTriage => {
                self.current_view = MenuState::CorpusTriageMenu;
                self.menu_state.select(Some(0));
                "Select corpus triage operation (ESC to go back)".to_string()
            }
            MenuAction::Quit => std::process::exit(0),
        }
    }

    // Handler for old duplicate resolution logic - now called from CorpusTriageMenu
    fn handle_metadata_deduplication(&mut self) -> String {
        // Phase 6: Load unresolved duplicate groups from database
                match config::get_db_path() {
                    Ok(db_path) => {
                        match Database::open(&db_path) {
                            Ok(db) => {
                                // Get all unresolved duplicate group IDs
                                match db.get_unresolved_duplicate_groups() {
                                    Ok(group_ids) => {
                                        if group_ids.is_empty() {
                                            return "No unresolved duplicate groups found. Run duplicate detection first.".to_string();
                                        }

                                        // Load tracks for the first group
                                        match db.get_duplicate_group_tracks(group_ids[0]) {
                                            Ok(first_group_tracks) => {
                                                if first_group_tracks.is_empty() {
                                                    return format!(
                                                        "No tracks found for duplicate group {}",
                                                        group_ids[0]
                                                    );
                                                }

                                                // Build duplicate group info list
                                                let duplicate_groups: Vec<DuplicateGroupInfo> =
                                                    group_ids
                                                        .iter()
                                                        .map(|&group_id| {
                                                            DuplicateGroupInfo {
                                                                group_id,
                                                                tracks: Vec::new(), // Tracks loaded on-demand
                                                                resolved: false,
                                                            }
                                                        })
                                                        .collect();

                                                let demo_tracks = first_group_tracks;
                                                if demo_tracks.is_empty() {
                                                    "No tracks in first duplicate group".to_string()
                                                } else {
                                                    // Load tag fields for each track
                                                    let tag_fields: Vec<Vec<TagField>> =
                                                        demo_tracks
                                                            .iter()
                                                            .map(track_to_tag_fields)
                                                            .collect();

                                                    // Phase 3: Deep clone for original state (enables change preview)
                                                    let original_tag_fields = tag_fields.clone();

                                                    // Auto-load first field for immediate editing
                                                    let initial_buffer = if !tag_fields.is_empty()
                                                        && !tag_fields[0].is_empty()
                                                    {
                                                        tag_fields[0][0].value.clone()
                                                    } else {
                                                        String::new()
                                                    };
                                                    let initial_name = if !tag_fields.is_empty()
                                                        && !tag_fields[0].is_empty()
                                                    {
                                                        tag_fields[0][0].name.clone()
                                                    } else {
                                                        String::new()
                                                    };

                                                    // Phase 6: Compute lengths before moving values
                                                    let num_tracks = demo_tracks.len();
                                                    let num_groups = duplicate_groups.len();

                                                    self.current_view = MenuState::TagEditor {
                                                        tracks: demo_tracks,
                                                        tag_fields,
                                                        original_tag_fields, // Phase 3: Store original
                                                        current_track_idx: 0,
                                                        current_field_idx: 0,

                                                        // New edit state fields
                                                        field_edit_state:
                                                            FieldEditState::NonEditable,
                                                        name_buffer: String::new(),
                                                        value_buffer: initial_buffer.clone(),
                                                        original_name: initial_name.clone(),
                                                        original_value: initial_buffer.clone(),

                                                        // Phase 6: Duplicate workflow fields
                                                        duplicate_groups: duplicate_groups.clone(),
                                                        current_group_idx: Some(0),
                                                        // show_write_changes_button removed - Phase 0
                                                        focus_on_value: true,
                                                    };
                                                    format!("Tag Editor: Duplicate group 1/{} - {} tracks", num_groups, num_tracks)
                                                }
                                            }
                                            Err(e) => {
                                                format!("Error loading tracks for group: {}", e)
                                            }
                                        }
                                    }
                                    Err(e) => format!("Error loading duplicate groups: {}", e),
                                }
                            }
                            Err(e) => format!("Error opening database: {}", e),
                        }
                    }
                    Err(e) => format!("Error getting database path: {}", e),
                }
    }

    fn start_scan(&mut self, source: ScanSource) {
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let path = source.path.clone();
        let source_id = source.source_id.clone();
        let cancel_flag_clone = cancel_flag.clone();

        // Spawn background scan thread
        std::thread::spawn(move || {
            let result = scanner::scan_directory_with_progress(
                &path,
                &source_id,
                Some(tx),
                cancel_flag_clone,
            );

            // If there was an error, log it
            if let Err(e) = result {
                let error_msg = format!("Scan error: {}", e);
                let _ = config::log_scan_error(&error_msg);
            }
        });

        // Set up operation state
        self.operation_in_progress = Some(OperationState {
            operation_type: OperationType::Scanning {
                source_name: source.name.clone(),
            },
            progress: ScanProgress {
                total_bytes: 0,
                bytes_processed: 0,
                files_processed: 0,
                current_file: None,
                errors: 0,
                start_time: std::time::Instant::now(),
            },
            cancel_flag,
        });
        self.operation_receiver = Some(rx);
        self.status_message = Some(format!("Scanning {}...", source.name));
    }

    fn start_scan_all(&mut self, sources: Vec<ScanSource>, clean_rescan: bool) {
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag_clone = cancel_flag.clone();

        // Spawn background thread to scan all sources sequentially
        std::thread::spawn(move || {
            // If clean rescan, clear database and scan state first
            if clean_rescan {
                let _ = tx.send(ScanMessage::Progress(ScanProgress {
                    total_bytes: 0,
                    bytes_processed: 0,
                    files_processed: 0,
                    current_file: Some("Clearing database...".to_string()),
                    errors: 0,
                    start_time: std::time::Instant::now(),
                }));

                if let Ok(db_path) = config::get_db_path() {
                    if let Ok(db) = Database::open(&db_path) {
                        // Clear all sources
                        for source in &sources {
                            let _ = db.clear_source(&source.source_id);
                            let _ = db.clear_scan_state(&source.source_id);
                        }
                    }
                }
            }

            for source in sources {
                // Check if cancelled
                if cancel_flag_clone.load(Ordering::Relaxed) {
                    let _ = tx.send(ScanMessage::Error("Scan cancelled by user".to_string()));
                    return;
                }

                // Scan this source
                let result = scanner::scan_directory_with_progress(
                    &source.path,
                    &source.source_id,
                    Some(tx.clone()),
                    cancel_flag_clone.clone(),
                );

                if let Err(e) = result {
                    let error_msg = format!("Scan error for {}: {}", source.name, e);
                    let _ = config::log_scan_error(&error_msg);
                    // Continue to next source even if one fails
                }
            }
        });

        // Set up operation state
        let status_msg = if clean_rescan {
            "Clean rescanning all sources (clearing cache)..."
        } else {
            "Scanning all sources..."
        };

        self.operation_in_progress = Some(OperationState {
            operation_type: OperationType::Scanning {
                source_name: "All Sources".to_string(),
            },
            progress: ScanProgress {
                total_bytes: 0,
                bytes_processed: 0,
                files_processed: 0,
                current_file: None,
                errors: 0,
                start_time: std::time::Instant::now(),
            },
            cancel_flag,
        });
        self.operation_receiver = Some(rx);
        self.status_message = Some(status_msg.to_string());
    }

    fn start_report(&mut self, report_type: ReportType) {
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag_clone = cancel_flag.clone();

        // Special handling for GenerateAll
        if matches!(report_type, ReportType::GenerateAll) {
            // Spawn background thread to generate all reports sequentially
            std::thread::spawn(move || {
                let start_time = std::time::Instant::now();
                let mut summaries = Vec::new();
                let mut errors = Vec::new();

                let reports = [
                    (
                        "/tmp/mla-legacy_matches.txt",
                        "Legacy Library Report",
                        reports::generate_legacy_report as fn(&std::path::Path) -> Result<String>,
                    ),
                    (
                        "/tmp/mla-deployment_status.txt",
                        "Deployment Status Report",
                        reports::generate_deployment_report,
                    ),
                    (
                        "/tmp/mla-quality_issues.txt",
                        "Quality Report",
                        reports::generate_quality_report,
                    ),
                    (
                        "/tmp/mla-duplicates.txt",
                        "Duplicate Detection Report",
                        reports::generate_duplicate_report,
                    ),
                ];

                for (i, (filename, report_name, generator)) in reports.iter().enumerate() {
                    // Check if cancelled
                    if cancel_flag_clone.load(Ordering::Relaxed) {
                        let _ = tx.send(ScanMessage::Error(
                            "Report generation cancelled by user".to_string(),
                        ));
                        return;
                    }

                    // Send progress
                    let _ = tx.send(ScanMessage::Progress(ScanProgress {
                        total_bytes: 0,
                        bytes_processed: 0,
                        files_processed: 0,
                        current_file: Some(format!("Generating {} ({}/4)...", report_name, i + 1)),
                        errors: 0,
                        start_time,
                    }));

                    // Generate report
                    let output_path = PathBuf::from(filename);
                    match generator(&output_path) {
                        Ok(summary) => summaries.push(format!("{}: {}", report_name, summary)),
                        Err(e) => errors.push(format!("{}: Error - {}", report_name, e)),
                    }
                }

                // Send completion
                let _ = tx.send(ScanMessage::Complete(crate::progress::ScanResult {
                    files_scanned: 0,
                    files_skipped: 0,
                    bytes_scanned: 0,
                    errors: errors.len(),
                    duration: start_time.elapsed(),
                }));

                // Send summary
                let mut final_summary =
                    format!("Generated {} reports successfully", summaries.len());
                if !errors.is_empty() {
                    final_summary.push_str(&format!(", {} errors", errors.len()));
                }
                final_summary.push_str(":\n");
                for s in &summaries {
                    final_summary.push_str(&format!("  - {}\n", s));
                }
                for e in &errors {
                    final_summary.push_str(&format!("  - {}\n", e));
                }
                let _ = tx.send(ScanMessage::Error(final_summary));
            });

            // Set up operation state
            self.operation_in_progress = Some(OperationState {
                operation_type: OperationType::GeneratingReport {
                    report_type: "All Reports".to_string(),
                },
                progress: ScanProgress {
                    total_bytes: 0,
                    bytes_processed: 0,
                    files_processed: 0,
                    current_file: Some("Starting...".to_string()),
                    errors: 0,
                    start_time: std::time::Instant::now(),
                },
                cancel_flag,
            });
            self.operation_receiver = Some(rx);
            self.status_message = Some("Generating all reports...".to_string());
            return;
        }

        // Single report generation
        let (filename, report_name, generator): (
            String,
            String,
            fn(&std::path::Path) -> Result<String>,
        ) = match report_type {
            ReportType::GenerateAll => unreachable!(), // Handled above
            ReportType::Legacy => (
                "/tmp/mla-legacy_matches.txt".to_string(),
                "Legacy Library Report".to_string(),
                reports::generate_legacy_report,
            ),
            ReportType::Deployment => (
                "/tmp/mla-deployment_status.txt".to_string(),
                "Deployment Status Report".to_string(),
                reports::generate_deployment_report,
            ),
            ReportType::Quality => (
                "/tmp/mla-quality_issues.txt".to_string(),
                "Quality Report".to_string(),
                reports::generate_quality_report,
            ),
            ReportType::Duplicates => (
                "/tmp/mla-duplicates.txt".to_string(),
                "Duplicate Detection Report".to_string(),
                reports::generate_duplicate_report,
            ),
        };

        // Spawn background report thread
        std::thread::spawn(move || {
            let start_time = std::time::Instant::now();

            // Send initial progress
            let _ = tx.send(ScanMessage::Progress(ScanProgress {
                total_bytes: 0,
                bytes_processed: 0,
                files_processed: 0,
                current_file: Some("Generating report...".to_string()),
                errors: 0,
                start_time,
            }));

            // Generate report
            let output_path = PathBuf::from(&filename);
            let result = generator(&output_path);

            // Send completion or error
            match result {
                Ok(summary) => {
                    let _ = tx.send(ScanMessage::Complete(crate::progress::ScanResult {
                        files_scanned: 0,
                        files_skipped: 0,
                        bytes_scanned: 0,
                        errors: 0,
                        duration: start_time.elapsed(),
                    }));
                    let _ = tx.send(ScanMessage::Error(summary)); // Reuse Error for summary message
                }
                Err(e) => {
                    let _ = tx.send(ScanMessage::Error(format!(
                        "Error generating report: {}",
                        e
                    )));
                }
            }
        });

        // Set up operation state
        self.operation_in_progress = Some(OperationState {
            operation_type: OperationType::GeneratingReport {
                report_type: report_name.clone(),
            },
            progress: ScanProgress {
                total_bytes: 0,
                bytes_processed: 0,
                files_processed: 0,
                current_file: Some("Starting...".to_string()),
                errors: 0,
                start_time: std::time::Instant::now(),
            },
            cancel_flag,
        });
        self.operation_receiver = Some(rx);
        self.status_message = Some(format!("Generating {}...", report_name));
    }

    fn start_deployment(&mut self, config: config::Config, dry_run: bool) {
        use crate::deploy;

        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag_clone = cancel_flag.clone();

        // Spawn background deployment thread
        std::thread::spawn(move || {
            let start_time = std::time::Instant::now();

            // Open database
            let db_path = match config::get_db_path() {
                Ok(path) => path,
                Err(e) => {
                    let _ = tx.send(ScanMessage::Error(format!(
                        "Failed to get database path: {}",
                        e
                    )));
                    return;
                }
            };

            let db = match Database::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    let _ = tx.send(ScanMessage::Error(format!(
                        "Failed to open database: {}",
                        e
                    )));
                    return;
                }
            };

            // Send initial progress
            let _ = tx.send(ScanMessage::Progress(ScanProgress {
                total_bytes: 0,
                bytes_processed: 0,
                files_processed: 0,
                current_file: Some("Creating deployment plan...".to_string()),
                errors: 0,
                start_time,
            }));

            // Create deployment plans
            let plans = match deploy::create_deployment_plan(&config, &db) {
                Ok(plans) => plans,
                Err(e) => {
                    let _ = tx.send(ScanMessage::Error(format!(
                        "Failed to create deployment plan: {}",
                        e
                    )));
                    return;
                }
            };

            // Execute each plan
            let mut total_deployed = 0;
            let mut total_skipped = 0;
            let mut total_lost_moved = 0;
            let mut all_errors = Vec::new();

            for plan in plans {
                if cancel_flag_clone.load(Ordering::Relaxed) {
                    let _ = tx.send(ScanMessage::Error(
                        "Deployment cancelled by user".to_string(),
                    ));
                    return;
                }

                // Send progress update
                let _ = tx.send(ScanMessage::Progress(ScanProgress {
                    total_bytes: 0,
                    bytes_processed: 0,
                    files_processed: total_deployed,
                    current_file: Some(format!("Deploying to library: {}", plan.library_name)),
                    errors: all_errors.len(),
                    start_time,
                }));

                // Find library path
                let library = match config
                    .libraries
                    .iter()
                    .find(|l| l.name == plan.library_name)
                {
                    Some(lib) => lib,
                    None => {
                        all_errors.push(format!("Library not found: {}", plan.library_name));
                        continue;
                    }
                };

                // Execute deployment
                let result = match deploy::execute_deployment(
                    &plan,
                    &library.path,
                    config.lost_files_dir.as_deref(),
                    dry_run,
                ) {
                    Ok(result) => result,
                    Err(e) => {
                        all_errors
                            .push(format!("Deployment error for {}: {}", plan.library_name, e));
                        continue;
                    }
                };

                total_deployed += result.files_deployed;
                total_skipped += result.files_skipped;
                total_lost_moved += result.lost_files_moved;
                all_errors.extend(result.errors);

                // Log successful deployments
                if !dry_run {
                    for action in &plan.files_to_deploy {
                        if let Err(e) = db.log_deployment(
                            &plan.library_name,
                            &action.corpus_track.path,
                            &action.target_path.to_string_lossy(),
                            action.corpus_track.inode,
                        ) {
                            eprintln!("Warning: Failed to log deployment: {}", e);
                        }
                    }
                }
            }

            // Send completion
            let summary = if dry_run {
                format!(
                    "Dry run complete: {} files to deploy, {} already deployed, {} lost files to move. {} errors.",
                    total_deployed, total_skipped, total_lost_moved, all_errors.len()
                )
            } else {
                format!(
                    "Deployment complete: {} files deployed, {} skipped, {} lost files moved. {} errors.",
                    total_deployed, total_skipped, total_lost_moved, all_errors.len()
                )
            };

            if !all_errors.is_empty() {
                let error_summary = format!("{}\nErrors:\n{}", summary, all_errors.join("\n"));
                let _ = tx.send(ScanMessage::Error(error_summary));
            } else {
                let _ = tx.send(ScanMessage::Complete(crate::progress::ScanResult {
                    files_scanned: total_deployed,
                    files_skipped: 0,
                    bytes_scanned: 0,
                    errors: 0,
                    duration: start_time.elapsed(),
                }));
                let _ = tx.send(ScanMessage::Error(summary)); // Reuse Error for summary
            }
        });

        // Set up operation state
        self.operation_in_progress = Some(OperationState {
            operation_type: OperationType::Deploying {
                library_name: "All".to_string(),
                dry_run,
            },
            progress: ScanProgress {
                total_bytes: 0,
                bytes_processed: 0,
                files_processed: 0,
                current_file: Some("Starting...".to_string()),
                errors: 0,
                start_time: std::time::Instant::now(),
            },
            cancel_flag,
        });
        self.operation_receiver = Some(rx);
    }

    fn show_dry_run_report(&mut self, config: config::Config) {
        use crate::deploy;

        // Generate report synchronously (it's fast)
        match config::get_db_path() {
            Ok(db_path) => match Database::open(&db_path) {
                Ok(db) => match deploy::generate_dry_run_report(&config, &db) {
                    Ok(reports) => {
                        let mut message = String::from("Deployment Dry Run Report:\n\n");
                        for report in reports {
                            message.push_str(&format!(
                                "Library: {}\n  Already deployed: {} files\n  To deploy: {} files ({:.2} GB)\n  Lost files: {} files ({:.2} GB)\n\n",
                                report.library_name,
                                report.files_already_deployed,
                                report.files_to_deploy,
                                report.bytes_to_deploy as f64 / 1_000_000_000.0,
                                report.lost_files,
                                report.lost_files_bytes as f64 / 1_000_000_000.0,
                            ));
                        }
                        self.status_message = Some(message);
                    }
                    Err(e) => {
                        self.status_message =
                            Some(format!("Error generating dry-run report: {}", e));
                    }
                },
                Err(e) => {
                    self.status_message = Some(format!("Error opening database: {}", e));
                }
            },
            Err(e) => {
                self.status_message = Some(format!("Error getting database path: {}", e));
            }
        }
    }

    fn save_all_tags(&mut self, tracks: Vec<Track>) {
        use std::path::Path;

        if let MenuState::TagEditor { ref tag_fields, .. } = self.current_view {
            let session_id = self.tag_edit_session_id.clone();
            let mut success_count = 0;
            let mut error_count = 0;

            for (idx, track) in tracks.iter().enumerate() {
                let track_id = match track.id {
                    Some(id) => id,
                    None => {
                        error_count += 1;
                        continue;
                    }
                };

                let path = Path::new(&track.path);
                let fields = &tag_fields[idx];

                // Convert tag fields to (key, value) tuples
                let tags_to_save: Vec<(String, String)> = fields
                    .iter()
                    .filter(|f| f.editable && !f.value.is_empty() && f.name != "New Tag")
                    .map(|f| (f.name.clone(), f.value.clone()))
                    .collect();

                // Write tags to file -> history -> database
                match metadata::write_tags(path, &tags_to_save, track_id, &session_id) {
                    Ok(_) => success_count += 1,
                    Err(e) => {
                        eprintln!("Failed to write tags for {}: {}", track.path, e);
                        error_count += 1;
                    }
                }
            }

            // Update status message
            if error_count == 0 {
                self.status_message =
                    Some(format!("✓ Saved {} track(s) successfully", success_count));
            } else {
                self.status_message = Some(format!(
                    "Saved {} track(s), {} error(s)",
                    success_count, error_count
                ));
            }

            // Exit tag editor and return to main menu
            self.current_view = MenuState::MainMenu;
            self.menu_state.select(Some(0));

            // Generate a new session ID for next editing session
            self.tag_edit_session_id = uuid::Uuid::new_v4().to_string();
        }
    }
}

pub fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        // Update eye animation
        app.update_eye_animation();

        terminal.draw(|f| ui(f, app))?;

        // Check for progress updates from background thread
        let mut should_clear_operation = false;
        if let Some(ref rx) = app.operation_receiver {
            while let Ok(message) = rx.try_recv() {
                match message {
                    ScanMessage::Progress(progress) => {
                        if let Some(ref mut op) = app.operation_in_progress {
                            op.progress = progress;
                        }
                    }
                    ScanMessage::Complete(result) => {
                        app.status_message = Some(
                            if result.files_scanned == 0 && result.files_skipped > 0 {
                                format!(
                                "Scan complete! {} files skipped (unchanged: identical inodes and modified timestamps) in {:.1}s",
                                result.files_skipped,
                                result.duration.as_secs_f64()
                            )
                            } else {
                                format!(
                                    "Scan complete! {} files ({:.2} GB) in {:.1}s",
                                    result.files_scanned,
                                    result.bytes_scanned as f64 / 1_000_000_000.0,
                                    result.duration.as_secs_f64()
                                )
                            },
                        );
                        should_clear_operation = true;
                    }
                    ScanMessage::Error(err) => {
                        app.status_message = Some(format!("Scan error: {}", err));
                        should_clear_operation = true;
                    }
                }
            }
        }

        // Clear operation state if scan completed or errored
        if should_clear_operation {
            app.operation_in_progress = None;
            app.operation_receiver = None;
        }

        // Handle keyboard input
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(18), // Fixed height for lower gutter
        ])
        .split(f.area());

    // Title - context-aware header
    let title_text = match &app.current_view {
        MenuState::TagEditor { .. } => {
            "Music Library Assistant - Tag Editing (Duplicate Resolution)"
        }
        _ => "Music Library Assistant",
    };
    let title = Paragraph::new(title_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Menu (main or sub-menu)
    match &app.current_view {
        MenuState::MainMenu => {
            // Render menu
            let items: Vec<ListItem> = app
                .menu_items
                .iter()
                .map(|item| ListItem::new(Line::from(item.label.clone())))
                .collect();

            let items = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("Menu"))
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(items, chunks[1], &mut app.menu_state);
        }
        MenuState::ScanSourceMenu { sources } => {
            let mut items: Vec<ListItem> = Vec::new();

            // First item: Scan All (normal)
            items.push(ListItem::new(Line::from("Scan all sources")));

            // Second item: Clean Rescan All (ignore cache)
            items.push(ListItem::new(Line::from("Scan all sources (please)")));

            // Remaining items: individual sources
            for src in sources.iter() {
                items.push(ListItem::new(Line::from(src.name.clone())));
            }

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Select Source to Scan"),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(list, chunks[1], &mut app.menu_state);
        }
        MenuState::ReportsMenu => {
            let items: Vec<ListItem> = vec![
                ListItem::new(Line::from("Generate All Reports")),
                ListItem::new(Line::from("Legacy Library Report")),
                ListItem::new(Line::from("Corpus Deployment Report")),
                ListItem::new(Line::from("Quality Report (Canonicalization)")),
                ListItem::new(Line::from("Duplicate Detection Report")),
            ];

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Select Report to Generate"),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(list, chunks[1], &mut app.menu_state);
        }
        MenuState::DeployMenu => {
            let items: Vec<ListItem> = vec![
                ListItem::new(Line::from("Dry Run (Preview Changes)")),
                ListItem::new(Line::from("Full Deploy")),
            ];

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Deploy to Libraries"),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(list, chunks[1], &mut app.menu_state);
        }
        MenuState::CorpusTriageMenu => {
            let items: Vec<ListItem> = vec![
                ListItem::new(Line::from("Metadata Deduplication (Tag Editor)")),
                ListItem::new(Line::from("Fingerprint-Based Deduplication")),
                ListItem::new(Line::from("Back to Main Menu")),
            ];

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Corpus Triage and Operations"),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(list, chunks[1], &mut app.menu_state);
        }
        MenuState::DuplicateGroupList { groups } => {
            // For MVP: show empty list with placeholder
            let items: Vec<ListItem> = if groups.is_empty() {
                vec![ListItem::new(Line::from(
                    "No duplicate groups found (MVP: not implemented yet)",
                ))]
            } else {
                groups
                    .iter()
                    .map(|g| {
                        let type_str = match g.group_type {
                            DuplicateGroupType::ExactMatch => "Exact",
                            DuplicateGroupType::MetadataMatch => "Metadata",
                        };
                        ListItem::new(Line::from(format!(
                            "[{}] {} tracks",
                            type_str,
                            g.tracks.len()
                        )))
                    })
                    .collect()
            };

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Duplicate Groups"),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(list, chunks[1], &mut app.menu_state);
        }
        MenuState::DuplicateGroupDetail {
            group,
            selected_track_idx,
        } => {
            // Show tracks in this duplicate group
            let items: Vec<ListItem> = group
                .tracks
                .iter()
                .enumerate()
                .map(|(idx, track)| {
                    let label = format!(
                        "[{}] {}",
                        if idx == *selected_track_idx { "*" } else { " " },
                        track.path
                    );
                    ListItem::new(Line::from(label))
                })
                .collect();

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Duplicate Group Detail"),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(list, chunks[1], &mut app.menu_state);
        }
        MenuState::TagEditor {
            tracks,
            tag_fields,
            current_track_idx,
            current_field_idx,
            field_edit_state,
            name_buffer,
            value_buffer,
            ..
        } => {
            // NEW LAYOUT: info pane (full width) | 3-column (tracks | editor | search) | status box (full width)
            let editor_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(5),  // Info pane (full-width, multi-line)
                    Constraint::Min(15),    // 3-column area (tracks | editor | search)
                    Constraint::Length(12), // Bottom status box (full-width)
                ])
                .split(chunks[1]);

            // 1. FILE INFO PANE - Full-width, multi-line, all immutable metadata
            let current_track = &tracks[*current_track_idx];

            // Format duration
            let duration_str = if let Some(duration_ms) = current_track.duration_ms {
                let total_seconds = duration_ms / 1000;
                let minutes = total_seconds / 60;
                let seconds = total_seconds % 60;
                format!("{}:{:02}", minutes, seconds)
            } else {
                "Unknown".to_string()
            };

            // Format file size (human-readable)
            let size_str = if current_track.file_size < 1024 {
                format!("{} B", current_track.file_size)
            } else if current_track.file_size < 1024 * 1024 {
                format!("{:.1} KB", current_track.file_size as f64 / 1024.0)
            } else {
                format!(
                    "{:.2} MB",
                    current_track.file_size as f64 / (1024.0 * 1024.0)
                )
            };

            // Format sample rate
            let sample_rate_str = current_track
                .sample_rate
                .map(|sr| format!("{} Hz", sr))
                .unwrap_or_else(|| "Unknown".to_string());

            let bitrate_str = current_track
                .bitrate_kbps
                .map(|b| format!("{} kbps", b))
                .unwrap_or_else(|| "Unknown".to_string());

            let info_lines = vec![
                Line::from(format!("Path: {}", current_track.path)),
                Line::from(format!(
                    "Format: {} | Size: {} | Duration: {} | Bitrate: {} | Sample Rate: {}",
                    current_track.file_type.to_uppercase(),
                    size_str,
                    duration_str,
                    bitrate_str,
                    sample_rate_str
                )),
                Line::from(format!(
                    "Source: {} | Inode: {}",
                    current_track.source, current_track.inode
                )),
            ];
            let info_para = Paragraph::new(info_lines).block(
                Block::default().borders(Borders::ALL).title(format!(
                    "File Info [Track {}/{}]",
                    current_track_idx + 1,
                    tracks.len()
                )),
            );
            f.render_widget(info_para, editor_layout[0]);

            // 2. 3-COLUMN LAYOUT: Tracks (30%) | Tag Editor (60%) | Search (10%)
            let three_column = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(30), // Track list
                    Constraint::Percentage(60), // Tag editor
                    Constraint::Percentage(10), // Search panel (collapsed)
                ])
                .split(editor_layout[1]);

            // 2a. TRACK LIST (left column)
            let track_items: Vec<ListItem> = tracks
                .iter()
                .enumerate()
                .map(|(idx, track)| {
                    let prefix = if idx == *current_track_idx {
                        ">> "
                    } else {
                        "   "
                    };
                    let artist = track.artist.as_deref().unwrap_or("Unknown");
                    let title = track.title.as_deref().unwrap_or("Unknown");
                    ListItem::new(Line::from(format!("{}{} - {}", prefix, artist, title))).style(
                        if idx == *current_track_idx {
                            Style::default().bg(Color::DarkGray)
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect();

            let track_list = List::new(track_items)
                .block(Block::default().borders(Borders::ALL).title("Tracks"));
            f.render_widget(track_list, three_column[0]);

            // 2b. TAG EDITOR (center column)
            let fields = &tag_fields[*current_track_idx];
            let field_lines: Vec<Line> = fields
                .iter()
                .enumerate()
                .map(|(idx, field)| {
                    let is_current = idx == *current_field_idx;

                    // Determine which part is being edited (show cursor)
                    let name_display =
                        if is_current && matches!(field_edit_state, FieldEditState::EditingName) {
                            name_buffer.clone() + "_" // Show cursor on name
                        } else {
                            field.name.clone()
                        };

                    let value_display =
                        if is_current && matches!(field_edit_state, FieldEditState::EditingValue) {
                            value_buffer.clone() + "_" // Show cursor on value
                        } else {
                            field.value.clone()
                        };

                    // Add "fill to all" button for applicable fields
                    let fill_button = if is_current
                        && field.editable
                        && !field.is_unique_per_track
                        && !field.value.is_empty()
                    {
                        " [F]"
                    } else {
                        ""
                    };

                    let line_text =
                        format!("{:18} : {}{}", name_display, value_display, fill_button);

                    let style = if is_current {
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };

                    Line::from(line_text).style(style)
                })
                .collect();

            let tag_para = Paragraph::new(field_lines)
                .block(Block::default().borders(Borders::ALL).title("Tag Editor"));
            f.render_widget(tag_para, three_column[1]);

            // 2c. SEARCH PANEL (right column, collapsed)
            let search_para = Paragraph::new(vec![Line::from("(TODO)")])
                .block(Block::default().borders(Borders::ALL).title("Search"))
                .alignment(Alignment::Center);
            f.render_widget(search_para, three_column[2]);

            // 3. BOTTOM STATUS BOX - Full width with dynamic messages + controls
            let mut status_lines = vec![];

            // Dynamic status message (1-2 lines at top)
            if let Some(ref msg) = app.status_message {
                status_lines
                    .push(Line::from(msg.clone()).style(Style::default().fg(Color::Yellow)));
                status_lines.push(Line::from(""));
            }

            // Controls (visible by default unless there's a very large message)
            status_lines.extend(vec![
                Line::from("Navigation: Tab/Shift+Tab = next/prev track | ←→ = toggle name/value | ↑↓ = navigate fields | Enter = edit/commit"),
                Line::from(""),
                Line::from("Actions: F = fill to all | Ctrl+U = clear | Esc = exit | Tab past last track to save all"),
            ]);

            // Phase 0: W key and Ctrl+S disabled - saves only via end-of-tracks modal

            let status_para = Paragraph::new(status_lines)
                .block(Block::default().borders(Borders::ALL).title("Status"));
            f.render_widget(status_para, editor_layout[2]);
        }
        MenuState::SaveConfirmationModal {
            selected_button, ..
        } => {
            // Phase 2: Render save confirmation modal
            // Calculate centered popup area (60% width, 30% height)
            let popup_area = centered_rect(60, 30, chunks[1]);

            // Clear background
            let clear_block = Block::default().style(Style::default().bg(Color::Reset));
            f.render_widget(clear_block, chunks[1]);

            // Render modal box
            let modal_block = Block::default()
                .borders(Borders::ALL)
                .title("Save Changes?")
                .border_style(Style::default().fg(Color::Yellow));

            let inner = modal_block.inner(popup_area);
            f.render_widget(modal_block, popup_area);

            // Button labels
            let buttons = [
                "[ Save All Changes ]",
                "[ Save All & Next Set ]",
                "[ Return to Editing ]",
            ];

            // Create lines with selection highlighting
            let mut button_lines: Vec<Line> = vec![
                Line::from(""),
                Line::from("All edits will be written to files."),
                Line::from(""),
            ];

            for (i, label) in buttons.iter().enumerate() {
                let style = if i == *selected_button {
                    Style::default()
                        .bg(Color::Cyan)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                button_lines.push(Line::from(*label).style(style));
                if i < buttons.len() - 1 {
                    button_lines.push(Line::from(""));
                }
            }

            button_lines.push(Line::from(""));
            button_lines.push(
                Line::from("Use ← → to select, Enter to confirm, Esc to cancel")
                    .style(Style::default().fg(Color::DarkGray)),
            );

            let paragraph = Paragraph::new(button_lines).alignment(Alignment::Center);
            f.render_widget(paragraph, inner);
        }
        MenuState::ChangePreviewModal {
            grouped_changes,
            single_changes,
            scroll_offset,
            ..
        } => {
            // Phase 4: Render change preview modal
            // Large modal to show all changes (80% width, 80% height)
            let modal_area = centered_rect(80, 80, chunks[1]);

            // Clear background
            let clear_block = Block::default().style(Style::default().bg(Color::Reset));
            f.render_widget(clear_block, chunks[1]);

            // Render modal box
            let modal_block = Block::default()
                .borders(Borders::ALL)
                .title("Review Changes Before Saving")
                .border_style(Style::default().fg(Color::Yellow));

            let inner = modal_block.inner(modal_area);
            f.render_widget(modal_block, modal_area);

            // Build lines for preview
            let mut lines = Vec::new();

            // Show summary at top
            let total_changes = grouped_changes
                .iter()
                .map(|g| g.track_indices.len())
                .sum::<usize>()
                + single_changes.len();
            lines.push(
                Line::from(format!("Total changes: {}", total_changes)).style(
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .fg(Color::Cyan),
                ),
            );
            lines.push(Line::from(""));

            // Grouped changes section
            if !grouped_changes.is_empty() {
                lines.push(
                    Line::from("Common Changes (multiple tracks):").style(
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .fg(Color::Green),
                    ),
                );
                lines.push(Line::from(""));

                for group in grouped_changes {
                    let track_list = if group.track_indices.len() <= 5 {
                        group
                            .track_indices
                            .iter()
                            .map(|i| (i + 1).to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        format!("{} tracks", group.track_indices.len())
                    };

                    let line_text = format!(
                        "  [{}] {}: '{}' → '{}'",
                        track_list,
                        group.field_name,
                        if group.old_value.is_empty() {
                            "(empty)"
                        } else {
                            &group.old_value
                        },
                        if group.new_value.is_empty() {
                            "(empty)"
                        } else {
                            &group.new_value
                        }
                    );
                    lines.push(Line::from(line_text).style(Style::default().fg(Color::Cyan)));
                }
                lines.push(Line::from(""));
            }

            // Single-track changes section
            if !single_changes.is_empty() {
                lines.push(
                    Line::from("Individual Track Changes:").style(
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .fg(Color::Yellow),
                    ),
                );
                lines.push(Line::from(""));

                for change in single_changes {
                    let line_text = format!(
                        "  Track {}: {}: '{}' → '{}'",
                        change.track_idx + 1,
                        change.field_name,
                        if change.old_value.is_empty() {
                            "(empty)"
                        } else {
                            &change.old_value
                        },
                        if change.new_value.is_empty() {
                            "(empty)"
                        } else {
                            &change.new_value
                        }
                    );
                    lines.push(Line::from(line_text).style(Style::default().fg(Color::White)));
                }
                lines.push(Line::from(""));
            }

            // Instructions at bottom
            lines.push(Line::from(""));
            lines.push(
                Line::from("Press Enter/Y to confirm and save, Esc/N to cancel")
                    .style(Style::default().fg(Color::DarkGray)),
            );
            lines.push(
                Line::from("Use ↑↓ or PgUp/PgDn to scroll")
                    .style(Style::default().fg(Color::DarkGray)),
            );

            // Apply scroll offset and clamp
            let max_scroll = lines.len().saturating_sub(inner.height as usize);
            let clamped_offset = (*scroll_offset).min(max_scroll);
            let visible_lines: Vec<Line> = lines
                .into_iter()
                .skip(clamped_offset)
                .take(inner.height as usize)
                .collect();

            let paragraph =
                Paragraph::new(visible_lines).wrap(ratatui::widgets::Wrap { trim: false });

            f.render_widget(paragraph, inner);
        }
        MenuState::FingerprintDedupDirSelect {
            selected_dirs,
            current_input,
            error_message,
        } => {
            let mut text = vec![
                Line::from("Fingerprint Deduplication - Directory Selection"),
                Line::from(""),
                Line::from("Selected directories:"),
            ];
            for (i, dir) in selected_dirs.iter().enumerate() {
                text.push(Line::from(format!("  {}. {}", i + 1, dir.display())));
            }
            text.push(Line::from(""));
            text.push(Line::from(format!(
                "Enter next directory path: {}",
                current_input
            )));
            if let Some(err) = error_message {
                text.push(Line::from(""));
                text.push(Line::from(format!("ERROR: {}", err)).style(Style::default().fg(Color::Red)));
            }
            text.push(Line::from(""));
            text.push(Line::from("[Enter on empty path to finish] [Esc to cancel]"));

            let paragraph = Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL))
                .wrap(ratatui::widgets::Wrap { trim: false });
            f.render_widget(paragraph, chunks[1]);
        }
        MenuState::FingerprintDedupResolve {
            conflict_sets,
            current_set_idx,
            session_stats,
            input_buffer,
            ..
        } => {
            if *current_set_idx >= conflict_sets.len() {
                let text = vec![Line::from("No more conflicts to resolve")];
                let paragraph = Paragraph::new(text).block(Block::default().borders(Borders::ALL));
                f.render_widget(paragraph, chunks[1]);
                return;
            }

            let conflict = &conflict_sets[*current_set_idx];
            let total_files: usize = conflict.tracks_by_dir.values().map(|v| v.len()).sum();

            let mut text = vec![
                Line::from(format!(
                    "Conflict Set {} of {} ({}-way conflict)",
                    current_set_idx + 1,
                    conflict_sets.len(),
                    conflict.conflict_dirs.len()
                )),
                Line::from("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"),
                Line::from(""),
                Line::from(format!("Files affected: {} total", total_files)),
                Line::from(format!("Match score: {}% (exact fingerprint match)", conflict.match_score)),
                Line::from(""),
                Line::from("Directories:"),
            ];

            for (i, dir) in conflict.conflict_dirs.iter().enumerate() {
                let count = conflict.tracks_by_dir.get(dir).map(|v| v.len()).unwrap_or(0);
                text.push(Line::from(format!("  [{}] {} ({} files)", i + 1, dir, count)));
            }

            text.push(Line::from(""));
            text.push(Line::from(format!("Select winning directory (1-{}): {}", conflict.conflict_dirs.len(), input_buffer)));
            text.push(Line::from(""));
            text.push(Line::from(format!("Session: {} resolved, {} skipped", session_stats.resolved_count, session_stats.skipped_count)));
            text.push(Line::from("[s] Skip  [q] Quit with stats  [Esc] Cancel"));

            let paragraph = Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title("Conflict Resolution"))
                .wrap(ratatui::widgets::Wrap { trim: false });
            f.render_widget(paragraph, chunks[1]);
        }
        MenuState::FingerprintDedupStats { stats, lost_found_path } => {
            let text = vec![
                Line::from("Fingerprint Deduplication Summary"),
                Line::from("═════════════════════════════════"),
                Line::from(""),
                Line::from(format!("Total conflict sets: {}", stats.total_sets)),
                Line::from(format!("Resolved: {}", stats.resolved_count)),
                Line::from(format!("Skipped: {}", stats.skipped_count)),
                Line::from(""),
                Line::from(format!("Files kept (in corpus): {}", stats.files_kept)),
                Line::from(format!("Files moved to lost+found: {}", stats.files_moved)),
                Line::from(""),
                Line::from("Lost+found location:"),
                Line::from(format!("  {}", lost_found_path.display())),
                Line::from(""),
                Line::from("Next steps:"),
                Line::from("• Rescan corpus to update database"),
                Line::from("• Review lost+found directory"),
                Line::from("• Delete moved files if confident"),
                Line::from(""),
                Line::from("[Enter] Return to menu"),
            ];

            let paragraph = Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title("Deduplication Complete"))
                .wrap(ratatui::widgets::Wrap { trim: false });
            f.render_widget(paragraph, chunks[1]);
        }
    }

    // Bottom gutter: split horizontally (status left, eye right)
    let show_eye = matches!(app.current_view, MenuState::MainMenu);

    let bottom_chunks = if show_eye {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(40),
                Constraint::Length(70), // Eye width
            ])
            .split(chunks[2])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(chunks[2])
    };

    // Render status/progress on the left (or full width if no eye)
    if let Some(ref op) = app.operation_in_progress {
        // Render progress display
        let progress_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(4),
            ])
            .split(bottom_chunks[0]);

        // Progress bar with dynamic title
        let (title, show_stats) = match &op.operation_type {
            OperationType::Scanning { .. } => ("Scanning Progress", true),
            OperationType::GeneratingReport { .. } => ("Report Generation", false),
            OperationType::Deploying { .. } => ("Deployment Progress", false),
        };

        let percentage = op.progress.percentage();
        let gauge = Gauge::default()
            .block(Block::default().title(title).borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Cyan))
            .percent(percentage.into())
            .label(format!("{}%", percentage));
        f.render_widget(gauge, progress_chunks[0]);

        // Stats (show for scanning only)
        if show_stats {
            let stats_text = format!(
                "Files: {} | Bytes: {:.2} GB / {:.2} GB | Speed: {:.1} MB/s",
                op.progress.files_processed,
                op.progress.bytes_processed as f64 / 1_000_000_000.0,
                op.progress.total_bytes as f64 / 1_000_000_000.0,
                op.progress.throughput_mbps()
            );
            let stats = Paragraph::new(stats_text)
                .block(Block::default().borders(Borders::ALL).title("Stats"))
                .style(Style::default().fg(Color::White));
            f.render_widget(stats, progress_chunks[1]);
        } else {
            let status_text = "Analyzing tracks and generating report...";
            let status = Paragraph::new(status_text)
                .block(Block::default().borders(Borders::ALL).title("Status"))
                .style(Style::default().fg(Color::White));
            f.render_widget(status, progress_chunks[1]);
        }

        // Current file + controls
        let current_file = op.progress.current_file.as_deref().unwrap_or("...");
        let file_text = format!("Current: {}\n\nPress 'c' or ESC to cancel", current_file);
        let file_para = Paragraph::new(file_text)
            .block(Block::default().borders(Borders::ALL).title("Status"))
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(file_para, progress_chunks[2]);
    } else {
        // Normal status message
        let status_text = app.status_message.as_deref().unwrap_or(
            "Use arrow keys (↑/↓) to navigate. Press Enter to select.\nPress ESC to go back, Q to quit."
        );
        let status = Paragraph::new(status_text)
            .block(Block::default().borders(Borders::ALL).title("Status"))
            .style(Style::default().fg(Color::White));
        f.render_widget(status, bottom_chunks[0]);
    }

    // Render eye on the right (only in MainMenu with no operation)
    if show_eye {
        let eye_art_raw = match app.eye_animation_state {
            EyeAnimationState::Idle => EYE_OPEN,
            EyeAnimationState::Closing => EYE_CLOSING,
            EyeAnimationState::Closed => EYE_CLOSED,
            EyeAnimationState::Opening => EYE_CLOSING, // Reuse closing frame for opening
            EyeAnimationState::FlutterClosing => EYE_CLOSING,
            EyeAnimationState::FlutterClosed => EYE_CLOSED,
            EyeAnimationState::FlutterOpening => EYE_CLOSING,
        };

        let eye = Paragraph::new(eye_art_raw)
            .block(Block::default().borders(Borders::ALL).title("MLA"))
            .style(Style::default().fg(Color::Cyan))
            .alignment(Alignment::Center);
        f.render_widget(eye, bottom_chunks[1]);
    }
}

/// Phase 2: Helper function to create a centered rectangle for popups
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
