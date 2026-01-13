//! Application State and Event Loop
//!
//! Provides the core application structure with mode-based UI dispatch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::ops::operation::OperationProgress;
// Re-export OperationType from ops::operation for use by other UI modules
pub use crate::ops::operation::OperationType;

// ============================================================================
// UI Mode Enum
// ============================================================================

/// Current UI mode - determines which module handles rendering and input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    /// Multi-pane main menu with categories and commands
    MainMenu,
    /// Tag editor for metadata editing
    TagEditor,
    /// Dialogue-based decision flow
    Dialogue,
    /// Dialogue summary after completing a flow
    DialogueSummary,
    /// Legacy sub-menus (being phased out)
    LegacyMenu,
}

// ============================================================================
// Background Operation Tracking
// ============================================================================

/// State of an in-progress background operation
#[derive(Debug)]
pub struct OperationState {
    pub operation_type: OperationType,
    pub progress: OperationProgress,
    pub cancel_flag: Arc<AtomicBool>,
}

impl OperationState {
    pub fn new(operation_type: OperationType) -> Self {
        Self {
            operation_type,
            progress: OperationProgress::new(0),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Eye Animation State Machine
// ============================================================================

/// State of the eye animation (Talos Principle inspired)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeAnimationState {
    /// Eye open, waiting for next blink
    Idle,
    /// Transitioning to closed
    Closing,
    /// Eye closed
    Closed,
    /// Transitioning back to open
    Opening,
    /// Rapid flutter closing
    FlutterClosing,
    /// Rapid flutter closed
    FlutterClosed,
    /// Rapid flutter opening
    FlutterOpening,
}

/// Type of blink animation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlinkType {
    /// Medium speed blink
    Normal,
    /// Slower, more deliberate blink
    Slow,
    /// Quick blink
    Fast,
    /// Multiple rapid blinks
    Flutter,
}

/// Result of rolling d20 for heartbeat trigger.
///
/// - `Nothing` (1-12, 60%): No action
/// - `Normal` (14-20, 35%): Standard heartbeat check
/// - `Expensive` (13, 5%): Expensive cleanup operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatRollResult {
    /// No heartbeat triggered (rolled 1-12)
    Nothing,
    /// Normal heartbeat (rolled 14-20)
    Normal,
    /// Expensive cleaning operations (rolled 13)
    Expensive,
}

/// Eye animation controller
#[derive(Debug)]
pub struct EyeAnimation {
    pub state: EyeAnimationState,
    pub state_start_time: Instant,
    pub next_blink_delay_secs: u64,
    pub current_blink_type: BlinkType,
    pub flutter_count: u8,
    /// When true, eye stays closed (waiting for startup heartbeat)
    pub heartbeat_pending: bool,
    /// Set when a blink completes and d20 triggers heartbeat (>= 13)
    heartbeat_roll_result: Option<HeartbeatRollResult>,
}

impl Default for EyeAnimation {
    fn default() -> Self {
        Self {
            state: EyeAnimationState::Idle,
            state_start_time: Instant::now(),
            next_blink_delay_secs: Self::random_blink_delay(),
            current_blink_type: BlinkType::Normal,
            flutter_count: 0,
            heartbeat_pending: false,
            heartbeat_roll_result: None,
        }
    }
}

impl EyeAnimation {
    /// Generate random delay between blinks: 30-60 seconds with occasional longer pauses
    pub fn random_blink_delay() -> u64 {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let hasher = RandomState::new();
        let random_val = hasher.hash_one(now);

        // 30-60 seconds normally, occasionally up to 90 seconds
        let base = 30 + (random_val % 30);
        if random_val.is_multiple_of(10) {
            base + 30 // 10% chance of extra long pause
        } else {
            base
        }
    }

    /// Roll a d20 - returns 1-20
    fn roll_d20() -> u8 {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let hasher = RandomState::new();
        let random_val = hasher.hash_one(now);

        // 1-20 inclusive
        (1 + (random_val % 20)) as u8
    }
}

/// Flip a coin - returns true for heads, false for tails.
/// Uses hash-based PRNG similar to eye animation d20 roll.
// TODO: Resolve coin-flip actions to Opinion in the future
pub fn flip_coin() -> bool {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let hasher = RandomState::new();
    let random_val = hasher.hash_one(now);

    random_val % 2 == 0
}

impl EyeAnimation {
    /// Check and clear the heartbeat roll result.
    ///
    /// Returns `Some(result)` if a heartbeat was triggered, `None` otherwise.
    pub fn take_heartbeat_trigger(&mut self) -> Option<HeartbeatRollResult> {
        self.heartbeat_roll_result.take()
    }

    /// Roll d20 and interpret result for heartbeat triggering.
    ///
    /// - 1-12 (60%): Nothing
    /// - 13 (5%): Expensive cleanup operations
    /// - 14-20 (35%): Normal heartbeat
    fn roll_heartbeat() -> HeartbeatRollResult {
        match Self::roll_d20() {
            13 => HeartbeatRollResult::Expensive,
            14..=20 => HeartbeatRollResult::Normal,
            _ => HeartbeatRollResult::Nothing,
        }
    }

