//! Configuration for API endpoints
//!
//! Prod (default; `build.sh`): https://compeller.ai
//! Dev (`--features dev`; `start-react.sh`): the host named by the
//! `REACT_DEV_API_HOST` environment variable at compile time, e.g.
//! `https://dev.example.com` (no trailing slash). Keep it in the untracked
//! `.env.local` — see `.env.example`.

/// API base URL for compeller.ai backend
/// Set via compile-time feature flag: --features dev
#[cfg(feature = "dev")]
pub const API_HOST: &str = env!(
    "REACT_DEV_API_HOST",
    "the `dev` feature needs REACT_DEV_API_HOST set at build time (see .env.example)"
);

#[cfg(not(feature = "dev"))]
pub const API_HOST: &str = "https://compeller.ai";

/// Full API endpoint base
pub const fn api_base() -> &'static str {
    #[cfg(feature = "dev")]
    { concat!(env!("REACT_DEV_API_HOST"), "/api") }

    #[cfg(not(feature = "dev"))]
    { "https://compeller.ai/api" }
}

/// OAuth endpoint base (no /api prefix)
pub const fn oauth_base() -> &'static str {
    API_HOST
}
