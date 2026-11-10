//! Connected Accounts Framework
//!
//! Generic provider abstraction for OAuth-connected accounts (YouTube, etc.)
//! This provides the foundation for YouTube Live Tier 2 integration.
//!
//! State machine: Disconnected → Connecting → Connected | Error
//!
//! Security notes:
//! - Tokens are stored in SQLite (can upgrade to Keychain later)
//! - Refresh tokens are never logged
//! - Scopes are tracked for re-auth prompts

use serde::{Deserialize, Serialize};

/// Provider identifiers for connected accounts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    /// Google/YouTube OAuth
    Google,
    // Future providers can be added here:
    // Twitch,
    // Facebook,
}

impl ProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderId::Google => "google",
            // ProviderId::Twitch => "twitch",
            // ProviderId::Facebook => "facebook",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderId::Google => "YouTube",
            // ProviderId::Twitch => "Twitch",
            // ProviderId::Facebook => "Facebook",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "google" => Some(ProviderId::Google),
            // "twitch" => Some(ProviderId::Twitch),
            // "facebook" => Some(ProviderId::Facebook),
            _ => None,
        }
    }
}

/// State machine for connected account status
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConnectedAccountState {
    /// Not connected, "Connect" button shown
    Disconnected,
    /// OAuth flow in progress
    Connecting,
    /// Successfully connected with valid tokens
    Connected,
    /// Connection failed or tokens expired
    Error(String),
    /// Tokens expired, needs re-auth
    NeedsReauth,
}

impl Default for ConnectedAccountState {
    fn default() -> Self {
        ConnectedAccountState::Disconnected
    }
}

/// A connected OAuth account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedAccount {
    /// Provider identifier
    pub provider: ProviderId,

    /// Current connection state
    pub state: ConnectedAccountState,

    /// OAuth refresh token (for getting new access tokens)
    /// Note: This is sensitive - never log it!
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,

    /// Current access token (short-lived)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,

    /// When the access token expires (Unix timestamp ms)
    #[serde(default)]
    pub token_expires_at: Option<u64>,

    /// OAuth scopes that were granted
    #[serde(default)]
    pub scopes_granted: Vec<String>,

    /// Provider-specific identifier (e.g., YouTube channel ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_user_id: Option<String>,

    /// Provider-specific display name (e.g., channel name)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_display_name: Option<String>,

    /// When the account was first connected (Unix timestamp ms)
    #[serde(default)]
    pub connected_at: u64,

    /// Last successful token refresh (Unix timestamp ms)
    #[serde(default)]
    pub last_refreshed_at: u64,
}

impl ConnectedAccount {
    /// Create a new disconnected account placeholder
    pub fn new_disconnected(provider: ProviderId) -> Self {
        Self {
            provider,
            state: ConnectedAccountState::Disconnected,
            refresh_token: None,
            access_token: None,
            token_expires_at: None,
            scopes_granted: Vec::new(),
            provider_user_id: None,
            provider_display_name: None,
            connected_at: 0,
            last_refreshed_at: 0,
        }
    }

    /// Check if the access token is expired or about to expire (within 5 min)
    pub fn is_token_expired(&self) -> bool {
        match self.token_expires_at {
            Some(expires_at) => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                // Consider expired if within 5 minutes of expiry
                expires_at < now_ms + (5 * 60 * 1000)
            }
            None => true, // No expiry set = treat as expired
        }
    }

    /// Check if we have a refresh token available
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }

    /// Check if we have the required scopes
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes_granted.iter().any(|s| s == scope)
    }

    /// Check if we have all required scopes for YouTube Live
    pub fn has_youtube_live_scopes(&self) -> bool {
        // Required scopes for YouTube Live streaming
        const REQUIRED_SCOPES: &[&str] = &[
            "https://www.googleapis.com/auth/youtube",
            "https://www.googleapis.com/auth/youtube.force-ssl",
        ];

        REQUIRED_SCOPES.iter().all(|scope| self.has_scope(scope))
    }
}

/// Required OAuth scopes for YouTube Live streaming
pub mod youtube_scopes {
    /// Full YouTube access (required for live streaming)
    pub const YOUTUBE: &str = "https://www.googleapis.com/auth/youtube";
    /// YouTube SSL (required for API access)
    pub const YOUTUBE_FORCE_SSL: &str = "https://www.googleapis.com/auth/youtube.force-ssl";
    /// Read-only YouTube access (for channel info)
    pub const YOUTUBE_READONLY: &str = "https://www.googleapis.com/auth/youtube.readonly";

    /// Get all scopes required for YouTube Live
    pub fn required_scopes() -> Vec<&'static str> {
        vec![YOUTUBE, YOUTUBE_FORCE_SSL]
    }
}

/// Server-side YouTube connection URLs
///
/// YouTube OAuth is handled by compeller.ai SaaS backend:
/// 1. App opens browser to compeller.ai YouTube connect endpoint
/// 2. Compeller.ai handles the OAuth flow with Google
/// 3. Tokens are stored server-side (more secure)
/// 4. App polls /youtube/status to check connection status
/// 5. Streaming API calls go through compeller.ai which proxies to YouTube
pub mod oauth_urls {
    use crate::config;

    /// Get the URL to initiate YouTube connection
    /// Opens in user's browser - server handles the OAuth flow
    pub fn youtube_connect(session_token: &str) -> String {
        format!("{}/youtube/connect?session={}", config::oauth_base(), session_token)
    }

    /// Get the URL to disconnect YouTube account
    pub fn youtube_disconnect(session_token: &str) -> String {
        format!("{}/youtube/disconnect?session={}", config::oauth_base(), session_token)
    }

    /// Get the URL to check connected accounts status
    pub fn connected_accounts_status() -> String {
        format!("{}/youtube/status", config::oauth_base())
    }
}
