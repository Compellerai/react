//! Eligibility Cache - Checks if user can stream to YouTube
//!
//! Eligibility is cached for 24 hours to avoid API spam.
//! Ineligibility reasons include:
//! - Channel not verified
//! - Live streaming not enabled
//! - Account restrictions
//! - Regional restrictions

use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;

use super::timing;

use crate::config;

/// Eligibility state machine
#[derive(Debug, Clone, PartialEq)]
pub enum EligibilityState {
    /// Haven't checked yet
    Unknown,
    /// Check in progress
    Checking,
    /// User is eligible to stream
    Eligible,
    /// User is not eligible (with reason)
    Ineligible(String),
}

impl Default for EligibilityState {
    fn default() -> Self {
        EligibilityState::Unknown
    }
}

/// Response from eligibility check endpoint
#[derive(Debug, Deserialize)]
struct EligibilityResponse {
    eligible: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    details: Option<String>,
}

/// Eligibility cache with rate limiting
#[derive(Clone)]
pub struct EligibilityCache {
    /// Current eligibility state
    state: Arc<Mutex<EligibilityState>>,
    /// When eligibility was last checked
    checked_at: Arc<Mutex<Option<Instant>>>,
    /// Earliest time we can check again (rate limiter)
    next_check_allowed: Arc<Mutex<Option<Instant>>>,
    /// Is a check in progress?
    checking: Arc<AtomicBool>,
    /// Compeller session token
    compeller_token: Arc<Mutex<Option<String>>>,
}

impl Default for EligibilityCache {
    fn default() -> Self {
        Self::new()
    }
}

impl EligibilityCache {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(EligibilityState::Unknown)),
            checked_at: Arc::new(Mutex::new(None)),
            next_check_allowed: Arc::new(Mutex::new(None)),
            checking: Arc::new(AtomicBool::new(false)),
            compeller_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the Compeller session token
    pub fn set_compeller_token(&self, token: Option<String>) {
        *self.compeller_token.lock() = token;
    }

    /// Get current eligibility state
    pub fn get_state(&self) -> EligibilityState {
        self.state.lock().clone()
    }

    /// Check if user is eligible
    pub fn is_eligible(&self) -> bool {
        matches!(*self.state.lock(), EligibilityState::Eligible)
    }

    /// Check if eligibility is known (not Unknown or Checking)
    pub fn is_known(&self) -> bool {
        let state = self.state.lock();
        matches!(*state, EligibilityState::Eligible | EligibilityState::Ineligible(_))
    }

    /// Check if eligibility check is in progress
    pub fn is_checking(&self) -> bool {
        self.checking.load(Ordering::Relaxed)
    }

    /// Check if cache is stale (older than ELIGIBILITY_CACHE_MS)
    pub fn is_stale(&self) -> bool {
        match *self.checked_at.lock() {
            Some(t) => t.elapsed() > Duration::from_millis(timing::ELIGIBILITY_CACHE_MS),
            None => true,
        }
    }

    /// Check if we can perform an eligibility check now (rate limited)
    pub fn can_check_now(&self) -> bool {
        if self.checking.load(Ordering::Relaxed) {
            return false;
        }
        match *self.next_check_allowed.lock() {
            Some(t) => Instant::now() >= t,
            None => true,
        }
    }

    /// Should we check eligibility?
    /// Returns true if Unknown or stale, and rate limit allows
    pub fn should_check(&self) -> bool {
        let state = self.state.lock().clone();
        let should = matches!(state, EligibilityState::Unknown) || self.is_stale();
        should && self.can_check_now()
    }

    /// Check eligibility (async, non-blocking)
    pub fn check(&self) {
        if self.checking.load(Ordering::Relaxed) {
            return;
        }

        let token = match self.compeller_token.lock().clone() {
            Some(t) => t,
            None => {
                *self.state.lock() = EligibilityState::Ineligible("Not logged in".to_string());
                return;
            }
        };

        self.checking.store(true, Ordering::Release);
        *self.state.lock() = EligibilityState::Checking;

        let state = self.state.clone();
        let checked_at = self.checked_at.clone();
        let next_check_allowed = self.next_check_allowed.clone();
        let checking = self.checking.clone();

        std::thread::spawn(move || {
            let result = Self::do_check(&token);

            match result {
                Ok(resp) => {
                    if resp.eligible {
                        *state.lock() = EligibilityState::Eligible;
                        log::info!("[YouTube] User is eligible to stream");
                    } else {
                        let reason = resp.reason.unwrap_or_else(|| "Unknown reason".to_string());
                        *state.lock() = EligibilityState::Ineligible(reason.clone());
                        log::warn!("[YouTube] User is not eligible: {}", reason);
                    }
                }
                Err(e) => {
                    log::error!("[YouTube] Eligibility check failed: {}", e);
                    // On network error, don't cache - allow retry
                    *state.lock() = EligibilityState::Unknown;
                }
            }

            *checked_at.lock() = Some(Instant::now());
            // Rate limit: don't check again for at least 1 minute on failure, or full cache time on success
            let cooldown = if matches!(*state.lock(), EligibilityState::Eligible | EligibilityState::Ineligible(_)) {
                timing::ELIGIBILITY_CACHE_MS
            } else {
                60_000 // 1 minute on network failure
            };
            *next_check_allowed.lock() = Some(Instant::now() + Duration::from_millis(cooldown));

            checking.store(false, Ordering::Release);
        });
    }

    /// Perform the actual eligibility check (blocking)
    fn do_check(token: &str) -> anyhow::Result<EligibilityResponse> {
        let url = format!("{}/youtube/eligibility", config::oauth_base());

        let response = ureq::get(&url)
            .timeout(Duration::from_secs(10))
            .set("Authorization", &format!("Bearer {}", token))
            .call()
            .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

        let resp: EligibilityResponse = response
            .into_json()
            .map_err(|e| anyhow::anyhow!("Invalid response: {}", e))?;

        Ok(resp)
    }

    /// Reset eligibility state (e.g., when YouTube disconnects)
    pub fn reset(&self) {
        *self.state.lock() = EligibilityState::Unknown;
        *self.checked_at.lock() = None;
        *self.next_check_allowed.lock() = None;
    }

    /// Get ineligibility reason (if ineligible)
    pub fn get_ineligible_reason(&self) -> Option<String> {
        match &*self.state.lock() {
            EligibilityState::Ineligible(reason) => Some(reason.clone()),
            _ => None,
        }
    }
}
