//! Live Session State Machine
//!
//! The authoritative state machine for YouTube Live streaming.
//! Controls both the control plane (YouTube API) and data plane (RTMP).
//!
//! State flow:
//! Idle → Creating → Bound → Connecting → Pushing → Testing → Live → Ending → Ended
//!                                                                   ↓
//!                                                                Failed
//!
//! Key invariants:
//! - Single session at a time (G1)
//! - Creation pipeline is serialized (G2)
//! - Cooldowns prevent flapping (G3)
//! - Stop is always honored (G4)

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;
use zeroize::Zeroize;

use super::{
    timing, BroadcastStatus, EncoderHealth, Latches, RetryState,
    RtmpState, StreamStatus, UserIntent, YouTubeObservations,
};

/// Session lifecycle states
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// No active session, ready to start
    Idle,
    /// Creating broadcast/stream/binding (see SessionStep)
    Creating,
    /// Broadcast and stream created and bound, ready to connect RTMP
    Bound,
    /// Connecting RTMP to ingest endpoint
    Connecting,
    /// RTMP connected, pushing frames, waiting for YouTube to see stream
    Pushing,
    /// YouTube sees stream, transitioning to testing
    Testing,
    /// Live! Broadcasting to viewers
    Live,
    /// Stopping the broadcast
    Ending,
    /// Broadcast ended cleanly
    Ended,
    /// Something went wrong
    Failed(String),
}

impl Default for SessionState {
    fn default() -> Self {
        SessionState::Idle
    }
}

impl SessionState {
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            SessionState::Idle => "Ready",
            SessionState::Creating => "Creating...",
            SessionState::Bound => "Bound",
            SessionState::Connecting => "Connecting...",
            SessionState::Pushing => "Connecting...",
            SessionState::Testing => "Testing...",
            SessionState::Live => "LIVE",
            SessionState::Ending => "Stopping...",
            SessionState::Ended => "Ended",
            SessionState::Failed(_) => "Failed",
        }
    }

    /// Is this a "busy" state where we shouldn't allow GoLive?
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            SessionState::Creating
                | SessionState::Bound
                | SessionState::Connecting
                | SessionState::Pushing
                | SessionState::Testing
                | SessionState::Live
                | SessionState::Ending
        )
    }

    /// Can we stop from this state?
    pub fn can_stop(&self) -> bool {
        matches!(
            self,
            SessionState::Creating
                | SessionState::Bound
                | SessionState::Connecting
                | SessionState::Pushing
                | SessionState::Testing
                | SessionState::Live
        )
    }
}

/// Creation pipeline steps (serialized)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStep {
    /// Step 0: Create broadcast
    CreateBroadcast,
    /// Step 1: Create stream (RTMP ingestion)
    CreateStream,
    /// Step 2: Bind broadcast to stream
    BindBroadcastToStream,
    /// Step 3: Fetch ingestion info
    FetchIngestionInfo,
    /// All steps complete
    Complete,
}

impl Default for SessionStep {
    fn default() -> Self {
        SessionStep::CreateBroadcast
    }
}

impl SessionStep {
    pub fn next(self) -> Self {
        match self {
            SessionStep::CreateBroadcast => SessionStep::CreateStream,
            SessionStep::CreateStream => SessionStep::BindBroadcastToStream,
            SessionStep::BindBroadcastToStream => SessionStep::FetchIngestionInfo,
            SessionStep::FetchIngestionInfo => SessionStep::Complete,
            SessionStep::Complete => SessionStep::Complete,
        }
    }
}

/// Secrets that must be cleared on session end
#[derive(Debug, Clone, Default)]
pub struct SessionSecrets {
    /// RTMP ingest URL
    ingest_url: Option<String>,
    /// Stream key (NEVER log this)
    stream_key: Option<String>,
}

impl SessionSecrets {
    pub fn set(&mut self, ingest_url: String, stream_key: String) {
        self.ingest_url = Some(ingest_url);
        self.stream_key = Some(stream_key);
    }

    pub fn get_ingest_url(&self) -> Option<&str> {
        self.ingest_url.as_deref()
    }

    pub fn get_stream_key(&self) -> Option<&str> {
        self.stream_key.as_deref()
    }