    /// Choose random blink type (weighted towards normal)
    pub fn random_blink_type() -> BlinkType {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
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

    /// Update the eye animation state machine
    pub fn update(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.state_start_time);

        match self.state {
            EyeAnimationState::Idle => {
                if elapsed.as_secs() >= self.next_blink_delay_secs {
                    self.current_blink_type = Self::random_blink_type();
                    self.state = EyeAnimationState::Closing;
                    self.state_start_time = now;

                    if matches!(self.current_blink_type, BlinkType::Flutter) {
                        self.flutter_count = 2 + ((now.elapsed().as_nanos() % 2) as u8);
                    }
                }
            }
            EyeAnimationState::Closing => {
                let duration = match self.current_blink_type {
                    BlinkType::Slow => 250,
                    BlinkType::Normal => 120,
                    BlinkType::Fast => 60,
                    BlinkType::Flutter => 40,
                };

                if elapsed.as_millis() >= duration {
                    self.state = EyeAnimationState::Closed;
                    self.state_start_time = now;
                }
            }
            EyeAnimationState::Closed => {
                let duration = match self.current_blink_type {
                    BlinkType::Slow => 200,
                    BlinkType::Normal => 100,
                    BlinkType::Fast => 50,
                    BlinkType::Flutter => 30,
                };

                if elapsed.as_millis() >= duration {
                    self.state = EyeAnimationState::Opening;
                    self.state_start_time = now;
                }
            }
            EyeAnimationState::Opening => {
                let duration = match self.current_blink_type {
                    BlinkType::Slow => 250,
                    BlinkType::Normal => 120,
                    BlinkType::Fast => 60,
                    BlinkType::Flutter => 40,
                };

                if elapsed.as_millis() >= duration {
                    if matches!(self.current_blink_type, BlinkType::Flutter) && self.flutter_count > 0
                    {
                        self.flutter_count -= 1;
                        self.state = EyeAnimationState::FlutterClosing;
                        self.state_start_time = now;
                    } else {
                        self.state = EyeAnimationState::Idle;
                        self.state_start_time = now;
                        self.next_blink_delay_secs = Self::random_blink_delay();

                        // Blink completed - roll d20 for heartbeat (>= 13)
                        let roll = Self::roll_heartbeat();
                        if roll != HeartbeatRollResult::Nothing {
                            self.heartbeat_roll_result = Some(roll);
                        }
                    }
                }
            }
            EyeAnimationState::FlutterClosing => {
                if elapsed.as_millis() >= 35 {
                    self.state = EyeAnimationState::FlutterClosed;
                    self.state_start_time = now;
                }
            }
            EyeAnimationState::FlutterClosed => {
                if elapsed.as_millis() >= 25 {
                    self.state = EyeAnimationState::FlutterOpening;
                    self.state_start_time = now;
                }
            }
            EyeAnimationState::FlutterOpening => {
                if elapsed.as_millis() >= 35 {
                    if self.flutter_count > 0 {
                        self.flutter_count -= 1;
                        self.state = EyeAnimationState::FlutterClosing;
                        self.state_start_time = now;
                    } else {
                        self.state = EyeAnimationState::Idle;
                        self.state_start_time = now;
                        self.next_blink_delay_secs = Self::random_blink_delay();

                        // Flutter blink completed - roll d20 for heartbeat (>= 13)
                        let roll = Self::roll_heartbeat();
                        if roll != HeartbeatRollResult::Nothing {
                            self.heartbeat_roll_result = Some(roll);
                        }
                    }
                }
            }
        }
    }

    /// Get the current eye frame to display
    pub fn current_frame(&self) -> EyeFrame {
        // Keep eye closed during heartbeat check
        if self.heartbeat_pending {
            return EyeFrame::Closed;
        }

        match self.state {
            EyeAnimationState::Idle | EyeAnimationState::Opening | EyeAnimationState::FlutterOpening => {
                EyeFrame::Open
            }
            EyeAnimationState::Closing | EyeAnimationState::FlutterClosing => EyeFrame::Closing,
            EyeAnimationState::Closed | EyeAnimationState::FlutterClosed => EyeFrame::Closed,
        }
    }

    /// Set heartbeat pending state (eye stays closed until cleared)
    pub fn set_heartbeat_pending(&mut self, pending: bool) {
        self.heartbeat_pending = pending;
        if pending {
            // Start in closed state when heartbeat begins
            self.state = EyeAnimationState::Closed;
            self.state_start_time = Instant::now();
        }
    }
}

/// Which eye frame to display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeFrame {
    Open,
    Closing,
    Closed,
}

// ASCII eye frames from The Talos Principle
pub const EYE_OPEN: &str = r#"                     ...',;;:cccccccc:;,..
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

pub const EYE_CLOSING: &str = r#"                         ...'',;;;;::;;;,'..
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

pub const EYE_CLOSED: &str = r#"                          ...'',;;;;;;;,,...
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
