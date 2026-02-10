//! Eye Animation System
//!
//! The Eye is MM's visual heartbeat - a Talos Principle-inspired animation
//! that blinks periodically. The daemon controls Eye lifecycle and eyeballing;
//! this module handles only the visual animation.
//!
//! ## Animation States
//!
//! ```text
//! Idle → Closing → Closed → Opening → Idle
//!                    ↓
//!              (Flutter loop)
//! ```
//!
//! ## Usage
//!
//! The Eye animation is controlled by passing `can_animate` to `update()`:
//! - `can_animate=true`: Normal blinking animation
//! - `can_animate=false`: Eye stays closed (daemon controls this based on eye_state)

use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ============================================================================
// Animation State Machine
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

/// Which eye frame to display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeFrame {
    Open,
    Closing,
    Closed,
}

// ============================================================================
// Eye Animation Controller
// ============================================================================

/// Eye animation controller.
///
/// Manages the visual eye animation only. Lifecycle state (Closed/Awake) and
/// eyeballing triggers are controlled by the daemon.
#[derive(Debug)]
pub struct Eye {
    /// Current animation state
    state: EyeAnimationState,
    /// When the current state started
    state_start_time: Instant,
    /// Seconds until next blink (when in Idle state)
    next_blink_delay_secs: u64,
    /// Type of current blink animation
    current_blink_type: BlinkType,
    /// Remaining flutter blinks in a flutter sequence
    flutter_count: u8,
    /// Set to true when a blink just completed (for d20 roll trigger in UI)
    blink_completed: bool,
}

impl Default for Eye {
    fn default() -> Self {
        Self {
            state: EyeAnimationState::Closed,
            state_start_time: Instant::now(),
            next_blink_delay_secs: Self::random_blink_delay(),
            current_blink_type: BlinkType::Normal,
            flutter_count: 0,
            blink_completed: false,
        }
    }
}

impl Eye {
    // ========================================================================
    // Animation State Queries
    // ========================================================================

    /// Get the current eye frame to display.
    ///
    /// Pure mapping from animation state to frame. Lifecycle checks are
    /// handled by the caller based on daemon's eye_state.
    pub fn current_frame(&self) -> EyeFrame {
        match self.state {
            EyeAnimationState::Idle
            | EyeAnimationState::Opening
            | EyeAnimationState::FlutterOpening => EyeFrame::Open,
            EyeAnimationState::Closing | EyeAnimationState::FlutterClosing => EyeFrame::Closing,
            EyeAnimationState::Closed | EyeAnimationState::FlutterClosed => EyeFrame::Closed,
        }
    }

    // ========================================================================
    // Animation Control
    // ========================================================================

    /// Update the eye animation state machine.
    ///
    /// Pass `can_animate=false` to keep the eye closed (used when daemon's
    /// eye_state is Closed or during eyeballing).
    pub fn update(&mut self, can_animate: bool) {
        // Clear blink_completed flag at start of each update
        self.blink_completed = false;

        if !can_animate {
            // Force closed state, don't advance animation
            self.state = EyeAnimationState::Closed;
            return;
        }

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
                        self.complete_blink(now);
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
                        self.complete_blink(now);
                    }
                }
            }
        }
    }

    /// Complete a blink - return to Idle and signal completion.
    fn complete_blink(&mut self, now: Instant) {
        self.state = EyeAnimationState::Idle;
        self.state_start_time = now;
        self.next_blink_delay_secs = Self::random_blink_delay();
        self.blink_completed = true;
    }

    // ========================================================================
    // Randomness
    // ========================================================================

    /// Generate random delay between blinks: 30-60 seconds with occasional longer pauses.
    fn random_blink_delay() -> u64 {
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

    /// Choose random blink type (weighted towards normal).
    fn random_blink_type() -> BlinkType {
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
}

// ============================================================================
// ASCII Art Frames
// ============================================================================

/// Eye open frame (Talos Principle inspired)
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

/// Eye closing frame (transition)
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

/// Eye closed frame
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
