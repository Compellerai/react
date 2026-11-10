//! YouTube Live Tier 2 Module
//!
//! Product-grade YouTube Live streaming with:
//! - OAuth via compeller.ai (server-side token management)
//! - Strict state machine (no duplicate broadcasts, no flapping)
//! - Separate control plane (YouTube API) and data plane (RTMP)
//! - Hard latches preventing accidental repetition
//!
//! Architecture:
//! - Desktop app owns: session state machine, RTMP data plane, UI latches
//! - Compeller.ai owns: OAuth tokens, YouTube API calls, token refresh

pub mod auth;
pub mod eligibility;
pub mod session;
pub mod control_plane;

pub use auth::{YouTubeAuth, YouTubeAuthState};
pub use eligibility::{EligibilityCache, EligibilityState};
pub use session::{LiveSession, SessionState, SessionStep};
pub use control_plane::YouTubeControlPlane;

use std::time::Instant;

/// Timing constants for stability
pub mod timing {
    /// Minimum hold time in Creating state (ms)
    pub const CREATING_HOLD_MS: u64 = 2000;
    /// Minimum hold time in Bound state (ms)
    pub const BOUND_HOLD_MS: u64 = 1000;
    /// Minimum hold time in Connecting state (ms)
    pub const CONNECTING_HOLD_MS: u64 = 1000;
    /// Minimum hold time in Pushing state (ms)
    pub const PUSHING_HOLD_MS: u64 = 2000;
    /// Minimum hold time in Testing state (ms)
    pub const TESTING_HOLD_MS: u64 = 2000;
    /// Minimum hold time in Live state (ms)
    pub const LIVE_HOLD_MS: u64 = 2000;
    /// Minimum hold time in Ending state (ms)
    pub const ENDING_HOLD_MS: u64 = 1000;

    /// Cooldown after GoLive click before allowing another (ms)
    pub const GO_LIVE_LATCH_MS: u64 = 3000;
    /// Cooldown between state transitions (testing/live) (ms)
    pub const TRANSITION_COOLDOWN_MS: u64 = 5000;

    /// YouTube status polling interval (ms) - 30 seconds, just for UI confirmation
    pub const POLL_INTERVAL_MS: u64 = 30000;
    /// Eligibility cache validity (ms) - 24 hours
    pub const ELIGIBILITY_CACHE_MS: u64 = 24 * 60 * 60 * 1000;

    /// Initial RTMP retry backoff (ms)
    pub const RTMP_INITIAL_BACKOFF_MS: u64 = 2000;
    /// Max RTMP retry backoff (ms)
    pub const RTMP_MAX_BACKOFF_MS: u64 = 30000;
    /// Max RTMP retry attempts before failure
    pub const RTMP_MAX_ATTEMPTS: u32 = 10;

    /// Initial API retry backoff (ms)
    pub const API_INITIAL_BACKOFF_MS: u64 = 2000;
    /// Max API retry backoff (ms)
    pub const API_MAX_BACKOFF_MS: u64 = 15000;
    /// Max API retry attempts before failure
    pub const API_MAX_ATTEMPTS: u32 = 3;

    /// Confirmation count required before transition (hysteresis)
    pub const CONFIRM_COUNT_REQUIRED: u32 = 2;

    /// Token refresh margin (refresh if expiring within this time) (ms)
    pub const TOKEN_REFRESH_MARGIN_MS: u64 = 5 * 60 * 1000; // 5 minutes
}

/// User intents that drive the state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserIntent {
    None,
    SelectYouTubeLive,
    ClickConnect,      // Open browser to connect YouTube
    ClickGoLive,
    ClickStop,
    ClickRetry,
    ClickDisconnect,   // Disconnect YouTube account
}

impl Default for UserIntent {
    fn default() -> Self {
        UserIntent::None
    }
}

/// RTMP connection state (data plane)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

impl Default for RtmpState {
    fn default() -> Self {
        RtmpState::Disconnected
    }
}

/// Encoder health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderHealth {
    Stopped,
    Running,
    Error,
}

impl Default for EncoderHealth {
    fn default() -> Self {
        EncoderHealth::Stopped
    }
}

/// Retry state with exponential backoff
#[derive(Debug, Clone)]
pub struct RetryState {
    pub attempts: u32,
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub next_retry_at: Option<Instant>,
}

impl RetryState {
    pub fn new(initial_backoff_ms: u64, max_backoff_ms: u64, max_attempts: u32) -> Self {
        Self {
            attempts: 0,
            max_attempts,
            backoff_ms: initial_backoff_ms,
            initial_backoff_ms,
            max_backoff_ms,
            next_retry_at: None,
        }
    }

    /// Record a failure and schedule next retry
    pub fn record_failure(&mut self) {
        self.attempts += 1;
        self.next_retry_at = Some(Instant::now() + std::time::Duration::from_millis(self.backoff_ms));
        // Exponential backoff with cap
        self.backoff_ms = (self.backoff_ms * 2).min(self.max_backoff_ms);
    }

