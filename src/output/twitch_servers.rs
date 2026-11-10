//! Twitch ingest server discovery and selection
//!
//! Pings Twitch ingest servers to find the one with lowest latency.
//! Tracks problematic servers during reconnection to avoid repeatedly
//! failing on the same server. Persists data to SQLite for fast startup.

use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use parking_lot::Mutex;

/// Twitch ingest servers (major regions)
const TWITCH_SERVERS: &[(&str, &str)] = &[
    ("San Francisco", "sfo.contribute.live-video.net"),
    ("Los Angeles", "lax.contribute.live-video.net"),
    ("Seattle", "sea.contribute.live-video.net"),
    ("Phoenix", "phx.contribute.live-video.net"),
    ("Denver", "den.contribute.live-video.net"),
    ("Dallas", "dfw.contribute.live-video.net"),
    ("Chicago", "ord.contribute.live-video.net"),
    ("New York", "jfk.contribute.live-video.net"),
    ("Miami", "mia.contribute.live-video.net"),
    ("Ashburn", "iad.contribute.live-video.net"),
    ("Atlanta", "atl.contribute.live-video.net"),
];

/// How long a server is blacklisted after failure (10 minutes)
const DEGRADED_TIMEOUT_SECS: u64 = 600;

/// How long cached ping data is considered fresh (5 minutes)
const PING_CACHE_FRESH_SECS: u64 = 300;

/// How long before we must re-ping (1 hour)
const PING_CACHE_MAX_AGE_SECS: u64 = 3600;

/// Twitch ingest server with ping result
#[derive(Clone, Debug)]
pub struct TwitchServer {
    pub name: String,
    pub host: String,
    pub latency_ms: Option<u32>,
    pub selected: bool,
    /// True if server is currently degraded (failed recently)
    pub degraded: bool,
}

/// Global state for Twitch servers
static TWITCH_SERVER_STATE: std::sync::OnceLock<Mutex<TwitchServerState>> = std::sync::OnceLock::new();

struct TwitchServerState {
    /// Cached server list with ping results
    servers: Vec<TwitchServer>,
    /// Currently selected host
    selected_host: String,
    /// Last time we pinged servers (for cache freshness)
    last_ping_time: Option<Instant>,
    /// Servers that have failed, with Instant when they were marked degraded
    degraded_hosts: HashMap<String, Instant>,
}

fn get_server_state() -> &'static Mutex<TwitchServerState> {
    TWITCH_SERVER_STATE.get_or_init(|| {
        Mutex::new(TwitchServerState {
            servers: Vec::new(),
            selected_host: String::new(),
            last_ping_time: None,
            degraded_hosts: HashMap::new(),
        })
    })
}

/// Get current unix timestamp
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Get all Twitch ingest servers with their ping results
pub fn get_twitch_servers() -> Vec<TwitchServer> {
    let mut state = get_server_state().lock();
    // Update degraded status based on timeout
    let now = Instant::now();
    let degraded_hosts = state.degraded_hosts.clone();
    for server in &mut state.servers {
        server.degraded = degraded_hosts.get(&server.host)
            .map(|&marked_at| now.duration_since(marked_at).as_secs() < DEGRADED_TIMEOUT_SECS)
            .unwrap_or(false);
    }
    state.servers.clone()
}

/// Get the currently selected Twitch server hostname
pub fn get_selected_twitch_server() -> String {
    get_server_state().lock().selected_host.clone()
}

/// Check if a server is currently degraded
pub fn is_server_degraded(host: &str) -> bool {
    let state = get_server_state().lock();
    if let Some(&marked_at) = state.degraded_hosts.get(host) {
        Instant::now().duration_since(marked_at).as_secs() < DEGRADED_TIMEOUT_SECS
    } else {
        false
    }
}

/// Mark a server as degraded (failed during streaming)
/// It will be excluded from selections for DEGRADED_TIMEOUT_SECS (10 min)
pub fn mark_server_degraded(host: &str) {
    let mut state = get_server_state().lock();
    log::warn!("[Twitch] Marking server as degraded for {}s: {}", DEGRADED_TIMEOUT_SECS, host);
    state.degraded_hosts.insert(host.to_string(), Instant::now());
}

/// Clear degraded status for a server (e.g., after successful streaming for 30+ seconds)
pub fn clear_server_degraded(host: &str) {
    let mut state = get_server_state().lock();
    if state.degraded_hosts.remove(host).is_some() {
        log::info!("[Twitch] Cleared degraded status for server: {}", host);
    }
}

