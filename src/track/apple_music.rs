//! Apple Music track source
//!
//! Uses osascript to poll the macOS Music app for current track information.
//! Polling runs in a background thread at a low frequency to avoid performance impact.

use super::{TrackInfo, TrackSource};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Apple Music track source using osascript (background thread polling)
pub struct AppleMusicSource {
    /// Shared state with background thread (using parking_lot for performance)
    state: Arc<Mutex<AppleMusicState>>,
    /// Background polling thread handle
    poll_thread: Option<JoinHandle<()>>,
    /// Flag to signal thread to stop (atomic for lock-free check)
    running: Arc<AtomicBool>,
}

/// Internal state shared between main and polling thread
struct AppleMusicState {
    /// Current track info (if any)
    current_track: Option<TrackInfo>,
    /// Whether the source is available
    available: bool,
}

impl Default for AppleMusicSource {
    fn default() -> Self {
        Self::new()
    }
}

impl AppleMusicSource {
    /// Create a new Apple Music source with background polling
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(AppleMusicState {
            current_track: None,
            available: true,
        }));
        let running = Arc::new(AtomicBool::new(true));

        // Start background polling thread
        let state_clone = state.clone();
        let running_clone = running.clone();
        let poll_thread = thread::Builder::new()
            .name("apple-music-poll".into())
            .spawn(move || {
                Self::poll_loop(state_clone, running_clone);
            })
            .expect("Failed to spawn Apple Music poll thread");

        Self {
            state,
            poll_thread: Some(poll_thread),
            running,
        }
    }

    /// Background polling loop - runs at low frequency to avoid performance impact
    fn poll_loop(state: Arc<Mutex<AppleMusicState>>, running: Arc<AtomicBool>) {
        log::info!("[AppleMusic] Background polling thread started");

        // Initial delay to let app start up
        thread::sleep(Duration::from_secs(2));

        loop {
            // Check if we should stop (lock-free)
            if !running.load(Ordering::Relaxed) {
                break;
            }

            // Poll for track info (this is slow, runs on background thread)
            let track = Self::fetch_track_info();

            // Update shared state (brief lock)
            {
                let mut s = state.lock();
                s.available = track.is_some() || Self::is_music_running_quick();
                s.current_track = track;
            }

            // Sleep between polls (2 seconds - we don't need real-time updates)
            // Check for shutdown every 100ms during sleep
            for _ in 0..20 {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }

        log::info!("[AppleMusic] Background polling thread stopped");
    }

    /// Run an osascript command and get the output
    fn run_osascript(script: &str) -> Option<String> {
        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .ok()?;

        if output.status.success() {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if result.is_empty() || result == "missing value" {
                None
            } else {
                Some(result)
            }
        } else {
            None
        }
    }

    /// Quick check if Music app is running (for availability status)
    fn is_music_running_quick() -> bool {
        // Use pgrep instead of osascript for faster check
        Command::new("pgrep")
            .arg("-x")
            .arg("Music")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Check if Music app is running (via AppleScript, more reliable but slower)
    fn is_music_running() -> bool {
        let script = r#"tell application "System Events" to (name of processes) contains "Music""#;
        Self::run_osascript(script)
            .map(|s| s.to_lowercase() == "true")
            .unwrap_or(false)
    }

    /// Fetch current track info from Music app (called from background thread only)
    fn fetch_track_info() -> Option<TrackInfo> {
        // Quick check if Music app is even running
        if !Self::is_music_running_quick() {
            return None;
        }

        // Get all track info in a single AppleScript call for efficiency
        let script = r#"
tell application "Music"
    try
        if player state is playing or player state is paused then
            set currentTrack to current track
            set trackName to name of currentTrack
            set trackArtist to artist of currentTrack
            set trackAlbum to album of currentTrack
            set trackDuration to duration of currentTrack
            set playerPos to player position
            set isPlaying to (player state is playing)
            return trackName & "|||" & trackArtist & "|||" & trackAlbum & "|||" & trackDuration & "|||" & playerPos & "|||" & isPlaying
        else
            return ""
        end if
    on error
        return ""
    end try
end tell
"#;

        let output = Self::run_osascript(script)?;
        if output.is_empty() {
            return None;
        }

        let parts: Vec<&str> = output.split("|||").collect();
        if parts.len() >= 6 {
            // Try to fetch artwork (separate call to avoid slowing down main info)
            let artwork = Self::fetch_artwork();

            Some(TrackInfo {
                title: parts[0].to_string(),
                artist: parts[1].to_string(),
                album: if parts[2].is_empty() { None } else { Some(parts[2].to_string()) },
                duration: parts[3].parse().ok(),
                position: parts[4].parse().ok(),
                is_playing: parts[5].to_lowercase() == "true",
                bpm: None, // Apple Music doesn't expose BPM
                source: "Apple Music".to_string(),
                deck: None,
                artwork,
            })
        } else {
            None
        }
    }

    /// Fetch album artwork from current track
    fn fetch_artwork() -> Option<Vec<u8>> {
        // Save artwork to temp file and read it back
        let temp_path = "/tmp/compeller_album_art.jpg";

        // Use raw data and write as binary
        let script = format!(r#"
tell application "Music"
    try
        set currentTrack to current track
        if (count of artworks of currentTrack) > 0 then
            set artworkData to raw data of artwork 1 of currentTrack
            set outFile to open for access POSIX file "{}" with write permission
            set eof outFile to 0
            write artworkData to outFile
            close access outFile
            return "ok"
        else
            return "no artwork"
        end if
    on error errMsg
        try
            close access POSIX file "{}"
        end try
        return "error"
    end try
end tell
"#, temp_path, temp_path);

        let result = Self::run_osascript(&script)?;
        if result == "ok" {
            std::fs::read(temp_path).ok()
        } else {
            None
        }
    }
}

impl Drop for AppleMusicSource {
    fn drop(&mut self) {
        // Signal thread to stop
        self.running.store(false, Ordering::Relaxed);
        // Wait for thread to finish
        if let Some(handle) = self.poll_thread.take() {
            let _ = handle.join();
        }
    }
}

impl TrackSource for AppleMusicSource {
    fn name(&self) -> &str {
        "Apple Music"
    }

    fn get_current_track(&mut self) -> Option<TrackInfo> {
        // Just return cached value from background thread (no blocking)
        self.state.lock().current_track.clone()
    }

    fn is_available(&self) -> bool {
        self.state.lock().available
    }
}