    /// Get full RTMP URL (ingest + key)
    pub fn get_rtmp_url(&self) -> Option<String> {
        match (&self.ingest_url, &self.stream_key) {
            (Some(url), Some(key)) => Some(format!("{}/{}", url, key)),
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        if let Some(ref mut url) = self.ingest_url {
            url.zeroize();
        }
        if let Some(ref mut key) = self.stream_key {
            key.zeroize();
        }
        self.ingest_url = None;
        self.stream_key = None;
    }
}

impl Drop for SessionSecrets {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Live session data
#[derive(Clone)]
pub struct LiveSession {
    /// Current session state
    state: Arc<Mutex<SessionState>>,
    /// Current creation step (when in Creating state)
    step: Arc<Mutex<SessionStep>>,
    /// Stream title for the broadcast
    title: Arc<Mutex<String>>,
    /// Stream resolution (e.g., "1080p", "2160p")
    resolution: Arc<Mutex<String>>,
    /// Stream frame rate (e.g., "30fps", "60fps")
    frame_rate: Arc<Mutex<String>>,
    /// Stream bitrate in kbps
    bitrate_kbps: Arc<Mutex<u32>>,
    /// Stream privacy (e.g., "public", "unlisted", "private")
    privacy: Arc<Mutex<String>>,
    /// YouTube broadcast ID
    broadcast_id: Arc<Mutex<Option<String>>>,
    /// YouTube stream ID
    stream_id: Arc<Mutex<Option<String>>>,
    /// Session secrets (ingest URL + stream key)
    secrets: Arc<Mutex<SessionSecrets>>,
    /// When session was created
    created_at: Arc<Mutex<Option<Instant>>>,
    /// Last error message (sanitized)
    last_error: Arc<Mutex<Option<String>>>,
    /// Latches for preventing rapid state changes
    latches: Arc<Mutex<Latches>>,
    /// API retry state
    api_retry: Arc<Mutex<RetryState>>,
    /// RTMP retry state
    rtmp_retry: Arc<Mutex<RetryState>>,
    /// RTMP connection state
    rtmp_state: Arc<Mutex<RtmpState>>,
    /// Encoder health
    encoder_health: Arc<Mutex<EncoderHealth>>,
    /// YouTube observations from polling
    observations: Arc<Mutex<YouTubeObservations>>,
    /// Next YouTube poll time
    next_poll: Arc<Mutex<Option<Instant>>>,
    /// Dropped frames counter
    dropped_frames: Arc<Mutex<u64>>,
    /// Is an API call in flight?
    api_in_flight: Arc<Mutex<bool>>,
}

impl Default for LiveSession {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSession {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState::Idle)),
            step: Arc::new(Mutex::new(SessionStep::CreateBroadcast)),
            title: Arc::new(Mutex::new("REACT (Live) by Compeller.ai".to_string())),
            resolution: Arc::new(Mutex::new("1080p".to_string())),
            frame_rate: Arc::new(Mutex::new("60fps".to_string())),
            bitrate_kbps: Arc::new(Mutex::new(6000)),
            privacy: Arc::new(Mutex::new("unlisted".to_string())),
            broadcast_id: Arc::new(Mutex::new(None)),
            stream_id: Arc::new(Mutex::new(None)),
            secrets: Arc::new(Mutex::new(SessionSecrets::default())),
            created_at: Arc::new(Mutex::new(None)),
            last_error: Arc::new(Mutex::new(None)),
            latches: Arc::new(Mutex::new(Latches::default())),
            api_retry: Arc::new(Mutex::new(RetryState::new(
                timing::API_INITIAL_BACKOFF_MS,
                timing::API_MAX_BACKOFF_MS,
                timing::API_MAX_ATTEMPTS,
            ))),
            rtmp_retry: Arc::new(Mutex::new(RetryState::new(
                timing::RTMP_INITIAL_BACKOFF_MS,
                timing::RTMP_MAX_BACKOFF_MS,
                timing::RTMP_MAX_ATTEMPTS,
            ))),
            rtmp_state: Arc::new(Mutex::new(RtmpState::Disconnected)),
            encoder_health: Arc::new(Mutex::new(EncoderHealth::Stopped)),
            observations: Arc::new(Mutex::new(YouTubeObservations::default())),
            next_poll: Arc::new(Mutex::new(None)),
            dropped_frames: Arc::new(Mutex::new(0)),
            api_in_flight: Arc::new(Mutex::new(false)),
        }
    }

    // ========== State Getters ==========

    pub fn get_state(&self) -> SessionState {
        self.state.lock().clone()
    }

    pub fn get_step(&self) -> SessionStep {
        *self.step.lock()
    }

    pub fn get_broadcast_id(&self) -> Option<String> {
        self.broadcast_id.lock().clone()
    }

    pub fn get_stream_id(&self) -> Option<String> {
        self.stream_id.lock().clone()
    }

    pub fn get_title(&self) -> String {
        self.title.lock().clone()
    }

    pub fn set_title(&self, title: String) {
        *self.title.lock() = title;
    }

    pub fn get_resolution(&self) -> String {
        self.resolution.lock().clone()
    }

    pub fn set_resolution(&self, resolution: String) {
        *self.resolution.lock() = resolution;
    }

    pub fn get_frame_rate(&self) -> String {
        self.frame_rate.lock().clone()
    }

    pub fn set_frame_rate(&self, frame_rate: String) {
        *self.frame_rate.lock() = frame_rate;
    }

    pub fn get_bitrate_kbps(&self) -> u32 {
        *self.bitrate_kbps.lock()
    }

    pub fn set_bitrate_kbps(&self, bitrate: u32) {
        *self.bitrate_kbps.lock() = bitrate;
    }

    /// Get output mode for GStreamer based on resolution and frame rate
    pub fn get_output_mode(&self) -> crate::output::OutputMode {
        let resolution = self.resolution.lock().clone();
        let frame_rate = self.frame_rate.lock().clone();
        let fps: u32 = frame_rate.replace("fps", "").parse().unwrap_or(30);

        match (resolution.as_str(), fps) {
            ("720p", 30) => crate::output::OutputMode::P720_30,
            ("720p", _) => crate::output::OutputMode::P720_60,
            ("1080p", 30) => crate::output::OutputMode::P1080_30,
            ("1080p", _) => crate::output::OutputMode::P1080_60,
            ("1440p", 30) => crate::output::OutputMode::P1440_30,
            ("1440p", _) => crate::output::OutputMode::P1440_60,
            ("2160p", 30) => crate::output::OutputMode::P2160_30,
            ("2160p", _) => crate::output::OutputMode::P2160_60,
            _ => crate::output::OutputMode::P1080_30, // Default
        }
    }

    pub fn get_privacy(&self) -> String {
        self.privacy.lock().clone()
    }

    pub fn set_privacy(&self, privacy: String) {
        *self.privacy.lock() = privacy;
    }

    pub fn get_rtmp_state(&self) -> RtmpState {
        *self.rtmp_state.lock()
    }

    pub fn get_encoder_health(&self) -> EncoderHealth {
        *self.encoder_health.lock()
    }

    pub fn get_last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    pub fn get_dropped_frames(&self) -> u64 {
        *self.dropped_frames.lock()
    }

    /// Get elapsed time since session started (in seconds)
    pub fn get_elapsed_secs(&self) -> f32 {
        self.created_at.lock()
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0)
    }

    pub fn get_observations(&self) -> YouTubeObservations {
        self.observations.lock().clone()
    }

    pub fn is_api_in_flight(&self) -> bool {
        *self.api_in_flight.lock()
    }

    // ========== Latch Queries ==========

    pub fn is_go_live_latched(&self) -> bool {
        self.latches.lock().is_go_live_latched()
    }

    pub fn is_transition_latched(&self) -> bool {
        self.latches.lock().is_transition_latched()
    }

    pub fn must_hold_state(&self) -> bool {
        self.latches.lock().must_hold_state()
    }

    // ========== State Transitions ==========

    /// Start a new session (GoLive clicked)
    /// Guards: state == Idle, not latched
    pub fn start(&self) -> bool {
        let mut state = self.state.lock();
        let latches = self.latches.lock();

        if *state != SessionState::Idle || latches.is_go_live_latched() {
            return false;
        }

        // Reset session data
        *self.broadcast_id.lock() = None;
        *self.stream_id.lock() = None;
        self.secrets.lock().clear();
        *self.last_error.lock() = None;
        *self.step.lock() = SessionStep::CreateBroadcast;
        self.api_retry.lock().reset();
        self.rtmp_retry.lock().reset();
        *self.rtmp_state.lock() = RtmpState::Disconnected;
        *self.encoder_health.lock() = EncoderHealth::Stopped;
        self.observations.lock().reset();
        *self.created_at.lock() = Some(Instant::now());
        *self.dropped_frames.lock() = 0;

        // Set latches
        drop(latches);
        self.latches.lock().latch_go_live(timing::GO_LIVE_LATCH_MS);
        self.latches.lock().set_min_hold(timing::CREATING_HOLD_MS);

        *state = SessionState::Creating;
        log::info!("[YouTube Session] Starting new session");
        true
    }

    /// Retry from failed state
    pub fn retry(&self) -> bool {
        let mut state = self.state.lock();

        if !matches!(*state, SessionState::Failed(_)) {
            return false;
        }

        if self.latches.lock().is_go_live_latched() {
            return false;
        }

        // Reset for retry
        *self.broadcast_id.lock() = None;
        *self.stream_id.lock() = None;
        self.secrets.lock().clear();
        *self.last_error.lock() = None;
        *self.step.lock() = SessionStep::CreateBroadcast;
        self.api_retry.lock().reset();
        self.rtmp_retry.lock().reset();
        *self.rtmp_state.lock() = RtmpState::Disconnected;
        *self.encoder_health.lock() = EncoderHealth::Stopped;
        self.observations.lock().reset();
        *self.created_at.lock() = Some(Instant::now());

        self.latches.lock().latch_go_live(timing::GO_LIVE_LATCH_MS);
        self.latches.lock().set_min_hold(timing::CREATING_HOLD_MS);

        *state = SessionState::Creating;
        log::info!("[YouTube Session] Retrying session");
        true
    }

    /// Advance creation step
    pub fn advance_step(&self) {
        let mut step = self.step.lock();
        *step = step.next();

        if *step == SessionStep::Complete {
            // All creation steps done → Bound
            let mut state = self.state.lock();
            *state = SessionState::Bound;
            drop(state);
            self.latches.lock().set_min_hold(timing::BOUND_HOLD_MS);
            log::info!("[YouTube Session] Creation complete → Bound");
        }
    }

    /// Set broadcast ID (after CreateBroadcast succeeds)
    pub fn set_broadcast_id(&self, id: String) {
        *self.broadcast_id.lock() = Some(id);
    }

    /// Set stream ID (after CreateStream succeeds)
    pub fn set_stream_id(&self, id: String) {
        *self.stream_id.lock() = Some(id);
    }

    /// Set ingestion info (after FetchIngestionInfo succeeds)
    pub fn set_ingestion_info(&self, ingest_url: String, stream_key: String) {
        self.secrets.lock().set(ingest_url, stream_key);
    }

    /// Get RTMP URL for connecting
    pub fn get_rtmp_url(&self) -> Option<String> {
        self.secrets.lock().get_rtmp_url()
    }

    /// Transition to Connecting (from Bound)
    pub fn start_connecting(&self) -> bool {
        let mut state = self.state.lock();
        if *state != SessionState::Bound {
            return false;
        }
        *state = SessionState::Connecting;
        *self.rtmp_state.lock() = RtmpState::Connecting;
        *self.encoder_health.lock() = EncoderHealth::Running;
        drop(state);
        self.latches.lock().set_min_hold(timing::CONNECTING_HOLD_MS);
        log::info!("[YouTube Session] Starting RTMP connection");
        true
    }

    /// RTMP connected → Pushing
    pub fn rtmp_connected(&self) -> bool {
        let mut state = self.state.lock();
        if *state != SessionState::Connecting {
            return false;
        }
        *state = SessionState::Pushing;
        *self.rtmp_state.lock() = RtmpState::Connected;
        // Reset RTMP retry on success
        self.rtmp_retry.lock().reset();
        // Start polling
        *self.next_poll.lock() = Some(Instant::now());
        drop(state);
        self.latches.lock().set_min_hold(timing::PUSHING_HOLD_MS);
        log::info!("[YouTube Session] RTMP connected → Pushing");
        true
    }

    /// RTMP dropped (during Pushing/Testing/Live)
    pub fn rtmp_dropped(&self) {
        let state = self.state.lock().clone();
        if matches!(state, SessionState::Pushing | SessionState::Testing | SessionState::Live) {
            *self.rtmp_state.lock() = RtmpState::Reconnecting;
            self.rtmp_retry.lock().record_failure();
            log::warn!("[YouTube Session] RTMP dropped, will reconnect");
        }
    }

    /// RTMP reconnected
    pub fn rtmp_reconnected(&self) {
        *self.rtmp_state.lock() = RtmpState::Connected;
        self.rtmp_retry.lock().reset();
        log::info!("[YouTube Session] RTMP reconnected");
    }

    /// Check if RTMP can retry
    pub fn can_rtmp_retry(&self) -> bool {
        self.rtmp_retry.lock().can_retry_now()
    }

    /// Check if RTMP retries exhausted
    pub fn is_rtmp_exhausted(&self) -> bool {
        self.rtmp_retry.lock().is_exhausted()
    }

    /// Transition to Testing (from Pushing)
    pub fn transition_to_testing(&self) -> bool {
        let mut state = self.state.lock();
        if *state != SessionState::Pushing {
            return false;
        }
        if self.latches.lock().is_transition_latched() {
            return false;
        }
        *state = SessionState::Testing;
        drop(state);
        self.latches.lock().latch_transition(timing::TRANSITION_COOLDOWN_MS);
        self.latches.lock().set_min_hold(timing::TESTING_HOLD_MS);
        log::info!("[YouTube Session] → Testing");
        true
    }

    /// Transition to Live (from Testing)
    pub fn transition_to_live(&self) -> bool {
        let mut state = self.state.lock();
        if *state != SessionState::Testing {
            return false;
        }
        if self.latches.lock().is_transition_latched() {
            return false;
        }
        *state = SessionState::Live;
        drop(state);
        self.latches.lock().latch_transition(timing::TRANSITION_COOLDOWN_MS);
        self.latches.lock().set_min_hold(timing::LIVE_HOLD_MS);
        log::info!("[YouTube Session] → LIVE!");
        true
    }

    /// Update observations from YouTube polling
    pub fn update_observations(&self, stream_active: bool, broadcast_status: BroadcastStatus) {
        let mut obs = self.observations.lock();
        obs.update_stream_active(stream_active);
        obs.update_broadcast_status(broadcast_status);
        obs.last_poll = Some(Instant::now());
    }

    /// Should we poll YouTube now?
    pub fn should_poll(&self) -> bool {
        let state = self.state.lock().clone();
        if !matches!(state, SessionState::Pushing | SessionState::Testing | SessionState::Live) {
            return false;
        }
        match *self.next_poll.lock() {
            Some(t) => Instant::now() >= t,
            None => false,
        }
    }

    /// Schedule next poll
    pub fn schedule_poll(&self) {
        *self.next_poll.lock() = Some(Instant::now() + Duration::from_millis(timing::POLL_INTERVAL_MS));
    }

    /// Stop the session
    pub fn stop(&self) -> bool {
        let mut state = self.state.lock();
        if !state.can_stop() {
            return false;
        }
        *state = SessionState::Ending;
        // Stop data plane immediately
        *self.rtmp_state.lock() = RtmpState::Disconnected;
        *self.encoder_health.lock() = EncoderHealth::Stopped;
        drop(state);
        self.latches.lock().set_min_hold(timing::ENDING_HOLD_MS);
        log::info!("[YouTube Session] Stopping");
        true
    }

    /// Session ended successfully
    pub fn ended(&self) {
        let mut state = self.state.lock();
        *state = SessionState::Ended;
        // Clear secrets immediately
        self.secrets.lock().clear();
        log::info!("[YouTube Session] Ended");
    }

    /// Session failed
    pub fn fail(&self, error: String) {
        let mut state = self.state.lock();
        *state = SessionState::Failed(error.clone());
        *self.last_error.lock() = Some(error.clone());
        // Stop data plane
        *self.rtmp_state.lock() = RtmpState::Error;
        *self.encoder_health.lock() = EncoderHealth::Stopped;
        // Clear secrets
        self.secrets.lock().clear();
        log::error!("[YouTube Session] Failed: {}", error);
    }

    /// Return to Idle (from Ended or Failed after user acknowledges)
    pub fn reset_to_idle(&self) {
        let mut state = self.state.lock();
        if !matches!(*state, SessionState::Ended | SessionState::Failed(_)) {
            return;
        }
        *state = SessionState::Idle;
        *self.broadcast_id.lock() = None;
        *self.stream_id.lock() = None;
        self.secrets.lock().clear();
        *self.last_error.lock() = None;
        self.latches.lock().clear();
        log::info!("[YouTube Session] Reset to Idle");
    }

    /// Set API in-flight flag
    pub fn set_api_in_flight(&self, in_flight: bool) {
        *self.api_in_flight.lock() = in_flight;
    }

    /// Record API failure for retry
    pub fn record_api_failure(&self) {
        self.api_retry.lock().record_failure();
    }

    /// Check if API can retry
    pub fn can_api_retry(&self) -> bool {
        self.api_retry.lock().can_retry_now()
    }

    /// Check if API retries exhausted
    pub fn is_api_exhausted(&self) -> bool {
        self.api_retry.lock().is_exhausted()
    }

    /// Reset API retry state
    pub fn reset_api_retry(&self) {
        self.api_retry.lock().reset();
    }

    /// Increment dropped frames counter
    pub fn add_dropped_frames(&self, count: u64) {
        *self.dropped_frames.lock() += count;
    }
}