/// Clear all degraded server marks
pub fn clear_all_degraded() {
    let mut state = get_server_state().lock();
    if !state.degraded_hosts.is_empty() {
        log::info!("[Twitch] Clearing {} degraded server marks", state.degraded_hosts.len());
        state.degraded_hosts.clear();
    }
}

/// Get count of currently degraded servers
pub fn get_degraded_server_count() -> usize {
    let state = get_server_state().lock();
    let now = Instant::now();
    state.degraded_hosts.values()
        .filter(|&&marked_at| now.duration_since(marked_at).as_secs() < DEGRADED_TIMEOUT_SECS)
        .count()
}

/// Ping a single server and return latency in ms (or None if unreachable)
fn ping_server(host: &str) -> Option<u32> {
    let addr = format!("{}:1935", host);
    let start = Instant::now();

    if let Ok(mut addrs) = addr.to_socket_addrs() {
        if let Some(addr) = addrs.next() {
            if TcpStream::connect_timeout(&addr, Duration::from_millis(1500)).is_ok() {
                return Some(start.elapsed().as_millis() as u32);
            }
        }
    }
    None
}

/// Ping all Twitch servers and update cache
/// Returns list sorted by latency (fastest first)
pub fn ping_all_servers() -> Vec<TwitchServer> {
    log::info!("[Twitch] Pinging all ingest servers...");

    let mut servers: Vec<TwitchServer> = Vec::new();
    let state_guard = get_server_state().lock();
    let degraded_hosts = state_guard.degraded_hosts.clone();
    drop(state_guard);

    let now = Instant::now();

    for (name, host) in TWITCH_SERVERS {
        let latency_ms = ping_server(host);

        if let Some(ms) = latency_ms {
            log::info!("[Twitch]   {} - {}ms", name, ms);
        } else {
            log::info!("[Twitch]   {} - unreachable", name);
        }

        let degraded = degraded_hosts.get(*host)
            .map(|&marked_at| now.duration_since(marked_at).as_secs() < DEGRADED_TIMEOUT_SECS)
            .unwrap_or(false);

        servers.push(TwitchServer {
            name: name.to_string(),
            host: host.to_string(),
            latency_ms,
            selected: false,
            degraded,
        });
    }

    // Sort by latency (unreachable at end)
    servers.sort_by(|a, b| {
        match (a.latency_ms, b.latency_ms) {
            (Some(la), Some(lb)) => la.cmp(&lb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    // Update global state
    {
        let mut state = get_server_state().lock();
        state.servers = servers.clone();
        state.last_ping_time = Some(Instant::now());
    }

    servers
}

/// Find the best available server (fastest non-degraded)
/// If all servers are degraded, returns the one with oldest degradation
/// Re-pings if cache is stale
pub fn find_best_available_server() -> String {
    // Check if we need to re-ping
    let needs_ping = {
        let state = get_server_state().lock();
        state.last_ping_time
            .map(|t| t.elapsed().as_secs() > PING_CACHE_FRESH_SECS)
            .unwrap_or(true)
    };

    if needs_ping {
        ping_all_servers();
    }

    let state = get_server_state().lock();
    let now = Instant::now();

    // Find best non-degraded server
    let mut best_host: Option<&str> = None;
    let mut best_latency = u32::MAX;

    for server in &state.servers {
        if let Some(latency) = server.latency_ms {
            // Check if degraded
            let is_degraded = state.degraded_hosts.get(&server.host)
                .map(|&marked_at| now.duration_since(marked_at).as_secs() < DEGRADED_TIMEOUT_SECS)
                .unwrap_or(false);

            if !is_degraded && latency < best_latency {
                best_latency = latency;
                best_host = Some(&server.host);
            }
        }
    }

    // If found a non-degraded server, use it
    if let Some(host) = best_host {
        log::info!("[Twitch] Best available server: {} ({}ms)", host, best_latency);
        return host.to_string();
    }

    // All servers are degraded - find the one with oldest degradation (most likely recovered)
    log::warn!("[Twitch] All servers degraded, using oldest degraded server");

    let mut oldest_host: Option<&str> = None;
    let mut oldest_time = Instant::now();

    for server in &state.servers {
        if server.latency_ms.is_some() {
            if let Some(&marked_at) = state.degraded_hosts.get(&server.host) {
                if marked_at < oldest_time {
                    oldest_time = marked_at;
                    oldest_host = Some(&server.host);
                }
            }
        }
    }

    if let Some(host) = oldest_host {
        log::info!("[Twitch] Using oldest degraded server: {}", host);
        return host.to_string();
    }

    // Fallback to generic endpoint
    log::warn!("[Twitch] No servers available, using fallback: live.twitch.tv");
    "live.twitch.tv".to_string()
}

/// Find best server, re-pinging first (for reconnection scenarios)
pub fn find_best_server_fresh() -> String {
    ping_all_servers();
    find_best_available_server()
}

/// Legacy function - calls find_best_available_server
pub fn find_best_twitch_server() -> String {
    find_best_available_server()
}

// ============================================================================
// Database Persistence
// ============================================================================

/// Initialize the twitch_servers table in the database
pub fn init_db_table(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS twitch_ingest_servers (
            host TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            latency_ms INTEGER,
            last_ping_time INTEGER NOT NULL,
            degraded_until INTEGER
        )",
        [],
    )?;
    Ok(())
}

/// Save server ping results to database
pub fn save_servers_to_db(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let state = get_server_state().lock();
    let now = unix_now();

    for server in &state.servers {
        let degraded_until: Option<u64> = state.degraded_hosts.get(&server.host)
            .map(|&marked_at| {
                let elapsed = Instant::now().duration_since(marked_at).as_secs();
                if elapsed < DEGRADED_TIMEOUT_SECS {
                    now + (DEGRADED_TIMEOUT_SECS - elapsed)
                } else {
                    0 // Already expired
                }
            })
            .filter(|&t| t > 0);

        conn.execute(
            "INSERT OR REPLACE INTO twitch_ingest_servers
             (host, name, latency_ms, last_ping_time, degraded_until)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                server.host,
                server.name,
                server.latency_ms,
                now as i64,
                degraded_until.map(|t| t as i64),
            ],
        )?;
    }

    log::info!("[Twitch] Saved {} servers to database", state.servers.len());
    Ok(())
}

/// Load server data from database (for fast startup)
/// Returns true if we loaded fresh enough data (< PING_CACHE_MAX_AGE_SECS old)
pub fn load_servers_from_db(conn: &rusqlite::Connection) -> rusqlite::Result<bool> {
    let now = unix_now();

    let mut stmt = conn.prepare(
        "SELECT host, name, latency_ms, last_ping_time, degraded_until
         FROM twitch_ingest_servers"
    )?;

    let mut servers: Vec<TwitchServer> = Vec::new();
    let mut degraded_hosts: HashMap<String, Instant> = HashMap::new();
    let mut oldest_ping: u64 = now;

    let rows = stmt.query_map([], |row| {
        let host: String = row.get(0)?;
        let name: String = row.get(1)?;
        let latency_ms: Option<u32> = row.get(2)?;
        let last_ping_time: i64 = row.get(3)?;
        let degraded_until: Option<i64> = row.get(4)?;
        Ok((host, name, latency_ms, last_ping_time as u64, degraded_until.map(|t| t as u64)))
    })?;

    for row in rows {
        let (host, name, latency_ms, last_ping_time, degraded_until) = row?;

        if last_ping_time < oldest_ping {
            oldest_ping = last_ping_time;
        }

        // Check if still degraded
        let degraded = degraded_until
            .map(|until| until > now)
            .unwrap_or(false);

        if degraded {
            if let Some(until) = degraded_until {
                // Convert future timestamp back to Instant
                let remaining_secs = until.saturating_sub(now);
                let marked_at = Instant::now() - Duration::from_secs(DEGRADED_TIMEOUT_SECS - remaining_secs);
                degraded_hosts.insert(host.clone(), marked_at);
            }
        }

        servers.push(TwitchServer {
            name,
            host,
            latency_ms,
            selected: false,
            degraded,
        });
    }

    if servers.is_empty() {
        log::info!("[Twitch] No cached server data in database");
        return Ok(false);
    }

    // Sort by latency
    servers.sort_by(|a, b| {
        match (a.latency_ms, b.latency_ms) {
            (Some(la), Some(lb)) => la.cmp(&lb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    // Mark best as selected
    if let Some(first) = servers.first_mut() {
        first.selected = true;
    }

    let cache_age = now - oldest_ping;
    let is_fresh = cache_age < PING_CACHE_MAX_AGE_SECS;

    // Update state
    {
        let mut state = get_server_state().lock();
        state.servers = servers;
        state.degraded_hosts = degraded_hosts;
        if is_fresh {
            // Pretend we pinged (cache_age) seconds ago
            state.last_ping_time = Some(Instant::now() - Duration::from_secs(cache_age));
        }
        if let Some(best) = state.servers.first() {
            state.selected_host = best.host.clone();
        }
    }

    log::info!("[Twitch] Loaded {} servers from database ({}s old, fresh={})",
        get_server_state().lock().servers.len(), cache_age, is_fresh);

    Ok(is_fresh)
}
