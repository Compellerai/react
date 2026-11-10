//! Track information and source abstraction
//!
//! Provides a source-agnostic TrackInfo struct and implementations for various
//! music sources (Apple Music, Beatlink/Pioneer, Denon, Spotify, etc.)

mod abletonlink;
mod apple_music;
mod prodjlink;

pub use abletonlink::AbletonLinkListener;
pub use apple_music::AppleMusicSource;
pub use prodjlink::ProDjLinkSource;

use std::time::Instant;
use std::net::IpAddr;

use prodjlink_rs::MasterBeat;

/// CDJ device information for UI display
#[derive(Debug, Clone)]
pub struct CdjDeviceInfo {
    /// Device ID (1-6)
    pub device_id: u8,
    /// Device name (e.g., "CDJ-3000")
    pub name: String,
    /// IP address
    pub ip: IpAddr,
    /// Whether track is loaded
    pub track_loaded: bool,
    /// Whether currently playing
    pub is_playing: bool,
    /// Whether looping
    pub is_looping: bool,
    /// Whether this is the master deck
    pub is_master: bool,
    /// Whether on-air (mixer channel fader up)
    pub is_on_air: bool,
    /// Current BPM
    pub bpm: Option<f32>,
    /// Track title (if metadata fetched)
    pub track_title: Option<String>,
    /// Track artist (if metadata fetched)
    pub track_artist: Option<String>,
}

/// Track information from any music source
#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    /// Track title
    pub title: String,
    /// Artist name
    pub artist: String,
    /// Album name (optional)
    pub album: Option<String>,
    /// Track duration in seconds (optional)
    pub duration: Option<f64>,
    /// Current playback position in seconds (optional)
    pub position: Option<f64>,
    /// Whether the track is currently playing
    pub is_playing: bool,
    /// BPM if available (useful for DJ software)
    pub bpm: Option<f32>,
    /// Source name (e.g., "Apple Music", "Pioneer CDJ", "Denon SC6000")
    pub source: String,
    /// Deck number for DJ setups (1, 2, 3, 4, etc.)
    pub deck: Option<u8>,
    /// Album artwork as PNG/JPEG bytes (optional)
    pub artwork: Option<Vec<u8>>,
}

impl TrackInfo {
    /// Create a new empty track info
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if this track info represents an actual track
    pub fn has_track(&self) -> bool {
        !self.title.is_empty()
    }

