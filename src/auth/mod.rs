//! Authentication module for Compeller.ai login
//!
//! Provides:
//! - State machine for auth flow (Anon → LoggingIn → Authed | Error)
//! - API calls to compeller.ai/api/login
//! - Token persistence in SQLite database
//! - Session refresh handling
//! - Keeps RTMP key mode working without auth (offline/local usage)
//!
//! Future: YouTube OAuth will be handled via the connected accounts framework.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub mod connected;
pub mod license;
pub use connected::{ConnectedAccount, ConnectedAccountState, ProviderId};

use crate::config;
const LOGIN_TIMEOUT_SECS: u64 = 30;
/// Browser sign-in handoff: give up if the web flow isn't completed in time.
const DESKTOP_SIGNIN_TIMEOUT_SECS: u64 = 300;
/// Browser sign-in handoff: frame-loop poll cadence (mirrors YouTubeAuth).
const DESKTOP_POLL_INTERVAL_MS: u128 = 2000;

/// Auth state machine states
#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    /// Not logged in, anonymous usage
    Anon,
    /// Login in progress
    LoggingIn,
    /// Successfully authenticated
    Authed,
    /// Login failed with error message
    Error(String),
}

impl Default for AuthState {
    fn default() -> Self {
        AuthState::Anon
    }
}

/// User information from successful login
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    pub username: String,
    pub roles: Vec<String>,
}

/// Value-driven entitlements derived from signed license blob.
///
/// These values are USED directly in computations (render multiplier, shader uniform,
/// stream count, recording countdown), not boolean gates. Patching a single check
/// won't help — every usage site consumes the value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entitlements {
    /// Account ID for forensic fingerprinting (0 = anonymous/no fingerprint)
    pub account_id: u32,
    /// Plan code identifier ("FREE", "STARTER", "PRO", "ENTERPRISE")
    pub plan_code: String,
    /// GPU render resolution multiplier (0.5 = 540p cap, 0.75, 1.0 = full)
    pub render_scale: f32,
    /// Maximum FPS (30 or 60)
    pub max_fps: u32,
    /// Show watermark overlay (driven as shader uniform, not a boolean check)
    pub watermark: bool,
    /// Maximum recording minutes (0 = unlimited)
    pub max_record_minutes: u32,
    /// Maximum concurrent streams (0 = none)
    pub max_streams: u32,
    /// Lighting control enabled (OSC, Hue, WLED, DMX)
    pub lighting_enabled: bool,
    /// YouTube Live streaming enabled
    pub youtube_enabled: bool,
}

impl Default for Entitlements {
    /// Degraded defaults for free/anonymous users
    fn default() -> Self {
        Self {
            account_id: 0,
            plan_code: "FREE".into(),
            render_scale: 0.5,
            max_fps: 30,
            watermark: true,
            max_record_minutes: 5,
            max_streams: 0,
            lighting_enabled: false,
            youtube_enabled: false,
        }
    }
}

impl Entitlements {
    /// Create entitlements from an enriched login response plan code.
    ///
    /// The signed license blob remains authoritative when available, but login
    /// already carries plan metadata. Use it as a safe fallback so transient
    /// license endpoint/signature problems do not make paid users look FREE.
    pub fn from_plan_code(plan_code: Option<&str>) -> Self {
        let Some(plan_code) = plan_code else {
            return Self::unknown_plan();
        };
        let normalized = plan_code.to_ascii_uppercase();
        match normalized.as_str() {
            "ENTERPRISE" => Self {
                account_id: 0,
                plan_code: "ENTERPRISE".into(),
                render_scale: 1.0,
                max_fps: 60,
                watermark: false,
                max_record_minutes: 0,
                max_streams: 10,
                lighting_enabled: true,
                youtube_enabled: true,
            },
            "PRO" => Self {
                account_id: 0,
                plan_code: "PRO".into(),
                render_scale: 1.0,
                max_fps: 60,
                watermark: false,
                max_record_minutes: 0,
                max_streams: 3,
                lighting_enabled: true,
                youtube_enabled: true,
            },
            "STARTER" => Self {
                account_id: 0,
                plan_code: "STARTER".into(),
                render_scale: 0.75,
                max_fps: 60,
                watermark: false,
                max_record_minutes: 30,
                max_streams: 1,
                lighting_enabled: false,
                youtube_enabled: true,
            },
            _ => Self::unknown_plan(),
        }
    }


