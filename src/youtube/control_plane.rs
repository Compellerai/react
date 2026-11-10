//! YouTube Control Plane - API calls via compeller.ai proxy
//!
//! All YouTube API calls go through compeller.ai which:
//! - Holds the OAuth tokens server-side
//! - Handles token refresh automatically
//! - Provides a simplified API for the desktop app
//!
//! The desktop app never sees YouTube OAuth tokens directly.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use parking_lot::Mutex;

use super::session::{LiveSession, SessionState, SessionStep};
use super::{BroadcastStatus, StreamStatus};

use crate::config;
const API_TIMEOUT_SECS: u64 = 30;

/// Broadcast creation request
#[derive(Debug, Serialize)]
struct CreateBroadcastRequest {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    privacy: String, // "public", "unlisted", "private"
    #[serde(skip_serializing_if = "Option::is_none")]
    category_id: Option<String>,
    resolution: String, // "1080p", "1440p", or "2160p"
    #[serde(rename = "goLiveNow")]
    go_live_now: bool, // true = instant live, false = scheduled
}

/// Broadcast creation response
#[derive(Debug, Deserialize)]
struct CreateBroadcastResponse {
    #[serde(default)]
    broadcast_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Stream creation response
#[derive(Debug, Deserialize)]
struct CreateStreamResponse {
    #[serde(default)]
    stream_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Bind response
#[derive(Debug, Deserialize)]
struct BindResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

/// Ingestion info response
#[derive(Debug, Deserialize)]
struct IngestionInfoResponse {
    #[serde(default)]
    ingest_url: Option<String>,
    #[serde(default)]
    stream_key: Option<String>,
    #[serde(default)]
    backup_ingest_url: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Status poll response
#[derive(Debug, Deserialize)]
struct StatusResponse {
    stream_status: String,
    broadcast_status: String,
    #[serde(default)]
    error: Option<String>,
}

/// Transition response
#[derive(Debug, Deserialize)]
struct TransitionResponse {
    success: bool,
    #[serde(default)]
    new_status: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// End broadcast response
#[derive(Debug, Deserialize)]
struct EndResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

/// YouTube Control Plane API client
#[derive(Clone)]
pub struct YouTubeControlPlane {
    /// Compeller session token for authentication
    compeller_token: Arc<Mutex<Option<String>>>,
}

impl Default for YouTubeControlPlane {
    fn default() -> Self {
        Self::new()
    }
}

impl YouTubeControlPlane {
    pub fn new() -> Self {
        Self {
            compeller_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the Compeller session token
    pub fn set_compeller_token(&self, token: Option<String>) {
        *self.compeller_token.lock() = token;
    }

    fn get_token(&self) -> Option<String> {
        self.compeller_token.lock().clone()
    }

    /// Execute the creation pipeline step by step
    /// This is called repeatedly from the state machine until all steps complete
    pub fn execute_creation_step(&self, session: &LiveSession) {
        let state = session.get_state();
        if state != SessionState::Creating {
            return;
        }

        if session.is_api_in_flight() {
            return; // Wait for current call to complete
        }

        let step = session.get_step();
        if step == SessionStep::Complete {
            return; // All done
        }

        let token = match self.get_token() {
            Some(t) => t,
            None => {
                session.fail("Not logged in to Compeller".to_string());
                return;
            }
        };

        session.set_api_in_flight(true);

        match step {
            SessionStep::CreateBroadcast => {
                self.create_broadcast(token, session.clone());
            }
            SessionStep::CreateStream => {
                self.create_stream(token, session.clone());
            }
            SessionStep::BindBroadcastToStream => {
                let broadcast_id = session.get_broadcast_id();
                let stream_id = session.get_stream_id();
                if let (Some(bid), Some(sid)) = (broadcast_id, stream_id) {
                    self.bind_broadcast_to_stream(token, bid, sid, session.clone());
                } else {
                    session.fail("Missing broadcast or stream ID".to_string());
                }
            }
            SessionStep::FetchIngestionInfo => {
                if let Some(stream_id) = session.get_stream_id() {
                    self.fetch_ingestion_info(token, stream_id, session.clone());
                } else {
                    session.fail("Missing stream ID".to_string());
                }
            }
            SessionStep::Complete => {}
        }
    }

    /// Create broadcast (Step 0)
    fn create_broadcast(&self, token: String, session: LiveSession) {
        std::thread::spawn(move || {
            let url = format!("{}/youtube/broadcast/create", config::api_base());

            // Get config from the session
            let title = session.get_title();
            let privacy = session.get_privacy();
            let resolution = session.get_resolution();

            let request = CreateBroadcastRequest {
                title,
                description: Some("Live stream from REACT by Compeller.ai".to_string()),
                privacy,
                category_id: Some("10".to_string()), // Music category
                resolution,
                go_live_now: true, // Go live immediately, not scheduled
            };

            log::info!("[YouTube] Creating broadcast at: {}", url);

            let result = ureq::post(&url)
                .timeout(Duration::from_secs(API_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .set("Content-Type", "application/json")
                .send_json(&request);

            session.set_api_in_flight(false);

            match result {
                Ok(response) => {
                    let status = response.status();
                    // Read body as string first for debugging
                    let body = response.into_string().unwrap_or_else(|e| format!("(failed to read body: {})", e));
                    log::info!("[YouTube] Broadcast create response: status={}, body={}", status, body);

                    if body.is_empty() {
                        Self::handle_api_error(&session, "Empty response from server");
                        return;
                    }

                    match serde_json::from_str::<CreateBroadcastResponse>(&body) {
                        Ok(resp) => {
                            if let Some(err) = resp.error {
                                Self::handle_api_error(&session, &err);
                            } else if let Some(broadcast_id) = resp.broadcast_id {
                                log::info!("[YouTube] Broadcast created: {}", broadcast_id);
                                session.set_broadcast_id(broadcast_id);
                                session.reset_api_retry();
                                session.advance_step();
                            } else {
                                Self::handle_api_error(&session, &format!("No broadcast_id in response: {}", body));
                            }
                        }
                        Err(e) => {
                            Self::handle_api_error(&session, &format!("Invalid JSON: {} - body: {}", e, body));
                        }
                    }
                }
                Err(ureq::Error::Status(code, response)) => {
                    let body = response.into_string().unwrap_or_else(|_| "(no body)".to_string());
                    log::error!("[YouTube] Broadcast create failed: HTTP {} - {}", code, body);
                    Self::handle_api_error(&session, &format!("HTTP {}: {}", code, body));
                }
                Err(e) => {
                    Self::handle_api_error(&session, &format!("Request failed: {}", e));
                }
            }
        });
    }

    /// Create stream (Step 1)
    fn create_stream(&self, token: String, session: LiveSession) {
        std::thread::spawn(move || {
            let url = format!("{}/youtube/stream/create", config::api_base());

            // Get config from the session
            let resolution = session.get_resolution();
            let frame_rate = session.get_frame_rate();

            let body = serde_json::json!({
                "title": "REACT Stream",
                "frame_rate": frame_rate,
                "resolution": resolution,
            });

            let result = ureq::post(&url)
                .timeout(Duration::from_secs(API_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .set("Content-Type", "application/json")
                .send_json(&body);

            session.set_api_in_flight(false);

            match result {
                Ok(response) => {
                    let status = response.status();
                    let body = response.into_string().unwrap_or_else(|e| format!("(failed to read body: {})", e));
                    log::info!("[YouTube] Stream create response: status={}, body={}", status, body);

                    match serde_json::from_str::<CreateStreamResponse>(&body) {
                        Ok(resp) => {
                            if let Some(err) = resp.error {
                                Self::handle_api_error(&session, &err);
                            } else if let Some(stream_id) = resp.stream_id {
                                session.set_stream_id(stream_id);
                                session.reset_api_retry();
                                session.advance_step();
                                log::info!("[YouTube] Stream created");
                            } else {
                                Self::handle_api_error(&session, &format!("No stream_id in response: {}", body));
                            }
                        }
                        Err(e) => {
                            Self::handle_api_error(&session, &format!("Invalid JSON: {} - body: {}", e, body));
                        }
                    }
                }
                Err(ureq::Error::Status(code, response)) => {
                    let body = response.into_string().unwrap_or_else(|_| "(no body)".to_string());
                    log::error!("[YouTube] Stream create failed: HTTP {} - {}", code, body);
                    Self::handle_api_error(&session, &format!("HTTP {}: {}", code, body));
                }
                Err(e) => {
                    Self::handle_api_error(&session, &format!("Request failed: {}", e));
                }
            }
        });
    }

    /// Bind broadcast to stream (Step 2)
    fn bind_broadcast_to_stream(&self, token: String, broadcast_id: String, stream_id: String, session: LiveSession) {
        std::thread::spawn(move || {
            let url = format!("{}/youtube/broadcast/bind", config::api_base());

            let body = serde_json::json!({
                "broadcast_id": broadcast_id,
                "stream_id": stream_id,
            });

            log::info!("[YouTube] Binding broadcast {} to stream {}", broadcast_id, stream_id);

            let result = ureq::post(&url)
                .timeout(Duration::from_secs(API_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .set("Content-Type", "application/json")
                .send_json(&body);

            session.set_api_in_flight(false);

            match result {
                Ok(response) => {
                    let status = response.status();
                    let body = response.into_string().unwrap_or_else(|e| format!("(failed to read body: {})", e));
                    log::info!("[YouTube] Bind response: status={}, body={}", status, body);

                    match serde_json::from_str::<BindResponse>(&body) {
                        Ok(resp) => {
                            if let Some(err) = resp.error {
                                Self::handle_api_error(&session, &err);
                            } else if resp.success {
                                session.reset_api_retry();
                                session.advance_step();
                                log::info!("[YouTube] Broadcast bound to stream");
                            } else {
                                Self::handle_api_error(&session, &format!("Bind failed: {}", body));
                            }
                        }
                        Err(e) => {
                            Self::handle_api_error(&session, &format!("Invalid JSON: {} - body: {}", e, body));
                        }
                    }
                }
                Err(ureq::Error::Status(code, response)) => {
                    let body = response.into_string().unwrap_or_else(|_| "(no body)".to_string());
                    log::error!("[YouTube] Bind failed: HTTP {} - {}", code, body);
                    Self::handle_api_error(&session, &format!("HTTP {}: {}", code, body));
                }
                Err(e) => {
                    Self::handle_api_error(&session, &format!("Request failed: {}", e));
                }
            }
        });
    }

    /// Fetch ingestion info (Step 3)
    fn fetch_ingestion_info(&self, token: String, stream_id: String, session: LiveSession) {
        std::thread::spawn(move || {
            let url = format!("{}/youtube/stream/{}/ingestion", config::api_base(), stream_id);

            let result = ureq::get(&url)
                .timeout(Duration::from_secs(API_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .call();

            session.set_api_in_flight(false);

            match result {
                Ok(response) => {
                    match response.into_json::<IngestionInfoResponse>() {
                        Ok(resp) => {
                            if let Some(err) = resp.error {
                                Self::handle_api_error(&session, &err);
                            } else if let (Some(ingest_url), Some(stream_key)) = (resp.ingest_url, resp.stream_key) {
                                session.set_ingestion_info(ingest_url, stream_key);
                                session.reset_api_retry();
                                session.advance_step();
                                log::info!("[YouTube] Ingestion info received");
                            } else {
                                Self::handle_api_error(&session, "Missing ingest_url or stream_key in response");
                            }
                        }
                        Err(e) => {
                            Self::handle_api_error(&session, &format!("Invalid response: {}", e));
                        }
                    }
                }
                Err(e) => {
                    Self::handle_api_error(&session, &format!("Request failed: {}", e));
                }
            }
        });
    }

    /// Poll YouTube status - DISABLED to conserve API quota
    /// With enableAutoStart=true, YouTube auto-transitions to live.
    /// RTMP connection status is sufficient to know we're streaming.
    #[allow(unused)]
    pub fn poll_status(&self, _session: &LiveSession) {
        // Polling disabled - not needed with enableAutoStart=true
        // The RTMP connection status tells us we're streaming
        // YouTube handles the rest automatically
    }

    /// Request transition to testing
    pub fn transition_to_testing(&self, session: &LiveSession) {
        let token = match self.get_token() {
            Some(t) => t,
            None => return,
        };

        let broadcast_id = match session.get_broadcast_id() {
            Some(id) => id,
            None => return,
        };

        let session = session.clone();

        std::thread::spawn(move || {
            let url = format!("{}/youtube/broadcast/{}/transition", config::api_base(), broadcast_id);

            let body = serde_json::json!({
                "status": "testing",
            });

            let result = ureq::post(&url)
                .timeout(Duration::from_secs(API_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .set("Content-Type", "application/json")
                .send_json(&body);

            match result {
                Ok(response) => {
                    if let Ok(resp) = response.into_json::<TransitionResponse>() {
                        if resp.success {
                            log::info!("[YouTube] Transition to testing requested");
                        } else if let Some(err) = resp.error {
                            log::warn!("[YouTube] Transition to testing failed: {}", err);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[YouTube] Transition to testing request failed: {}", e);
                }
            }
        });
    }

    /// Request transition to live
    pub fn transition_to_live(&self, session: &LiveSession) {
        let token = match self.get_token() {
            Some(t) => t,
            None => return,
        };

        let broadcast_id = match session.get_broadcast_id() {
            Some(id) => id,
            None => return,
        };

        let session = session.clone();

        std::thread::spawn(move || {
            let url = format!("{}/youtube/broadcast/{}/transition", config::api_base(), broadcast_id);

            let body = serde_json::json!({
                "status": "live",
            });

            let result = ureq::post(&url)
                .timeout(Duration::from_secs(API_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .set("Content-Type", "application/json")
                .send_json(&body);

            match result {
                Ok(response) => {
                    if let Ok(resp) = response.into_json::<TransitionResponse>() {
                        if resp.success {
                            log::info!("[YouTube] Transition to live requested");
                        } else if let Some(err) = resp.error {
                            log::warn!("[YouTube] Transition to live failed: {}", err);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[YouTube] Transition to live request failed: {}", e);
                }
            }
        });
    }

    /// End broadcast
    pub fn end_broadcast(&self, session: &LiveSession) {
        let token = match self.get_token() {
            Some(t) => t,
            None => {
                session.ended();
                return;
            }
        };

        let broadcast_id = match session.get_broadcast_id() {
            Some(id) => id,
            None => {
                session.ended();
                return;
            }
        };

        let session = session.clone();

        std::thread::spawn(move || {
            let url = format!("{}/youtube/broadcast/{}/end", config::api_base(), broadcast_id);

            let result = ureq::post(&url)
                .timeout(Duration::from_secs(API_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .call();

            match result {
                Ok(response) => {
                    if let Ok(resp) = response.into_json::<EndResponse>() {
                        if resp.success {
                            log::info!("[YouTube] Broadcast ended");
                        } else if let Some(err) = resp.error {
                            log::warn!("[YouTube] End broadcast failed: {}", err);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[YouTube] End broadcast request failed: {}", e);
                }
            }

            // Always mark as ended, even if API call fails
            // (the broadcast will timeout on YouTube's side)
            session.ended();
        });
    }

    /// Handle API error with retry logic
    fn handle_api_error(session: &LiveSession, error: &str) {
        log::error!("[YouTube API] Error: {}", error);

        if session.can_api_retry() {
            session.record_api_failure();
            log::info!("[YouTube API] Will retry");
        } else {
            // Sanitize error for user display
            let user_error = if error.contains("401") || error.contains("403") {
                "Authentication error - please reconnect YouTube".to_string()
            } else if error.contains("quota") || error.contains("rate") {
                "Rate limit exceeded - please try again later".to_string()
            } else if error.contains("timeout") || error.contains("network") {
                "Network error - please check your connection".to_string()
            } else {
                "Failed to create broadcast".to_string()
            };
            session.fail(user_error);
        }
    }
}