    /// Format as display string (Artist - Title)
    pub fn display_string(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

/// Trait for track information sources
pub trait TrackSource: Send {
    /// Get the source name
    fn name(&self) -> &str;

    /// Poll for current track info
    /// Returns None if no track is playing or source is unavailable
    fn get_current_track(&mut self) -> Option<TrackInfo>;

    /// Check if the source is available/connected
    fn is_available(&self) -> bool;

    /// Get a status message for UI display (e.g., "Scanning for devices...")
    fn status_message(&self) -> Option<String> {
        None
    }

    /// Get CDJ device info (for Pro DJ Link sources only)
    fn get_cdj_devices(&self) -> Vec<CdjDeviceInfo> {
        Vec::new()
    }

    /// Get the master deck's current beat (Pro DJ Link sources only).
    /// `None` when there is no master CDJ. Mirrors `get_cdj_devices`'s
    /// default-empty pattern — only `ProDjLinkSource` overrides it.
    fn master_beat(&self) -> Option<MasterBeat> {
        None
    }
}

/// Track announcement state for UI display
pub struct TrackAnnouncement {
    /// Current track being displayed
    current_track: Option<TrackInfo>,
    /// Previous track (to detect changes)
    last_track_id: String,
    /// When the current announcement started
    show_time: Option<Instant>,
    /// How long to show the announcement (seconds)
    display_duration: f32,
    /// Current opacity (for fade in/out)
    opacity: f32,
    /// Target opacity
    target_opacity: f32,
    /// Whether we're in the "new track" announcement phase
    is_announcing: bool,
    /// Animation progress (0.0 = center/large, 1.0 = corner/small)
    animation_progress: f32,
    /// Time spent in center before moving to corner (seconds)
    center_hold_time: f32,
    /// Duration of the shrink animation (seconds)
    shrink_duration: f32,
}

impl Default for TrackAnnouncement {
    fn default() -> Self {
        Self {
            current_track: None,
            last_track_id: String::new(),
            show_time: None,
            display_duration: 0.0, // 0 = never auto-hide (stay visible while track is playing)
            opacity: 0.0,
            target_opacity: 0.0,
            is_announcing: false,
            animation_progress: 1.0, // Start at corner position
            center_hold_time: 3.0,   // Hold in center for 3 seconds
            shrink_duration: 0.8,    // Shrink animation takes 0.8 seconds
        }
    }
}

impl TrackAnnouncement {
    /// Create a new track announcement handler
    pub fn new() -> Self {
        Self::default()
    }

    /// Update with new track info
    /// Returns true if this is a NEW track (first time we've seen it)
    pub fn update(&mut self, track: Option<TrackInfo>, dt: f32) -> bool {
        // Create a track ID to detect changes
        let track_id = track.as_ref()
            .map(|t| format!("{}:{}", t.artist, t.title))
            .unwrap_or_default();

        // Check if we have a track playing
        let has_playing_track = track.as_ref().map(|t| t.is_playing).unwrap_or(false);

        // Check if track changed
        let is_new_track = track_id != self.last_track_id && track.is_some();

        if is_new_track {
            // New track - start announcement from center
            self.current_track = track.clone();
            self.last_track_id = track_id;
            self.show_time = Some(Instant::now());
            self.target_opacity = 1.0;
            self.is_announcing = true;
            self.animation_progress = 0.0; // Reset to center position
        } else if track.is_some() {
            // Same track - update it (position may have changed)
            self.current_track = track.clone();
        }

        // Stay visible while track is playing, fade out when stopped/paused
        if has_playing_track {
            self.target_opacity = 1.0;
        } else if track.is_none() || !has_playing_track {
            // No track or track stopped/paused - fade out
            self.target_opacity = 0.0;
        }

        // Smooth opacity transition
        let fade_speed = 4.0; // Speed of fade in/out
        if self.opacity < self.target_opacity {
            self.opacity = (self.opacity + dt * fade_speed).min(self.target_opacity);
        } else if self.opacity > self.target_opacity {
            self.opacity = (self.opacity - dt * fade_speed).max(self.target_opacity);
        }

        // Clamp to zero when very small
        if self.opacity < 0.01 && self.target_opacity == 0.0 {
            self.opacity = 0.0;
        }

        // Update animation progress (center → corner)
        if self.animation_progress < 1.0 {
            if let Some(start_time) = self.show_time {
                let elapsed = start_time.elapsed().as_secs_f32();
                if elapsed > self.center_hold_time {
                    // Start shrinking after hold time
                    let shrink_elapsed = elapsed - self.center_hold_time;
                    // Ease-out cubic for smooth deceleration
                    let t = (shrink_elapsed / self.shrink_duration).min(1.0);
                    self.animation_progress = 1.0 - (1.0 - t).powi(3);
                }
            }
        }

        is_new_track
    }

    /// Get current track info if visible
    pub fn current_track(&self) -> Option<&TrackInfo> {
        if self.opacity > 0.01 {
            self.current_track.as_ref()
        } else {
            None
        }
    }

    /// Get current opacity for rendering
    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// Check if announcement is currently visible
    pub fn is_visible(&self) -> bool {
        self.opacity > 0.01
    }

    /// Get animation progress (0.0 = center/large, 1.0 = corner/small)
    pub fn animation_progress(&self) -> f32 {
        self.animation_progress
    }

    /// Force show the current track (for testing or manual trigger)
    pub fn force_show(&mut self) {
        if self.current_track.is_some() {
            self.show_time = Some(Instant::now());
            self.target_opacity = 1.0;
            self.is_announcing = true;
        }
    }
}