    /// Degraded entitlements when authenticated plan metadata is unavailable.
    /// Keep the safe FREE limits, but do not label the account as FREE in UI.
    pub fn unknown_plan() -> Self {
        Self {
            plan_code: String::new(),
            ..Self::default()
        }
    }

    /// Create entitlements for an authenticated free user (same as default)
    pub fn for_authenticated_user() -> Self {
        Self::default()
    }

    /// Create entitlements from user roles (legacy compat — returns defaults,
    /// real entitlements come from the license blob)
    pub fn from_roles(_roles: &[String]) -> Self {
        Self::for_authenticated_user()
    }

    /// Backwards-compatible check for YouTube streaming
    pub fn can_stream_youtube(&self) -> bool {
        self.youtube_enabled
    }
}

/// Credentials for login (username/email + password)
#[derive(Debug, Clone)]
pub struct LoginCredentials {
    pub username: String,
    pub password: String,
}

/// API login response (enriched with account + plan details)
#[derive(Debug, Deserialize)]
struct LoginResponse {
    username: String,
    #[serde(default)]
    roles: Vec<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    // Enriched fields from CompellerAccessTokenJsonRenderer
    #[serde(default)]
    account_id: Option<i64>,
    #[serde(default)]
    plan_code: Option<String>,
    #[serde(default)]
    plan_display_name: Option<String>,
    #[serde(default)]
    subscription_status: Option<String>,
}

/// Main auth manager - thread-safe, clonable
#[derive(Clone)]
pub struct AuthManager {
    /// Current auth state
    state: Arc<Mutex<AuthState>>,
    /// Current user (if authenticated)
    user: Arc<Mutex<Option<AuthUser>>>,
    /// Access token (if authenticated)
    token: Arc<Mutex<Option<String>>>,
    /// Refresh token (for session refresh)
    refresh_token: Arc<Mutex<Option<String>>>,
    /// User entitlements
    entitlements: Arc<Mutex<Entitlements>>,
    /// Is a login operation in progress?
    is_logging_in: Arc<AtomicBool>,
    /// Error message from last failed operation
    last_error: Arc<Mutex<Option<String>>>,
    /// Connected accounts (YouTube, etc.)
    connected_accounts: Arc<Mutex<Vec<ConnectedAccount>>>,
    /// In-flight browser sign-in handoff (Sign in with Google): the one-time
    /// code REACT polls against SaaS while the user completes the web flow.
    desktop_signin: Arc<Mutex<Option<DesktopSignIn>>>,
    /// One-shot guard so only one desktop-signin poll runs at a time.
    desktop_polling: Arc<AtomicBool>,
}

