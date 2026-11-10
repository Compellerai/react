//! YouTube Auth - Connection status via compeller.ai
//!
//! OAuth is handled server-side by compeller.ai:
//! 1. App opens browser to compeller.ai/oauth/youtube/connect
//! 2. Compeller.ai handles Google OAuth, stores tokens
//! 3. App polls compeller.ai/youtube/status for connection status
//! 4. All YouTube API calls go through compeller.ai proxy

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;

use crate::config;
const POLL_TIMEOUT_SECS: u64 = 10;

/// YouTube auth state machine states
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YouTubeAuthState {
    /// Not connected - show "Connect YouTube" button
    LoggedOut,
    /// OAuth flow in progress (browser open)
    Authenticating,
    /// Connected and ready to stream
    Ready,
    /// Connection failed or was revoked
    Error(String),
}

impl Default for YouTubeAuthState {
    fn default() -> Self {
        YouTubeAuthState::LoggedOut
    }
}

/// YouTube channel info from compeller.ai
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YouTubeChannelInfo {
    pub channel_id: String,
    pub channel_name: String,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

/// Response from compeller.ai connected accounts endpoint
#[derive(Debug, Deserialize)]
struct ConnectedAccountsResponse {
    #[serde(default)]
    youtube: Option<YouTubeConnectionStatus>,
}

#[derive(Debug, Deserialize)]
struct YouTubeConnectionStatus {
    connected: bool,
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    channel_name: Option<String>,
    #[serde(default)]
    thumbnail_url: Option<String>,
    #[serde(default)]
    scopes_granted: Vec<String>,
    #[serde(default)]
    error: Option<String>,
}

/// YouTube authentication manager
///
/// Handles connection status polling and OAuth flow initiation.
/// Tokens are stored server-side on compeller.ai.
#[derive(Clone)]
pub struct YouTubeAuth {
    /// Current auth state
    state: Arc<Mutex<YouTubeAuthState>>,
    /// Channel info (if connected)
    channel_info: Arc<Mutex<Option<YouTubeChannelInfo>>>,
    /// Scopes granted by the user
    scopes_granted: Arc<Mutex<Vec<String>>>,
    /// Is a status check in progress?
    checking: Arc<AtomicBool>,
    /// Last status check time
    last_check: Arc<Mutex<Option<Instant>>>,
    /// Last error message
    last_error: Arc<Mutex<Option<String>>>,
    /// Compeller session token (for API auth)
    compeller_token: Arc<Mutex<Option<String>>>,
}

impl Default for YouTubeAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl YouTubeAuth {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(YouTubeAuthState::LoggedOut)),
            channel_info: Arc::new(Mutex::new(None)),
            scopes_granted: Arc::new(Mutex::new(Vec::new())),
            checking: Arc::new(AtomicBool::new(false)),
            last_check: Arc::new(Mutex::new(None)),
            last_error: Arc::new(Mutex::new(None)),
            compeller_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the Compeller session token (required for API calls)
    pub fn set_compeller_token(&self, token: Option<String>) {
        *self.compeller_token.lock() = token;
    }

    /// Get current auth state
    pub fn get_state(&self) -> YouTubeAuthState {
        self.state.lock().clone()
    }

    /// Check if YouTube is connected and ready
    pub fn is_ready(&self) -> bool {
        matches!(*self.state.lock(), YouTubeAuthState::Ready)
    }

    /// Check if currently authenticating
    pub fn is_authenticating(&self) -> bool {
        matches!(*self.state.lock(), YouTubeAuthState::Authenticating)
    }

    /// Get channel info (if connected)
    pub fn get_channel_info(&self) -> Option<YouTubeChannelInfo> {
        self.channel_info.lock().clone()
    }

    /// Get scopes granted
    pub fn get_scopes(&self) -> Vec<String> {
        self.scopes_granted.lock().clone()
    }

    /// Get last error
    pub fn get_last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    /// Start OAuth flow - opens browser to compeller.ai
    pub fn start_connect(&self) {
        // Don't start OAuth if already connected or already authenticating
        let current_state = self.state.lock().clone();
        if matches!(current_state, YouTubeAuthState::Ready | YouTubeAuthState::Authenticating) {
            log::info!("[YouTube Auth] Already connected or authenticating, skipping OAuth");
            return;
        }

        let token = self.compeller_token.lock().clone();

        if token.is_none() {
            *self.state.lock() = YouTubeAuthState::Error("Not logged into Compeller".to_string());
            *self.last_error.lock() = Some("Not logged into Compeller".to_string());
            return;
        }

        *self.state.lock() = YouTubeAuthState::Authenticating;
        *self.last_error.lock() = None;

        // Open browser to YouTube connect endpoint
        let url = format!(
            "{}/youtube/connect?session={}",
            config::oauth_base(),
            token.unwrap()
        );

        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(&url)
                .spawn();
        }

        log::info!("[YouTube Auth] Opening browser for OAuth: {}", url);
    }

    /// Check connection status (poll compeller.ai)
    /// Call this periodically when in Authenticating state, or to refresh status
    pub fn check_status(&self) {
        if self.checking.load(Ordering::Relaxed) {
            return; // Already checking
        }

        let token = match self.compeller_token.lock().clone() {
            Some(t) => t,
            None => {
                *self.state.lock() = YouTubeAuthState::LoggedOut;
                return;
            }
        };

        self.checking.store(true, Ordering::Release);

        let state = self.state.clone();
        let channel_info = self.channel_info.clone();
        let scopes_granted = self.scopes_granted.clone();
        let last_check = self.last_check.clone();
        let last_error = self.last_error.clone();
        let checking = self.checking.clone();

        std::thread::spawn(move || {
            let result = Self::do_check_status(&token);

            match result {
                Ok(status) => {
                    if status.connected {
                        // Connected!
                        *state.lock() = YouTubeAuthState::Ready;
                        *channel_info.lock() = Some(YouTubeChannelInfo {
                            channel_id: status.channel_id.unwrap_or_default(),
                            channel_name: status.channel_name.unwrap_or_else(|| "Unknown".to_string()),
                            thumbnail_url: status.thumbnail_url,
                        });
                        *scopes_granted.lock() = status.scopes_granted;
                        *last_error.lock() = None;
                        log::info!("[YouTube Auth] Connected to YouTube");
                    } else if let Some(err) = status.error {
                        // Error from server
                        *state.lock() = YouTubeAuthState::Error(err.clone());
                        *last_error.lock() = Some(err);
                        *channel_info.lock() = None;
                    } else {
                        // Not connected, no error (user hasn't completed OAuth yet)
                        // Keep current state if Authenticating, otherwise LoggedOut
                        let current = state.lock().clone();
                        if !matches!(current, YouTubeAuthState::Authenticating) {
                            *state.lock() = YouTubeAuthState::LoggedOut;
                        }
                    }
                }
                Err(e) => {
                    log::error!("[YouTube Auth] Status check failed: {}", e);
                    // Don't change state on network error during auth
                    // (user might still be completing OAuth in browser)
                }
            }

            *last_check.lock() = Some(Instant::now());
            checking.store(false, Ordering::Release);
        });
    }

    /// Perform the actual status check (blocking)
    fn do_check_status(token: &str) -> anyhow::Result<YouTubeConnectionStatus> {
        let url = format!("{}/youtube/status", config::oauth_base());

        let response = ureq::get(&url)
            .timeout(Duration::from_secs(POLL_TIMEOUT_SECS))
            .set("Authorization", &format!("Bearer {}", token))
            .call()
            .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

        let resp: ConnectedAccountsResponse = response
            .into_json()
            .map_err(|e| anyhow::anyhow!("Invalid response: {}", e))?;

        Ok(resp.youtube.unwrap_or(YouTubeConnectionStatus {
            connected: false,
            channel_id: None,
            channel_name: None,
            thumbnail_url: None,
            scopes_granted: Vec::new(),
            error: None,
        }))
    }

    /// Disconnect YouTube account
    pub fn disconnect(&self) {
        let token = match self.compeller_token.lock().clone() {
            Some(t) => t,
            None => return,
        };

        let state = self.state.clone();
        let channel_info = self.channel_info.clone();
        let scopes_granted = self.scopes_granted.clone();

        std::thread::spawn(move || {
            let url = format!("{}/youtube/disconnect", config::oauth_base());

            let result = ureq::post(&url)
                .timeout(Duration::from_secs(POLL_TIMEOUT_SECS))
                .set("Authorization", &format!("Bearer {}", token))
                .call();

            match result {
                Ok(_) => {
                    *state.lock() = YouTubeAuthState::LoggedOut;
                    *channel_info.lock() = None;
                    *scopes_granted.lock() = Vec::new();
                    log::info!("[YouTube Auth] Disconnected from YouTube");
                }
                Err(e) => {
                    log::error!("[YouTube Auth] Disconnect failed: {}", e);
                }
            }
        });
    }

    /// Reset to logged out state (e.g., when Compeller logs out)
    pub fn reset(&self) {
        *self.state.lock() = YouTubeAuthState::LoggedOut;
        *self.channel_info.lock() = None;
        *self.scopes_granted.lock() = Vec::new();
        *self.last_error.lock() = None;
        *self.compeller_token.lock() = None;
    }

    /// Should we poll for status?
    pub fn should_poll(&self) -> bool {
        // Poll if authenticating and haven't checked recently
        if matches!(*self.state.lock(), YouTubeAuthState::Authenticating) {
            let last = self.last_check.lock().clone();
            return match last {
                Some(t) => t.elapsed() > Duration::from_millis(2000), // Poll every 2s during auth
                None => true,
            };
        }
        false
    }
}