    /// Check if we can retry now
    pub fn can_retry_now(&self) -> bool {
        if self.attempts >= self.max_attempts {
            return false;
        }
        match self.next_retry_at {
            Some(t) => Instant::now() >= t,
            None => true,
        }
    }

    /// Check if max retries exhausted
    pub fn is_exhausted(&self) -> bool {
        self.attempts >= self.max_attempts
    }

    /// Reset retry state
    pub fn reset(&mut self) {
        self.attempts = 0;
        self.backoff_ms = self.initial_backoff_ms;
        self.next_retry_at = None;
    }
}

/// Latch state for preventing rapid re-triggering
#[derive(Debug, Clone, Default)]
pub struct Latches {
    /// GoLive button disabled until this instant
    pub go_live_disabled_until: Option<Instant>,
    /// State transitions disabled until this instant
    pub transition_disabled_until: Option<Instant>,
    /// Minimum state hold until this instant
    pub min_state_hold_until: Option<Instant>,
}

impl Latches {
    /// Check if GoLive is currently latched
    pub fn is_go_live_latched(&self) -> bool {
        self.go_live_disabled_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Check if transitions are currently latched
    pub fn is_transition_latched(&self) -> bool {
        self.transition_disabled_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Check if we must hold current state
    pub fn must_hold_state(&self) -> bool {
        self.min_state_hold_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Set GoLive latch
    pub fn latch_go_live(&mut self, duration_ms: u64) {
        self.go_live_disabled_until = Some(Instant::now() + std::time::Duration::from_millis(duration_ms));
    }

    /// Set transition latch
    pub fn latch_transition(&mut self, duration_ms: u64) {
        self.transition_disabled_until = Some(Instant::now() + std::time::Duration::from_millis(duration_ms));
    }

    /// Set minimum state hold
    pub fn set_min_hold(&mut self, duration_ms: u64) {
        self.min_state_hold_until = Some(Instant::now() + std::time::Duration::from_millis(duration_ms));
    }

    /// Clear all latches
    pub fn clear(&mut self) {
        self.go_live_disabled_until = None;
        self.transition_disabled_until = None;
        self.min_state_hold_until = None;
    }
}

/// Observations from YouTube polling
#[derive(Debug, Clone, Default)]
pub struct YouTubeObservations {
    /// Is the stream receiving data (active)?
    pub stream_active: bool,
    /// Confirmation count for stream_active
    pub stream_active_confirmed: u32,
    /// Broadcast lifecycle status from YouTube
    pub broadcast_status: BroadcastStatus,
    /// Confirmation count for broadcast status
    pub broadcast_status_confirmed: u32,
    /// Last poll timestamp
    pub last_poll: Option<Instant>,
}

impl YouTubeObservations {
    /// Update stream active status with hysteresis
    pub fn update_stream_active(&mut self, active: bool) {
        if active == self.stream_active {
            self.stream_active_confirmed += 1;
        } else {
            self.stream_active = active;
            self.stream_active_confirmed = 1;
        }
    }

    /// Update broadcast status with hysteresis
    pub fn update_broadcast_status(&mut self, status: BroadcastStatus) {
        if status == self.broadcast_status {
            self.broadcast_status_confirmed += 1;
        } else {
            self.broadcast_status = status;
            self.broadcast_status_confirmed = 1;
        }
    }

    /// Check if stream active is confirmed
    pub fn is_stream_active_confirmed(&self) -> bool {
        self.stream_active && self.stream_active_confirmed >= timing::CONFIRM_COUNT_REQUIRED
    }

    /// Check if broadcast status is confirmed
    pub fn is_status_confirmed(&self, status: BroadcastStatus) -> bool {
        self.broadcast_status == status && self.broadcast_status_confirmed >= timing::CONFIRM_COUNT_REQUIRED
    }

    /// Reset observations
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// YouTube broadcast lifecycle status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BroadcastStatus {
    #[default]
    Unknown,
    Created,
    Ready,
    Testing,
    Live,
    Complete,
    Revoked,
}

impl BroadcastStatus {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "created" => BroadcastStatus::Created,
            "ready" => BroadcastStatus::Ready,
            "testing" | "teststarting" => BroadcastStatus::Testing,
            "live" | "livestarting" => BroadcastStatus::Live,
            "complete" => BroadcastStatus::Complete,
            "revoked" => BroadcastStatus::Revoked,
            _ => BroadcastStatus::Unknown,
        }
    }
}

/// Stream status from YouTube
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamStatus {
    #[default]
    Unknown,
    Active,
    Inactive,
    Error,
}

impl StreamStatus {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "active" => StreamStatus::Active,
            "inactive" => StreamStatus::Inactive,
            "error" => StreamStatus::Error,
            _ => StreamStatus::Unknown,
        }
    }
}