/// State of an in-flight browser sign-in handoff.
struct DesktopSignIn {
    /// One-time code issued by SaaS; the browser session authorizes it.
    code: String,
    /// When the flow started — abandoned after DESKTOP_SIGNIN_TIMEOUT_SECS.
    started: std::time::Instant,
    /// Last poll attempt (frame-loop gate, ~2s cadence like YouTubeAuth).
    last_poll: std::time::Instant,
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(AuthState::Anon)),
            user: Arc::new(Mutex::new(None)),
            token: Arc::new(Mutex::new(None)),
            refresh_token: Arc::new(Mutex::new(None)),
            entitlements: Arc::new(Mutex::new(Entitlements::default())),
            is_logging_in: Arc::new(AtomicBool::new(false)),
            last_error: Arc::new(Mutex::new(None)),
            connected_accounts: Arc::new(Mutex::new(Vec::new())),
            desktop_signin: Arc::new(Mutex::new(None)),
            desktop_polling: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get current auth state
    pub fn get_state(&self) -> AuthState {
        self.state.lock().clone()
    }

    /// Get current user (if authenticated)
    pub fn get_user(&self) -> Option<AuthUser> {
        self.user.lock().clone()
    }

    /// Get access token (if authenticated)
    pub fn get_token(&self) -> Option<String> {
        self.token.lock().clone()
    }

    /// Whether the logged-in user is a Compeller admin (from the roles stored at
    /// login / restored from the DB). Lets the client grant admin-only affordances
    /// — e.g. editing shared stages — without depending on a possibly-stale
    /// per-stage `editable` flag. The server still enforces the same gate on write.
    pub fn is_admin(&self) -> bool {
        self.user
            .lock()
            .as_ref()
            .map_or(false, |u| u.roles.iter().any(|r| r == "ROLE_ADMIN"))
    }

    /// Get user entitlements
    pub fn get_entitlements(&self) -> Entitlements {
        self.entitlements.lock().clone()
    }

    /// Check if user is authenticated
    pub fn is_authenticated(&self) -> bool {
        matches!(*self.state.lock(), AuthState::Authed)
    }

    /// Check if login is in progress
    pub fn is_logging_in(&self) -> bool {
        self.is_logging_in.load(Ordering::Relaxed)
    }

    /// Get last error message
    pub fn get_last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    /// Clear last error
    pub fn clear_error(&self) {
        *self.last_error.lock() = None;
        // If we were in error state, go back to anon
        let mut state = self.state.lock();
        if matches!(*state, AuthState::Error(_)) {
            *state = AuthState::Anon;
        }
    }

    /// Restore auth from saved credentials (call on app startup)
    /// This restores the local state but doesn't validate with server yet
    pub fn restore_from_db(&self, db: &crate::db::Database) {
        if let Some((user, token, refresh, roles_str)) = db.get_auth_credentials() {
            let roles: Vec<String> = roles_str
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            *self.user.lock() = Some(AuthUser {
                username: user,
                roles: roles.clone(),
            });
            *self.token.lock() = Some(token);
            *self.refresh_token.lock() = refresh;

            // Restore entitlements from cached license blob (with 7-day grace)
            let restored_entitlements = license::load_cached_entitlements(db)
                .unwrap_or_else(|| Entitlements::from_roles(&roles));
            log::info!("[Auth] Restored entitlements: plan={}", restored_entitlements.plan_code);
            *self.entitlements.lock() = restored_entitlements;
            *self.state.lock() = AuthState::Authed;

            log::info!("[Auth] Restored session from database");
        }

        // Load connected accounts
        if let Ok(accounts) = db.get_connected_accounts() {
            *self.connected_accounts.lock() = accounts;
            log::info!("[Auth] Loaded {} connected accounts", self.connected_accounts.lock().len());
        }
    }

    /// Validate the stored token with the server (call after restore_from_db)
    /// If token is invalid, clears auth state so user must login again
    /// Spawns background thread, doesn't block
    pub fn validate_token_async(&self, db: crate::db::Database) {
        let token = match self.token.lock().clone() {
            Some(t) => t,
            None => return, // No token to validate
        };

        let state = self.state.clone();
        let user = self.user.clone();
        let token_store = self.token.clone();
        let refresh_token = self.refresh_token.clone();
        let entitlements = self.entitlements.clone();
        let last_error = self.last_error.clone();

        std::thread::spawn(move || {
            match Self::do_validate_token(&token) {
                Ok(true) => {
                    log::info!("[Auth] Token validated successfully");
                }
                Ok(false) => {
                    // Token rejected by the server (e.g. the 10h JWT from a
                    // password login expired). Before signing the user out,
                    // try a SILENT re-login with the stored credentials —
                    // "authenticate once" must survive token expiry for DJs
                    // who set up at home and play venues for weeks.
                    let renewed = db.get_login_credentials().and_then(|(username, password)| {
                        match Self::do_login(&LoginCredentials { username, password }) {
                            Ok(response) => {
                                log::info!("[Auth] Silent re-login renewed the session for {}", response.username);
                                let roles_str = response.roles.join(",");
                                if let Err(e) = db.save_auth_credentials(
                                    &response.username,
                                    &response.access_token,
                                    response.refresh_token.as_deref(),
                                    &roles_str,
                                ) {
                                    log::error!("[Auth] Failed to save renewed credentials: {}", e);
                                }
                                *user.lock() = Some(AuthUser {
                                    username: response.username.clone(),
                                    roles: response.roles.clone(),
                                });
                                *token_store.lock() = Some(response.access_token);
                                *refresh_token.lock() = response.refresh_token;
                                Some(())
                            }
                            Err(e) => {
                                log::warn!("[Auth] Silent re-login failed: {}", e);
                                None
                            }
                        }
                    });

                    if renewed.is_none() {
                        // Definitively signed out — clear auth state
                        log::warn!("[Auth] Stored token is invalid or expired, clearing session");
                        *state.lock() = AuthState::Anon;
                        *user.lock() = None;
                        *token_store.lock() = None;
                        *refresh_token.lock() = None;
                        *entitlements.lock() = Entitlements::default();
                        *last_error.lock() = Some("Session expired. Please sign in again.".to_string());

                        // Clear from database
                        if let Err(e) = db.clear_auth_credentials() {
                            log::error!("[Auth] Failed to clear invalid credentials: {}", e);
                        }
                    }
                }
                Err(e) => {
                    // Network error - keep the session, user might be offline
                    log::warn!("[Auth] Could not validate token (network error): {}", e);
                    // Don't clear session on network errors - user might be offline
                }
            }
        });
    }

    /// Validate token with server (blocking)
    /// Returns Ok(true) if valid, Ok(false) if invalid, Err if network error
    fn do_validate_token(token: &str) -> Result<bool> {
        let url = format!("{}/youtube/status", config::oauth_base());

        let result = ureq::get(&url)
            .timeout(Duration::from_secs(10))
            .set("Authorization", &format!("Bearer {}", token))
            .call();

        match result {
            Ok(_response) => {
                // 200 OK means token is valid
                Ok(true)
            }
            Err(ureq::Error::Status(401, _)) => {
                // 401 Unauthorized means token is invalid
                Ok(false)
            }
            Err(ureq::Error::Status(code, _)) => {
                // Other status codes - treat as network issue
                Err(anyhow!("Server returned status {}", code))
            }
            Err(ureq::Error::Transport(t)) => {
                // Network error
                Err(anyhow!("Network error: {}", t))
            }
        }
    }

    /// Login with username/password (spawns background thread)
    pub fn login(&self, credentials: LoginCredentials, db: crate::db::Database) {
        if self.is_logging_in.load(Ordering::Relaxed) {
            log::warn!("[Auth] Login already in progress");
            return;
        }

        self.is_logging_in.store(true, Ordering::Release);
        *self.state.lock() = AuthState::LoggingIn;
        *self.last_error.lock() = None;

        let state = self.state.clone();
        let user = self.user.clone();
        let token = self.token.clone();
        let refresh_token = self.refresh_token.clone();
        let entitlements = self.entitlements.clone();
        let is_logging_in = self.is_logging_in.clone();
        let last_error = self.last_error.clone();

        std::thread::spawn(move || {
            let result = Self::do_login(&credentials);

            match result {
                Ok(response) => {
                    // Also save raw credentials for auto-login (optional, can be removed for security)
                    if let Err(e) = db.save_login_credentials(&credentials.username, &credentials.password) {
                        log::warn!("[Auth] Failed to save login credentials: {}", e);
                    }

                    Self::apply_login_success(
                        response,
                        &db,
                        &state,
                        &user,
                        &token,
                        &refresh_token,
                        &entitlements,
                    );
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    log::error!("[Auth] Login failed: {}", error_msg);
                    *last_error.lock() = Some(error_msg.clone());
                    *state.lock() = AuthState::Error(error_msg);
                }
            }

            is_logging_in.store(false, Ordering::Release);
        });
    }

    /// Persist + publish a successful login. Shared by the password flow and
    /// the browser sign-in handoff (Sign in with Google): store credentials,
    /// resolve entitlements (live blob → cached blob → plan_code fallback),
    /// flip state to Authed.
    #[allow(clippy::too_many_arguments)]
    fn apply_login_success(
        response: LoginResponse,
        db: &crate::db::Database,
        state: &Arc<Mutex<AuthState>>,
        user: &Arc<Mutex<Option<AuthUser>>>,
        token: &Arc<Mutex<Option<String>>>,
        refresh_token: &Arc<Mutex<Option<String>>>,
        entitlements: &Arc<Mutex<Entitlements>>,
    ) {
        let auth_user = AuthUser {
            username: response.username.clone(),
            roles: response.roles.clone(),
        };

        // Save to database
        let roles_str = response.roles.join(",");
        if let Err(e) = db.save_auth_credentials(
            &response.username,
            &response.access_token,
            response.refresh_token.as_deref(),
            &roles_str,
        ) {
            log::error!("[Auth] Failed to save credentials: {}", e);
        }

        // Fetch license blob for entitlements
        let user_entitlements = match license::fetch_license_blob(&response.access_token, db) {
            Ok(ent) => {
                log::info!("[Auth] License blob fetched: plan={}", ent.plan_code);
                ent
            }
            Err(e) => {
                log::warn!("[Auth] Could not fetch license blob: {}", e);
                // Try cached signed blob first. If unavailable, use enriched login
                // plan metadata before falling back to FREE defaults.
                license::load_cached_entitlements(db).unwrap_or_else(|| {
                    let fallback = Entitlements::from_plan_code(response.plan_code.as_deref());
                    log::info!(
                        "[Auth] Using login plan fallback: plan={}",
                        fallback.plan_code
                    );
                    fallback
                })
            }
        };

        // Update state
        *user.lock() = Some(auth_user);
        *token.lock() = Some(response.access_token);
        *refresh_token.lock() = response.refresh_token;
        let mut user_entitlements = user_entitlements;
        if user_entitlements.account_id == 0 {
            user_entitlements.account_id = response.account_id.unwrap_or(0).max(0) as u32;
        }
        *entitlements.lock() = user_entitlements;
        *state.lock() = AuthState::Authed;

        log::info!("[Auth] Login successful for {}", response.username);
    }

    // === Browser sign-in handoff (Sign in with Google) =====================
    //
    // REACT cannot run Google's web OAuth itself (no embedded browser, no
    // deep-link registration), so it mirrors the YouTube-connect pattern in
    // reverse: ask SaaS for a one-time code, open the system browser on the
    // authorize page (the web login offers Google AND password), and poll
    // until the signed-in browser session authorizes the code. The poll
    // returns the same JSON shape as /api/login with the user's persistent
    // API token as access_token, so the session survives offline forever.

    /// Start the browser sign-in flow. The one-time code is generated HERE
    /// (256 bits, hex) — the server only learns it when the signed-in
    /// browser session authorizes it, so there is no start endpoint and no
    /// pre-registered state.
    pub fn start_desktop_signin(&self) {
        if self.is_logging_in.load(Ordering::Relaxed) {
            log::warn!("[Auth] Login already in progress");
            return;
        }

        self.is_logging_in.store(true, Ordering::Release);
        *self.state.lock() = AuthState::LoggingIn;
        *self.last_error.lock() = None;

        let code: String = {
            use rand::RngCore;
            let mut bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut bytes);
            bytes.iter().map(|b| format!("{:02x}", b)).collect()
        };
        let authorize_url = format!(
            "{}/desktopAuth/authorize?code={}",
            config::oauth_base(),
            code
        );

        log::info!("[Auth] Desktop sign-in started, opening browser");
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&authorize_url).spawn();
        }
        #[cfg(not(target_os = "macos"))]
        {
            log::warn!("[Auth] Open this URL to sign in: {}", authorize_url);
        }

        let now = std::time::Instant::now();
        *self.desktop_signin.lock() = Some(DesktopSignIn {
            code,
            started: now,
            last_poll: now,
        });
        // State stays LoggingIn; the frame-loop poll finishes or fails the
        // flow (is_logging_in stays held until then).
    }

    /// Frame-loop gate: an in-flight browser sign-in wants a poll.
    pub fn should_poll_desktop_signin(&self) -> bool {
        if self.desktop_polling.load(Ordering::Relaxed) {
            return false;
        }
        self.desktop_signin
            .lock()
            .as_ref()
            .map_or(false, |s| s.last_poll.elapsed().as_millis() >= DESKTOP_POLL_INTERVAL_MS)
    }

    /// Poll SaaS for the handoff result (spawns a one-shot thread).
    pub fn poll_desktop_signin(&self, db: crate::db::Database) {
        let (code, expired) = {
            let mut guard = self.desktop_signin.lock();
            match guard.as_mut() {
                None => return,
                Some(s) => {
                    s.last_poll = std::time::Instant::now();
                    (
                        s.code.clone(),
                        s.started.elapsed().as_secs() >= DESKTOP_SIGNIN_TIMEOUT_SECS,
                    )
                }
            }
        };

        if expired {
            *self.desktop_signin.lock() = None;
            let msg = "Sign-in timed out. Please try again.".to_string();
            log::warn!("[Auth] Desktop sign-in timed out");
            *self.last_error.lock() = Some(msg.clone());
            *self.state.lock() = AuthState::Error(msg);
            self.is_logging_in.store(false, Ordering::Release);
            return;
        }

        if self.desktop_polling.swap(true, Ordering::AcqRel) {
            return; // a poll is already in flight
        }

        let state = self.state.clone();
        let user = self.user.clone();
        let token = self.token.clone();
        let refresh_token = self.refresh_token.clone();
        let entitlements = self.entitlements.clone();
        let last_error = self.last_error.clone();
        let is_logging_in = self.is_logging_in.clone();
        let desktop_signin = self.desktop_signin.clone();
        let desktop_polling = self.desktop_polling.clone();

        std::thread::spawn(move || {
            match Self::do_desktop_poll(&code) {
                Ok(Some(response)) => {
                    *desktop_signin.lock() = None;
                    Self::apply_login_success(
                        response,
                        &db,
                        &state,
                        &user,
                        &token,
                        &refresh_token,
                        &entitlements,
                    );
                    is_logging_in.store(false, Ordering::Release);
                }
                Ok(None) => {
                    // Still pending — the user is in the browser; keep waiting.
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.starts_with("Network error") {
                        // Venue wifi blip — keep the flow alive and retry.
                        log::warn!("[Auth] Desktop sign-in poll network error: {}", msg);
                    } else {
                        // Unknown/expired code or hard server error → fail.
                        log::error!("[Auth] Desktop sign-in failed: {}", msg);
                        *desktop_signin.lock() = None;
                        *last_error.lock() = Some(msg.clone());
                        *state.lock() = AuthState::Error(msg);
                        is_logging_in.store(false, Ordering::Release);
                    }
                }
            }
            desktop_polling.store(false, Ordering::Release);
        });
    }

    /// One poll round-trip (blocking). Ok(Some) = authorized with the session
    /// payload; Ok(None) = still pending.
    fn do_desktop_poll(code: &str) -> Result<Option<LoginResponse>> {
        let url = format!("{}/v1/auth/desktop/poll", config::api_base());

        let body = serde_json::json!({ "code": code });
        let response = ureq::post(&url)
            .timeout(Duration::from_secs(10))
            .set("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| match e {
                ureq::Error::Status(404, _) => anyhow!("Sign-in code expired. Please try again."),
                ureq::Error::Status(code, _) => anyhow!("Server error ({})", code),
                ureq::Error::Transport(t) => anyhow!("Network error: {}", t),
            })?;

        let value: serde_json::Value = response
            .into_json()
            .map_err(|e| anyhow!("Invalid response: {}", e))?;
        if value.get("status").and_then(|s| s.as_str()) == Some("authorized") {
            let login: LoginResponse =
                serde_json::from_value(value).map_err(|e| anyhow!("Invalid response: {}", e))?;
            Ok(Some(login))
        } else {
            Ok(None)
        }
    }

    /// Perform the actual login HTTP request (blocking)
    fn do_login(credentials: &LoginCredentials) -> Result<LoginResponse> {
        let url = format!("{}/login", config::api_base());

        let body = serde_json::json!({
            "username": credentials.username,
            "password": credentials.password,
        });

        let response = ureq::post(&url)
            .timeout(Duration::from_secs(LOGIN_TIMEOUT_SECS))
            .set("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| {
                match e {
                    ureq::Error::Status(401, _) => anyhow!("Invalid username or password"),
                    ureq::Error::Status(403, _) => anyhow!("Account disabled"),
                    ureq::Error::Status(code, _) => anyhow!("Server error ({})", code),
                    ureq::Error::Transport(t) => anyhow!("Network error: {}", t),
                }
            })?;

        let login_response: LoginResponse = response
            .into_json()
            .map_err(|e| anyhow!("Invalid response: {}", e))?;

        Ok(login_response)
    }

    /// Logout and clear all auth state
    pub fn logout(&self, db: &crate::db::Database) {
        *self.state.lock() = AuthState::Anon;
        *self.user.lock() = None;
        *self.token.lock() = None;
        *self.refresh_token.lock() = None;
        *self.entitlements.lock() = Entitlements::default();
        *self.last_error.lock() = None;

        // Clear from database
        if let Err(e) = db.clear_auth_credentials() {
            log::error!("[Auth] Failed to clear credentials: {}", e);
        }
        // Clear cached license blob
        if let Err(e) = db.clear_license_blob() {
            log::error!("[Auth] Failed to clear license blob: {}", e);
        }

        log::info!("[Auth] Logged out");
    }

    /// Refresh the license blob in the background (call every 24h)
    pub fn refresh_license_async(&self, db: crate::db::Database) {
        let token = match self.token.lock().clone() {
            Some(t) => t,
            None => return,
        };

        let entitlements = self.entitlements.clone();

        std::thread::spawn(move || {
            match license::fetch_license_blob(&token, &db) {
                Ok(ent) => {
                    log::info!("[Auth] License blob refreshed: plan={}", ent.plan_code);
                    *entitlements.lock() = ent;
                }
                Err(e) => {
                    // Keep existing entitlements on network error
                    log::warn!("[Auth] License refresh failed (keeping cached): {}", e);
                }
            }
        });
    }

    /// Get connected account by provider
    pub fn get_connected_account(&self, provider: ProviderId) -> Option<ConnectedAccount> {
        self.connected_accounts
            .lock()
            .iter()
            .find(|a| a.provider == provider)
            .cloned()
    }

    /// Add or update a connected account
    pub fn set_connected_account(&self, account: ConnectedAccount, db: &crate::db::Database) {
        let mut accounts = self.connected_accounts.lock();

        // Remove existing account for this provider
        accounts.retain(|a| a.provider != account.provider);

        // Save to database
        if let Err(e) = db.save_connected_account(&account) {
            log::error!("[Auth] Failed to save connected account: {}", e);
        }

        accounts.push(account);
    }

    /// Disconnect a connected account
    pub fn disconnect_account(&self, provider: ProviderId, db: &crate::db::Database) {
        let mut accounts = self.connected_accounts.lock();
        accounts.retain(|a| a.provider != provider);

        if let Err(e) = db.delete_connected_account(&provider.as_str()) {
            log::error!("[Auth] Failed to delete connected account: {}", e);
        }

        log::info!("[Auth] Disconnected {} account", provider.as_str());
    }
}


#[cfg(test)]
mod entitlement_tests {
    use super::Entitlements;

    #[test]
    fn enterprise_plan_fallback_enables_lighting() {
        let ent = Entitlements::from_plan_code(Some("enterprise"));
        assert_eq!(ent.plan_code, "ENTERPRISE");
        assert_eq!(ent.max_streams, 10);
        assert!(ent.lighting_enabled);
        assert!(!ent.watermark);
    }

    #[test]
    fn unknown_plan_fallback_has_safe_limits_but_no_label() {
        let ent = Entitlements::from_plan_code(Some("bogus"));
        assert!(ent.plan_code.is_empty());
        assert!(ent.watermark);
        assert!(!ent.lighting_enabled);
    }

    #[test]
    fn missing_plan_fallback_has_safe_limits_but_no_label() {
        let ent = Entitlements::from_plan_code(None);
        assert!(ent.plan_code.is_empty());
        assert!(ent.watermark);
        assert!(!ent.lighting_enabled);
    }
}
