//! Database module for persistent app state using SQLite
//!
//! Stores user preferences, session state, and settings that should
//! persist across app restarts.

use anyhow::{anyhow, Result};
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// App state that gets persisted to the database
#[derive(Debug, Clone)]
pub struct AppState {
    /// Audio gain setting (1.0 = unity, 3.5 = 50% on log slider)
    pub audio_gain: f32,

    /// Kick detection sensitivity (0.5-5.0, 2.75 = 50%)
    pub kick_sensitivity: f32,

    /// Video visibility (0.0-1.0)
    pub video_visibility: f32,

    /// Video scale (0.05-4.0)
    pub video_scale: f32,

    /// Video luma key (0.0-1.0) - makes dark areas transparent
    pub video_luma_key: f32,

    /// Camera layer luma key (0.0-1.0) - dark camera areas become transparent
    pub camera_luma_key: f32,

    /// Last selected preset category
    pub last_category: Option<String>,

    /// Last selected preset name
    pub last_preset: Option<String>,

    /// Whether to show menu on startup
    pub menu_visible: bool,

    /// Last video directory path
    pub video_directory: Option<String>,

    /// Recording output directory path
    pub recording_directory: Option<String>,

    /// Silence fade threshold in seconds
    pub silence_threshold: f32,

    /// Whether to show audio meters
    pub show_audio_meters: bool,

    /// projectM section expanded
    pub projectm_expanded: bool,

    /// Video section expanded
    pub video_expanded: bool,

    /// Audio section expanded
    pub audio_expanded: bool,

    /// Show all apps in audio source list
    pub show_all_apps: bool,

    /// projectM visibility (0.0-1.0)
    pub projectm_visibility: f32,

    /// Current render mode ("projectm" or "led_wall")
    pub render_mode: String,

    /// Logo overlay visible
    pub logo_visible: bool,

    /// Logo overlay opacity (0.0-1.0)
    pub logo_opacity: f32,

    /// Logo overlay scale (0.1-2.0)
    pub logo_scale: f32,

    /// Logo overlay position ("top_left", "top_right", "bottom_left", "bottom_right", "center")
    pub logo_position: String,

    /// Custom logo path (None = use default REACT logo)
    pub custom_logo_path: Option<String>,

    /// Tuning reference ("440" or "432")
    pub tuning_reference: String,

    /// Show Chroma Wheel in Harmonic Field
    pub show_chroma_wheel: bool,

    /// Show Math HUD overlay
    pub show_math_hud: bool,

    /// Hide numbers on QS Wheel
    pub hide_qs_numbers: bool,

    /// Show QS Wheel
    pub show_qs_wheel: bool,

    /// QS Wheel dominant-only mode
    pub qs_dominant_only: bool,

    /// Show Performance HUD
    pub show_perf_hud: bool,

    /// Show Debug HUD
    pub show_debug_hud: bool,

    /// Quality tier (0=Lo, 1=Med, 2=Hi)
    pub quality_tier: u8,

    /// Background quality (0=Lo, 1=Med, 2=Hi)
    pub bg_quality: u8,

    /// Background quality auto-scaling
    pub bg_quality_auto: bool,

    /// Recording audio gain (0.1-16.0, 1.0 = unity)
    pub rec_audio_gain: f32,

    // === Hero Skin Settings ===
    /// Skin mode (0=None, 1=Surface, 2=Emissive, 3=SurfaceEmissive)
    pub skin_mode: i32,
    /// Surface texture mix (0-1)
    pub skin_surface_mix: f32,
    /// Emissive strength (0-1.5)
    pub skin_emissive_strength: f32,
    /// Surface contrast (0.5-2.0)
    pub skin_surf_contrast: f32,
    /// Surface saturation (0-2.0)
    pub skin_surf_saturation: f32,
    /// Shadow lift (0-0.25)
    pub skin_shadow_lift: f32,
    /// AO clamp minimum (0-0.6)
    pub skin_ao_clamp: f32,
    /// Rim/fresnel strength (0-1)
    pub skin_rim_strength: f32,
    /// Bloom amount (0-1)
    pub skin_bloom_amount: f32,
    /// Specular sparkle (0-1)
    pub skin_specular_sparkle: f32,
    /// Edge outline strength (0-1)
    pub skin_edge_outline: f32,
    /// UV mode (0=cylindrical, 1=triplanar)
    pub skin_uv_mode: i32,
    /// Starburst lens effect enabled
    pub skin_starburst_enabled: bool,
    /// Lock UV to dominant pitch
    pub skin_lock_to_dominant: bool,

    // Mythos engine parameters
    pub mythos_instability: f32,
    pub mythos_field_strength: f32,
    pub mythos_disturbance_size: f32,
    pub mythos_harmonic_influence: f32,
    pub mythos_decay: f32,

    // Virtual-stage camera view — orbit around the FOH preset (persisted).
    pub virtual_stage_zoom: f32,
    pub virtual_stage_yaw: f32,
    pub virtual_stage_pitch: f32,
    /// Show the floating fixture-name labels on the virtual stage.
    pub virtual_stage_labels: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            audio_gain: 3.5,        // 50% on log slider (0.1-128)
            kick_sensitivity: 2.75, // 50% on linear slider (0.5-5.0)
            video_visibility: 0.9,
            video_scale: 1.5,
            video_luma_key: 0.0,
            camera_luma_key: 0.0,
            last_category: None,
            last_preset: None,
            menu_visible: true,
            video_directory: None,
            recording_directory: None,
            silence_threshold: 3.0,
            show_audio_meters: true,
            projectm_expanded: true,
            video_expanded: true,
            audio_expanded: true,
            show_all_apps: false,
            projectm_visibility: 1.0,
            render_mode: "projectm".to_string(),
            logo_visible: true,
            logo_opacity: 0.7,
            logo_scale: 0.5,
            logo_position: "bottom_right".to_string(),
            custom_logo_path: None,
            tuning_reference: "440".to_string(),  // Standard concert pitch
            show_chroma_wheel: false,  // Off by default
            show_math_hud: false,      // Off by default
            hide_qs_numbers: true,     // Hidden by default
            show_qs_wheel: true,       // QS Wheel visible by default
            qs_dominant_only: true,    // Dominant-only mode by default
            show_perf_hud: false,      // Perf HUD off by default
            show_debug_hud: false,     // Debug HUD off by default
            quality_tier: 1,           // Medium quality by default
            bg_quality: 1,             // Medium BG quality by default
            bg_quality_auto: true,     // Auto-scaling enabled by default
            rec_audio_gain: 1.0,       // Unity gain by default
            // Hero skin defaults (from FieldSkin::hero())
            skin_mode: 3,              // SurfaceEmissive
            skin_surface_mix: 0.9,
            skin_emissive_strength: 0.5,
            skin_surf_contrast: 1.35,
            skin_surf_saturation: 1.25,
            skin_shadow_lift: 0.08,
            skin_ao_clamp: 0.35,
            skin_rim_strength: 0.25,
            skin_bloom_amount: 0.3,
            skin_specular_sparkle: 0.2,
            skin_edge_outline: 0.0,
            skin_uv_mode: 1,           // Triplanar
            skin_starburst_enabled: false,
            skin_lock_to_dominant: false,
            // Mythos defaults
            mythos_instability: 0.50,
            mythos_field_strength: 0.60,
            mythos_disturbance_size: 0.40,
            mythos_harmonic_influence: 0.30,
            mythos_decay: 0.50,
            // Virtual-stage camera: 1.0 zoom, no orbit.
            virtual_stage_zoom: 1.0,
            virtual_stage_yaw: 0.0,
            virtual_stage_pitch: 0.0,
            virtual_stage_labels: true,
        }
    }
}

/// Database connection wrapper (Clone via Arc)
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open or create the database at the standard config location
    pub fn open() -> Result<Self> {
        let db_path = Self::get_db_path()?;

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        log::info!("[DB] Opening database at {:?}", db_path);

        let conn = Connection::open(&db_path)?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        // Initialize schema
        db.init_schema()?;

        // Seed default pinned presets (if none exist)
        db.seed_default_pins()?;

        // Load cached Twitch server data for faster startup
        db.load_twitch_server_cache();

        Ok(db)
    }

    /// Seed default pinned presets if table is empty
    fn seed_default_pins(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Check if there are any pinned presets
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM pinned_presets",
            [],
            |row| row.get(0),
        )?;

        if count > 0 {
            return Ok(()); // Already has pins, don't overwrite
        }

        // Add default pinned presets
        let default_pins = [
            ("Geometric Field", "Hero's Journey"),
            ("LED Wall", "Spectrum Analyzer"),
            ("LED Wall", "Center Pulse"),
        ];

        for (i, (cat, name)) in default_pins.iter().enumerate() {
            conn.execute(
                "INSERT OR IGNORE INTO pinned_presets (category, preset_name, sort_order) VALUES (?, ?, ?)",
                params![cat, name, i as i32],
            )?;
        }

        log::info!("[DB] Seeded {} default pinned presets", default_pins.len());
        Ok(())
    }

    /// Load cached Twitch server ping data from database
    fn load_twitch_server_cache(&self) {
        let conn = self.conn.lock().unwrap();
        match crate::output::load_servers_from_db(&conn) {
            Ok(is_fresh) => {
                if is_fresh {
                    log::info!("[DB] Loaded fresh Twitch server cache from database");
                } else {
                    log::info!("[DB] Loaded stale Twitch server cache (will re-ping on first use)");
                }
            }
            Err(e) => {
                log::warn!("[DB] Failed to load Twitch server cache: {}", e);
            }
        }
    }

    /// Save Twitch server ping data to database
    pub fn save_twitch_server_cache(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        crate::output::save_servers_to_db(&conn)?;
        Ok(())
    }

    /// Get the database file path
    fn get_db_path() -> Result<PathBuf> {
        // Use platform-specific config directory
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not find config directory"))?;

        Ok(config_dir.join("compeller").join("react.db"))
    }

    /// Initialize database schema
    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Key-value store for app settings
        conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Offline stage cache: the last-synced fixture patch per stage so REACT
        // can render a stage with no internet (SaaS stays authority when online).
        // `saas_updated_at` records the stage's SaaS lastUpdated at cache time —
        // reserved for reconnect conflict detection (queued local edits vs a
        // SaaS-side change).
        conn.execute(
            "CREATE TABLE IF NOT EXISTS cached_stages (
                stage_id INTEGER PRIMARY KEY,
                patch_json TEXT NOT NULL,
                saas_updated_at TEXT,
                cached_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // COM-499 offline lighting cues: the local source of truth for the
        // per-frame executor, so a running show drives with ~zero SaaS calls (the
        // showtime loop must not depend on the cue poll). When online the worker
        // mirrors fetched cues here; the renderer hydrates from this store on a
        // cold/offline start; operator-launched cues (origin='local') are written
        // here directly. `cue_json` is the full CueDto (launch_contract / variables
        // / metadata) so `ActiveCue::from_dto` can rebuild the cue with no network.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS active_cues (
                launch_id TEXT PRIMARY KEY,
                stage_id INTEGER,
                status TEXT NOT NULL,
                cue_json TEXT NOT NULL,
                origin TEXT NOT NULL DEFAULT 'saas',
                promoted_to TEXT,
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_active_cues_stage ON active_cues(stage_id)",
            [],
        )?;
        // Migration: add promoted_to to pre-existing active_cues DBs (COM-499
        // promote-on-connectivity). Error swallowed when the column already exists.
        let _ = conn.execute("ALTER TABLE active_cues ADD COLUMN promoted_to TEXT", []);

        // COM-499 cache-from-launches: every cue seen online is remembered here as
        // a re-launchable DEFINITION (keyed by cue_profile_id / effect_id), so the
        // operator can fire it locally with zero SaaS — even offline. `cue_json` is
        // a representative CueDto cloned (new launch_id, status ACTIVE) on launch.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS cue_definitions (
                def_key TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                cue_json TEXT NOT NULL,
                stage_id INTEGER,
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_cue_definitions_stage ON cue_definitions(stage_id)",
            [],
        )?;

        // Preset favorites
        conn.execute(
            "CREATE TABLE IF NOT EXISTS favorites (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                category TEXT NOT NULL,
                preset_name TEXT NOT NULL,
                added_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(category, preset_name)
            )",
            [],
        )?;

        // Pinned presets (works for ALL renderer types - projectM, LED Wall, etc.)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pinned_presets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                category TEXT NOT NULL,
                preset_name TEXT NOT NULL,
                sort_order INTEGER DEFAULT 0,
                added_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(category, preset_name)
            )",
            [],
        )?;

        // Recent presets history
        conn.execute(
            "CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                category TEXT NOT NULL,
                preset_name TEXT NOT NULL,
                played_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Index for faster history queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_history_played ON history(played_at DESC)",
            [],
        )?;

        // Preset metadata (custom names, notes, etc.)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS preset_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                custom_name TEXT,
                notes TEXT,
                rating INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                hidden INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Migration: Add hidden column if it doesn't exist (for existing databases)
        let _ = conn.execute(
            "ALTER TABLE preset_metadata ADD COLUMN hidden INTEGER DEFAULT 0",
            [],
        );

        // Migration: Add subcategory column for folder hierarchy
        let _ = conn.execute(
            "ALTER TABLE preset_metadata ADD COLUMN subcategory TEXT",
            [],
        );

        // Video associations with presets
        conn.execute(
            "CREATE TABLE IF NOT EXISTS preset_videos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                preset_path TEXT NOT NULL,
                video_path TEXT NOT NULL,
                priority INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(preset_path, video_path)
            )",
            [],
        )?;

        // Song/track associations with presets (themes)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS preset_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                preset_path TEXT NOT NULL,
                track_name TEXT NOT NULL,
                artist TEXT,
                album TEXT,
                priority INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(preset_path, track_name, artist)
            )",
            [],
        )?;

        // Indexes for faster lookups
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_preset_metadata_path ON preset_metadata(file_path)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_preset_videos_preset ON preset_videos(preset_path)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_preset_tracks_preset ON preset_tracks(preset_path)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_preset_tracks_track ON preset_tracks(track_name, artist)",
            [],
        )?;

        // Recordings metadata table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recordings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                file_name TEXT NOT NULL,
                custom_name TEXT,
                notes TEXT,
                duration_secs REAL,
                file_size_bytes INTEGER,
                width INTEGER,
                height INTEGER,
                fps REAL,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_recordings_path ON recordings(file_path)",
            [],
        )?;

        // Video (Compel) metadata table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS video_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                custom_name TEXT,
                notes TEXT,
                hidden INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_video_metadata_path ON video_metadata(file_path)",
            [],
        )?;

        // Song/track associations with videos (compels)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS video_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                video_path TEXT NOT NULL,
                track_name TEXT NOT NULL,
                artist TEXT,
                album TEXT,
                priority INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(video_path, track_name, artist)
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_video_tracks_video ON video_tracks(video_path)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_video_tracks_track ON video_tracks(track_name, artist)",
            [],
        )?;

        // Add favorite column to video_metadata if it doesn't exist
        let _ = conn.execute(
            "ALTER TABLE video_metadata ADD COLUMN favorite INTEGER DEFAULT 0",
            [],
        );

        // Add thumbnail columns to video_metadata if they don't exist
        let _ = conn.execute(
            "ALTER TABLE video_metadata ADD COLUMN thumbnail_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE video_metadata ADD COLUMN thumbnail_generated INTEGER DEFAULT 0",
            [],
        );

        // Add thumbnail columns to recordings if they don't exist
        let _ = conn.execute(
            "ALTER TABLE recordings ADD COLUMN thumbnail_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE recordings ADD COLUMN thumbnail_generated INTEGER DEFAULT 0",
            [],
        );

        // Recent videos table (ordered list of recently played videos)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recent_videos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                played_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Sync videos table - tracks sync state for each remote video
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sync_videos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                remote_url TEXT NOT NULL,
                remote_size INTEGER NOT NULL,
                local_size INTEGER DEFAULT 0,
                status TEXT DEFAULT 'pending',
                error TEXT,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sync_videos_status ON sync_videos(status)",
            [],
        )?;

        // Compel videos table - tracks sync state for authenticated user's compel videos
        conn.execute(
            "CREATE TABLE IF NOT EXISTS compel_videos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                compel_id INTEGER NOT NULL,
                video_id INTEGER NOT NULL UNIQUE,
                name TEXT NOT NULL,
                artist TEXT,
                title TEXT,
                remote_size INTEGER NOT NULL DEFAULT 0,
                local_size INTEGER DEFAULT 0,
                status TEXT DEFAULT 'pending',
                error TEXT,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_compel_videos_status ON compel_videos(status)",
            [],
        )?;

        // CDJ devices table - tracks discovered Pro DJ Link devices and their state
        conn.execute(
            "CREATE TABLE IF NOT EXISTS cdj_devices (
                device_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                ip TEXT NOT NULL,
                track_title TEXT,
                track_artist TEXT,
                is_master INTEGER DEFAULT 0,
                is_playing INTEGER DEFAULT 0,
                is_looping INTEGER DEFAULT 0,
                is_on_air INTEGER DEFAULT 0,
                bpm REAL,
                last_seen INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // MIDI mappings table - stores controller mappings
        conn.execute(
            "CREATE TABLE IF NOT EXISTS midi_mappings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                device_name TEXT NOT NULL,
                channel INTEGER NOT NULL,
                msg_type TEXT NOT NULL,
                msg_index INTEGER NOT NULL,
                target TEXT NOT NULL,
                min_value REAL DEFAULT 0.0,
                max_value REAL DEFAULT 1.0,
                inverted INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(device_name, channel, msg_type, msg_index)
            )",
            [],
        )?;

        // MIDI preset bindings - map MIDI notes to specific presets
        conn.execute(
            "CREATE TABLE IF NOT EXISTS midi_preset_bindings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                device_name TEXT NOT NULL DEFAULT '',
                channel INTEGER NOT NULL DEFAULT 255,
                note INTEGER NOT NULL,
                preset_id TEXT NOT NULL,
                name TEXT,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(device_name, channel, note)
            )",
            [],
        )?;

        // Camera tracking settings - per-camera visibility and enable state
        conn.execute(
            "CREATE TABLE IF NOT EXISTS camera_settings (
                camera_index INTEGER PRIMARY KEY,
                visibility_step INTEGER DEFAULT 16,
                enabled INTEGER DEFAULT 1,
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Camera media settings - per-camera background media visibility (separate from tracking)
        // This controls alpha compositing of camera feeds as background layers
        conn.execute(
            "CREATE TABLE IF NOT EXISTS camera_media_settings (
                camera_index INTEGER PRIMARY KEY,
                visibility_step INTEGER DEFAULT 0,
                enabled INTEGER DEFAULT 0,
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // ========== Authentication Tables ==========

        // Compeller auth credentials - stores session tokens
        conn.execute(
            "CREATE TABLE IF NOT EXISTS auth_credentials (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL,
                access_token TEXT NOT NULL,
                refresh_token TEXT,
                roles TEXT DEFAULT '',
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Login credentials (for auto-login) - stored separately from tokens
        // Note: In production, consider using system keychain instead
        conn.execute(
            "CREATE TABLE IF NOT EXISTS login_credentials (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL,
                password TEXT NOT NULL,
                created_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Connected accounts (OAuth providers like YouTube/Google)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS connected_accounts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL UNIQUE,
                state TEXT NOT NULL DEFAULT 'disconnected',
                refresh_token TEXT,
                access_token TEXT,
                token_expires_at INTEGER,
                scopes_granted TEXT DEFAULT '',
                provider_user_id TEXT,
                provider_display_name TEXT,
                connected_at INTEGER DEFAULT 0,
                last_refreshed_at INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_connected_accounts_provider ON connected_accounts(provider)",
            [],
        )?;

        // License blob cache (for offline grace period)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS license_blob (
                id INTEGER PRIMARY KEY,
                blob TEXT NOT NULL,
                signature TEXT NOT NULL,
                fetched_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Twitch ingest servers cache
        crate::output::init_twitch_db_table(&conn)?;

        // ========== Segmented Recording Tables (Crash-Resilient) ==========

        // Recording sessions - tracks overall recording state
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recording_sessions (
                session_id TEXT PRIMARY KEY,
                output_dir TEXT NOT NULL,
                segment_duration_ms INTEGER NOT NULL DEFAULT 10000,
                width INTEGER NOT NULL,
                height INTEGER NOT NULL,
                fps INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                total_segments INTEGER DEFAULT 0,
                total_duration_ms INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        // Segment journal - two-phase commit log for crash recovery
        conn.execute(
            "CREATE TABLE IF NOT EXISTS segment_journal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                segment_index INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'begun',
                filename_tmp TEXT NOT NULL,
                filename_final TEXT,
                duration_ms INTEGER,
                first_pts_ns INTEGER,
                last_pts_ns INTEGER,
                file_size_bytes INTEGER,
                video_frames INTEGER,
                audio_samples INTEGER,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now')),
                UNIQUE(session_id, segment_index),
                FOREIGN KEY(session_id) REFERENCES recording_sessions(session_id)
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_segment_journal_session ON segment_journal(session_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_segment_journal_status ON segment_journal(status)",
            [],
        )?;

        // ========== Compel Set Upload Tables ==========

        // Set uploads - tracks upload state for recordings being uploaded to Compeller.ai
        conn.execute(
            "CREATE TABLE IF NOT EXISTS set_uploads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                file_name TEXT NOT NULL,
                total_size INTEGER NOT NULL,
                uploaded_size INTEGER DEFAULT 0,
                session_token TEXT,
                status TEXT DEFAULT 'pending',
                error TEXT,
                title TEXT,
                recorded_at TEXT,
                theme_used TEXT,
                audio_source TEXT,
                duration_seconds INTEGER,
                checksum TEXT,
                remote_set_id INTEGER,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_set_uploads_status ON set_uploads(status)",
            [],
        )?;

        log::info!("[DB] Schema initialized");
        Ok(())
    }

    /// Get a setting value
    pub fn get_setting(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?",
            [key],
            |row| row.get(0),
        ).ok()
    }

    /// Set a setting value
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))",
            params![key, value],
        )?;
        Ok(())
    }

    /// COM-499 Phase B4: persist the operator LFO bank as a JSON blob in the
    /// existing `settings` table (the bank is one small config object — no new
    /// table). Runtime fields (`phase`/`output`) are `#[serde(skip)]`, so only
    /// config is written.
    pub fn save_lfo_bank(&self, bank: &crate::emerge::LfoBankConfig) -> Result<()> {
        let json = serde_json::to_string(bank).unwrap_or_default();
        self.set_setting("lfo_bank", &json)
    }

    /// Load the operator LFO bank; returns the default (8 disabled slots) if the
    /// blob is absent or unparseable, so a corrupt or older blob can never wipe
    /// the running rig. `#[serde(default)]` on the slots tolerates added fields.
    pub fn load_lfo_bank(&self) -> crate::emerge::LfoBankConfig {
        self.get_setting("lfo_bank")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Delete a setting
    pub fn delete_setting(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM settings WHERE key = ?", params![key])?;
        Ok(())
    }

    /// Get a float setting
    pub fn get_float(&self, key: &str, default: f32) -> f32 {
        self.get_setting(key)
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    /// Set a float setting
    pub fn set_float(&self, key: &str, value: f32) -> Result<()> {
        self.set_setting(key, &value.to_string())
    }

    /// Get a bool setting
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get_setting(key)
            .map(|s| s == "1" || s.to_lowercase() == "true")
            .unwrap_or(default)
    }

    /// Set a bool setting
    pub fn set_bool(&self, key: &str, value: bool) -> Result<()> {
        self.set_setting(key, if value { "1" } else { "0" })
    }

    // ---- Offline stage cache (SaaS stays authority when online) ----

    /// Cache a stage's fixture patch (JSON) for offline load. `saas_updated_at`
    /// is the SaaS stage `lastUpdated` at fetch time (reserved for reconnect
    /// conflict detection); pass `None` until that is wired.
    pub fn cache_stage_patch(
        &self,
        stage_id: i64,
        patch_json: &str,
        saas_updated_at: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO cached_stages (stage_id, patch_json, saas_updated_at, cached_at)
             VALUES (?, ?, ?, strftime('%s', 'now'))",
            params![stage_id, patch_json, saas_updated_at],
        )?;
        Ok(())
    }

    /// Load a cached stage patch (JSON), if one exists for this stage.
    pub fn cached_stage_patch(&self, stage_id: i64) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT patch_json FROM cached_stages WHERE stage_id = ?",
            [stage_id],
            |row| row.get(0),
        )
        .ok()
    }

    /// Cache the SaaS stage list (JSON) for the offline Stage: dropdown.
    pub fn cache_stage_list(&self, json: &str) -> Result<()> {
        self.set_setting("cached_stage_list", json)
    }

    /// Load the cached stage list (JSON), if any.
    pub fn cached_stage_list(&self) -> Option<String> {
        self.get_setting("cached_stage_list")
    }

    /// Remember the last stage the operator selected (for startup restore).
    pub fn set_last_stage_id(&self, stage_id: i64) -> Result<()> {
        self.set_setting("last_stage_id", &stage_id.to_string())
    }

    /// Recall the last selected stage id, if any.
    pub fn last_stage_id(&self) -> Option<i64> {
        self.get_setting("last_stage_id").and_then(|s| s.parse().ok())
    }

    /// Cache the worker's fixture-key -> SaaS-id map (JSON) per stage, so that
    /// after an offline start the worker can still address existing rows by id
    /// — avoiding a duplicate Create on reconnect (slice 3 part 1).
    pub fn cache_stage_id_map(&self, stage_id: i64, json: &str) -> Result<()> {
        self.set_setting(&format!("stage_idmap_{}", stage_id), json)
    }

    /// Load the cached fixture-key -> SaaS-id map (JSON) for a stage.
    pub fn cached_stage_id_map(&self, stage_id: i64) -> Option<String> {
        self.get_setting(&format!("stage_idmap_{}", stage_id))
    }

    // ---- COM-499 offline lighting-cue store (the executor's source of truth) ----

    /// Mirror a cue (full CueDto JSON) into the local store so the per-frame
    /// executor can drive it offline. `origin` is `saas` for a SaaS-minted cue the
    /// worker fetched, or `local` for an operator-launched cue. Upserts on
    /// `launch_id` (latest status/vars win), bumping `updated_at`.
    pub fn upsert_active_cue(
        &self,
        launch_id: &str,
        stage_id: i64,
        status: &str,
        cue_json: &str,
        origin: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO active_cues (launch_id, stage_id, status, cue_json, origin, updated_at)
             VALUES (?, ?, ?, ?, ?, strftime('%s', 'now'))
             ON CONFLICT(launch_id) DO UPDATE SET
                 stage_id   = excluded.stage_id,
                 status     = excluded.status,
                 cue_json   = excluded.cue_json,
                 updated_at = excluded.updated_at",
            params![launch_id, stage_id, status, cue_json, origin],
        )?;
        Ok(())
    }

    /// Stored cue JSONs for a stage, optionally filtered by `origin`, ordered
    /// oldest-first and tie-broken by `launch_id` so an offline-restart compose is
    /// DETERMINISTIC (approximating the server's `lastUpdated` order, so an
    /// equal-priority `override` tiebreak doesn't silently flip after a restart).
    /// `Some("saas")` seeds the live SaaS set on a cold/offline start; `Some("local")`
    /// is the operator-launched overlay; `None` returns both.
    pub fn active_cue_jsons_for_stage(&self, stage_id: i64, origin: Option<&str>) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let map_rows = |stmt: &mut rusqlite::Statement, p: &[&dyn rusqlite::ToSql]| -> Vec<String> {
            stmt.query_map(p, |row| row.get::<_, String>(0))
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };
        match origin {
            Some(o) => {
                let mut stmt = match conn.prepare(
                    "SELECT cue_json FROM active_cues WHERE stage_id = ? AND origin = ? ORDER BY updated_at ASC, launch_id ASC",
                ) {
                    Ok(s) => s,
                    Err(_) => return Vec::new(),
                };
                map_rows(&mut stmt, &[&stage_id, &o])
            }
            None => {
                let mut stmt = match conn.prepare(
                    "SELECT cue_json FROM active_cues WHERE stage_id = ? ORDER BY updated_at ASC, launch_id ASC",
                ) {
                    Ok(s) => s,
                    Err(_) => return Vec::new(),
                };
                map_rows(&mut stmt, &[&stage_id])
            }
        }
    }

    /// The launch_ids stored for a stage, optionally filtered by `origin` — the
    /// worker uses `Some("saas")` to prune cues the server no longer returns
    /// (reconcile) without touching operator-local cues.
    pub fn active_cue_ids_for_stage(&self, stage_id: i64, origin: Option<&str>) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let map_rows = |stmt: &mut rusqlite::Statement, p: &[&dyn rusqlite::ToSql]| -> Vec<String> {
            stmt.query_map(p, |row| row.get::<_, String>(0))
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };
        match origin {
            Some(o) => {
                let mut stmt = match conn
                    .prepare("SELECT launch_id FROM active_cues WHERE stage_id = ? AND origin = ?")
                {
                    Ok(s) => s,
                    Err(_) => return Vec::new(),
                };
                map_rows(&mut stmt, &[&stage_id, &o])
            }
            None => {
                let mut stmt = match conn
                    .prepare("SELECT launch_id FROM active_cues WHERE stage_id = ?")
                {
                    Ok(s) => s,
                    Err(_) => return Vec::new(),
                };
                map_rows(&mut stmt, &[&stage_id])
            }
        }
    }

    /// Drop a stored cue (server no longer returns it, or it finished STOPPED).
    pub fn remove_active_cue(&self, launch_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM active_cues WHERE launch_id = ?", params![launch_id])?;
        Ok(())
    }

    /// Drop SaaS-origin cue rows for every stage EXCEPT `keep_stage_id`. Called on
    /// a stage switch so an abandoned stage's stale cues can't resurrect when the
    /// operator returns to it (the next successful poll repopulates the current
    /// stage). Operator-local cues (origin = 'local') are preserved across stages.
    pub fn prune_saas_active_cues_other_stages(&self, keep_stage_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM active_cues WHERE origin = 'saas' AND stage_id <> ?",
            params![keep_stage_id],
        )?;
        Ok(())
    }

    /// Operator-local cues for a stage as `(launch_id, cue_json, promoted_to)` — the
    /// COM-499 promote-on-connectivity pass: un-promoted (`promoted_to` None) rows
    /// get pushed up to SaaS; a promoted row is dropped once its server cue arrives.
    pub fn local_active_cues_for_promote(&self, stage_id: i64) -> Vec<(String, String, Option<String>)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT launch_id, cue_json, promoted_to FROM active_cues WHERE stage_id = ? AND origin = 'local'",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![stage_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Record that a local cue was promoted to SaaS, storing the server-minted
    /// launch_id so the next poll can swap the local row out when it arrives.
    pub fn set_active_cue_promoted(&self, launch_id: &str, server_launch_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE active_cues SET promoted_to = ?, updated_at = strftime('%s', 'now') WHERE launch_id = ?",
            params![server_launch_id, launch_id],
        )?;
        Ok(())
    }

    /// The server launch_id a local cue was promoted to, if any — so an operator
    /// stop of an already-promoted (not-yet-swapped) local cue can also stop the
    /// SaaS cue and keep it from re-appearing on the next poll.
    pub fn active_cue_promoted_to(&self, launch_id: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT promoted_to FROM active_cues WHERE launch_id = ?",
            [launch_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    // ---- COM-499 cache-from-launches cue definitions (operator re-launch) ----

    /// Remember a cue's definition so the operator can re-launch it locally later.
    /// Keyed by `def_key` (cue_profile_id / effect_id); the latest launch's
    /// variables win (upsert), matching the cache-from-launches contract.
    pub fn upsert_cue_definition(
        &self,
        def_key: &str,
        label: &str,
        cue_json: &str,
        stage_id: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cue_definitions (def_key, label, cue_json, stage_id, updated_at)
             VALUES (?, ?, ?, ?, strftime('%s', 'now'))
             ON CONFLICT(def_key) DO UPDATE SET
                 label      = excluded.label,
                 cue_json   = excluded.cue_json,
                 stage_id   = excluded.stage_id,
                 updated_at = excluded.updated_at",
            params![def_key, label, cue_json, stage_id],
        )?;
        Ok(())
    }

    /// Cached cue definitions for a stage as `(def_key, label)`, alphabetically by
    /// label — the operator's local-launch picker.
    pub fn cue_definitions_for_stage(&self, stage_id: i64) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT def_key, label FROM cue_definitions WHERE stage_id = ? ORDER BY label ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![stage_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// The stored `cue_json` for a definition, cloned (new launch_id, status
    /// ACTIVE) at operator launch time.
    pub fn cue_definition_json(&self, def_key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT cue_json FROM cue_definitions WHERE def_key = ?",
            [def_key],
            |row| row.get(0),
        )
        .ok()
    }

    /// Load full app state from database
    pub fn load_state(&self) -> AppState {
        AppState {
            audio_gain: self.get_float("audio_gain", 3.5),
            kick_sensitivity: self.get_float("kick_sensitivity", 2.75),
            video_visibility: self.get_float("video_visibility", 1.0),
            video_scale: self.get_float("video_scale", 1.5),
            video_luma_key: self.get_float("video_luma_key", 0.0),
            camera_luma_key: self.get_float("camera_luma_key", 0.0),
            last_category: self.get_setting("last_category"),
            last_preset: self.get_setting("last_preset"),
            menu_visible: self.get_bool("menu_visible", true),
            video_directory: self.get_setting("video_directory"),
            recording_directory: self.get_setting("recording_directory"),
            silence_threshold: self.get_float("silence_threshold", 3.0),
            show_audio_meters: self.get_bool("show_audio_meters", true),
            projectm_expanded: self.get_bool("projectm_expanded", true),
            video_expanded: self.get_bool("video_expanded", true),
            audio_expanded: self.get_bool("audio_expanded", true),
            show_all_apps: self.get_bool("show_all_apps", false),
            projectm_visibility: self.get_float("projectm_visibility", 1.0),
            render_mode: self.get_setting("render_mode").unwrap_or_else(|| "projectm".to_string()),
            logo_visible: self.get_bool("logo_visible", true),
            logo_opacity: self.get_float("logo_opacity", 0.7),
            logo_scale: self.get_float("logo_scale", 0.5),
            logo_position: self.get_setting("logo_position").unwrap_or_else(|| "bottom_right".to_string()),
            custom_logo_path: self.get_setting("custom_logo_path"),
            tuning_reference: self.get_setting("tuning_reference").unwrap_or_else(|| "440".to_string()),
            show_chroma_wheel: self.get_bool("show_chroma_wheel", false),
            show_math_hud: self.get_bool("show_math_hud", false),
            hide_qs_numbers: self.get_bool("hide_qs_numbers", true),
            show_qs_wheel: self.get_bool("show_qs_wheel", true),
            qs_dominant_only: self.get_bool("qs_dominant_only", true),
            show_perf_hud: self.get_bool("show_perf_hud", false),
            show_debug_hud: self.get_bool("show_debug_hud", false),
            quality_tier: self.get_setting("quality_tier")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            bg_quality: self.get_setting("bg_quality")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            bg_quality_auto: self.get_bool("bg_quality_auto", true),
            rec_audio_gain: self.get_float("rec_audio_gain", 1.0),
            // Hero skin settings
            skin_mode: self.get_setting("skin_mode")
                .and_then(|s| s.parse().ok())
                .unwrap_or(3), // SurfaceEmissive
            skin_surface_mix: self.get_float("skin_surface_mix", 0.9),
            skin_emissive_strength: self.get_float("skin_emissive_strength", 0.5),
            skin_surf_contrast: self.get_float("skin_surf_contrast", 1.35),
            skin_surf_saturation: self.get_float("skin_surf_saturation", 1.25),
            skin_shadow_lift: self.get_float("skin_shadow_lift", 0.08),
            skin_ao_clamp: self.get_float("skin_ao_clamp", 0.35),
            skin_rim_strength: self.get_float("skin_rim_strength", 0.25),
            skin_bloom_amount: self.get_float("skin_bloom_amount", 0.3),
            skin_specular_sparkle: self.get_float("skin_specular_sparkle", 0.2),
            skin_edge_outline: self.get_float("skin_edge_outline", 0.0),
            skin_uv_mode: self.get_setting("skin_uv_mode")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1), // Triplanar
            skin_starburst_enabled: self.get_bool("skin_starburst_enabled", false),
            skin_lock_to_dominant: self.get_bool("skin_lock_to_dominant", false),
            // Mythos
            mythos_instability: self.get_float("mythos_instability", 0.50),
            mythos_field_strength: self.get_float("mythos_field_strength", 0.60),
            mythos_disturbance_size: self.get_float("mythos_disturbance_size", 0.40),
            mythos_harmonic_influence: self.get_float("mythos_harmonic_influence", 0.30),
            mythos_decay: self.get_float("mythos_decay", 0.50),
            virtual_stage_zoom: self.get_float("virtual_stage_zoom", 1.0),
            virtual_stage_yaw: self.get_float("virtual_stage_yaw", 0.0),
            virtual_stage_pitch: self.get_float("virtual_stage_pitch", 0.0),
            virtual_stage_labels: self.get_bool("virtual_stage_labels", true),
        }
    }

    /// Save full app state to database
    pub fn save_state(&self, state: &AppState) -> Result<()> {
        self.set_float("audio_gain", state.audio_gain)?;
        self.set_float("kick_sensitivity", state.kick_sensitivity)?;
        self.set_float("video_visibility", state.video_visibility)?;
        self.set_float("video_scale", state.video_scale)?;
        if let Some(ref cat) = state.last_category {
            self.set_setting("last_category", cat)?;
        }
        if let Some(ref preset) = state.last_preset {
            self.set_setting("last_preset", preset)?;
        }
        self.set_bool("menu_visible", state.menu_visible)?;
        if let Some(ref dir) = state.video_directory {
            self.set_setting("video_directory", dir)?;
        }
        if let Some(ref dir) = state.recording_directory {
            self.set_setting("recording_directory", dir)?;
        }
        self.set_float("silence_threshold", state.silence_threshold)?;
        self.set_bool("show_audio_meters", state.show_audio_meters)?;
        self.set_bool("projectm_expanded", state.projectm_expanded)?;
        self.set_bool("video_expanded", state.video_expanded)?;
        self.set_bool("audio_expanded", state.audio_expanded)?;
        self.set_bool("show_all_apps", state.show_all_apps)?;
        self.set_float("projectm_visibility", state.projectm_visibility)?;
        self.set_setting("render_mode", &state.render_mode)?;
        self.set_bool("show_chroma_wheel", state.show_chroma_wheel)?;
        self.set_bool("show_math_hud", state.show_math_hud)?;
        self.set_bool("hide_qs_numbers", state.hide_qs_numbers)?;
        self.set_bool("show_qs_wheel", state.show_qs_wheel)?;
        self.set_bool("qs_dominant_only", state.qs_dominant_only)?;
        self.set_bool("show_perf_hud", state.show_perf_hud)?;
        self.set_bool("show_debug_hud", state.show_debug_hud)?;
        self.set_setting("quality_tier", &state.quality_tier.to_string())?;
        self.set_setting("bg_quality", &state.bg_quality.to_string())?;
        self.set_bool("bg_quality_auto", state.bg_quality_auto)?;
        self.set_float("rec_audio_gain", state.rec_audio_gain)?;
        Ok(())
    }

    /// Add a preset to favorites
    pub fn add_favorite(&self, category: &str, preset_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO favorites (category, preset_name) VALUES (?, ?)",
            params![category, preset_name],
        )?;
        log::info!("[DB] Added favorite: {} / {}", category, preset_name);
        Ok(())
    }

    /// Remove a preset from favorites
    pub fn remove_favorite(&self, category: &str, preset_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM favorites WHERE category = ? AND preset_name = ?",
            params![category, preset_name],
        )?;
        Ok(())
    }

    /// Check if a preset is favorited
    pub fn is_favorite(&self, category: &str, preset_name: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM favorites WHERE category = ? AND preset_name = ?",
            params![category, preset_name],
            |_| Ok(()),
        ).is_ok()
    }

    /// Get all favorites
    pub fn get_favorites(&self) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT category, preset_name FROM favorites ORDER BY added_at DESC") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    // ============ Pinned Presets ============

    /// Pin a preset (works for any renderer type)
    pub fn pin_preset(&self, category: &str, preset_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Get max sort_order and add 1
        let max_order: i32 = conn.query_row(
            "SELECT COALESCE(MAX(sort_order), 0) FROM pinned_presets",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        conn.execute(
            "INSERT OR IGNORE INTO pinned_presets (category, preset_name, sort_order) VALUES (?, ?, ?)",
            params![category, preset_name, max_order + 1],
        )?;
        log::info!("[DB] Pinned preset: {} / {}", category, preset_name);
        Ok(())
    }

    /// Unpin a preset
    pub fn unpin_preset(&self, category: &str, preset_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM pinned_presets WHERE category = ? AND preset_name = ?",
            params![category, preset_name],
        )?;
        log::info!("[DB] Unpinned preset: {} / {}", category, preset_name);
        Ok(())
    }

    /// Check if a preset is pinned
    pub fn is_pinned(&self, category: &str, preset_name: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM pinned_presets WHERE category = ? AND preset_name = ?",
            params![category, preset_name],
            |_| Ok(()),
        ).is_ok()
    }

    /// Get all pinned presets (sorted by sort_order)
    pub fn get_pinned_presets(&self) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT category, preset_name FROM pinned_presets ORDER BY sort_order ASC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Add preset to history
    pub fn add_to_history(&self, category: &str, preset_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO history (category, preset_name) VALUES (?, ?)",
            params![category, preset_name],
        )?;

        // Keep only last 100 history entries
        conn.execute(
            "DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY played_at DESC LIMIT 100)",
            [],
        )?;

        Ok(())
    }

    /// Get recent history
    pub fn get_history(&self, limit: usize) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT category, preset_name FROM history ORDER BY played_at DESC LIMIT ?"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    // ========== Preset Metadata ==========

    /// Set custom name for a preset
    pub fn set_preset_custom_name(&self, file_path: &str, custom_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preset_metadata (file_path, custom_name, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET custom_name = excluded.custom_name, updated_at = excluded.updated_at",
            params![file_path, custom_name],
        )?;
        Ok(())
    }

    /// Get custom name for a preset (returns None if not set)
    pub fn get_preset_custom_name(&self, file_path: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT custom_name FROM preset_metadata WHERE file_path = ?",
            [file_path],
            |row| row.get(0),
        ).ok().flatten()
    }

    /// Set notes for a preset
    pub fn set_preset_notes(&self, file_path: &str, notes: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preset_metadata (file_path, notes, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET notes = excluded.notes, updated_at = excluded.updated_at",
            params![file_path, notes],
        )?;
        Ok(())
    }

    /// Set rating for a preset (1-5)
    pub fn set_preset_rating(&self, file_path: &str, rating: i32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preset_metadata (file_path, rating, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET rating = excluded.rating, updated_at = excluded.updated_at",
            params![file_path, rating.clamp(0, 5)],
        )?;
        Ok(())
    }

    /// Increment play count for a preset
    pub fn increment_preset_play_count(&self, file_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preset_metadata (file_path, play_count, updated_at)
             VALUES (?, 1, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET play_count = play_count + 1, updated_at = excluded.updated_at",
            params![file_path],
        )?;
        Ok(())
    }

    /// Get full preset metadata
    pub fn get_preset_metadata(&self, file_path: &str) -> Option<PresetMetadata> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT file_path, custom_name, notes, rating, play_count, hidden, subcategory FROM preset_metadata WHERE file_path = ?",
            [file_path],
            |row| Ok(PresetMetadata {
                file_path: row.get(0)?,
                custom_name: row.get(1).ok(),
                notes: row.get(2).ok(),
                rating: row.get(3).unwrap_or(0),
                play_count: row.get(4).unwrap_or(0),
                hidden: row.get::<_, i32>(5).unwrap_or(0) != 0,
                subcategory: row.get(6).ok(),
            }),
        ).ok()
    }

    /// Set subcategory for a preset (folder hierarchy within category)
    pub fn set_preset_subcategory(&self, file_path: &str, subcategory: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preset_metadata (file_path, subcategory, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET subcategory = excluded.subcategory, updated_at = excluded.updated_at",
            params![file_path, subcategory],
        )?;
        Ok(())
    }

    /// Set hidden status for a preset
    pub fn set_preset_hidden(&self, file_path: &str, hidden: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preset_metadata (file_path, hidden, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET hidden = excluded.hidden, updated_at = excluded.updated_at",
            params![file_path, if hidden { 1 } else { 0 }],
        )?;
        Ok(())
    }

    /// Check if a preset is hidden
    pub fn is_preset_hidden(&self, file_path: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT hidden FROM preset_metadata WHERE file_path = ?",
            [file_path],
            |row| row.get::<_, i32>(0),
        ).unwrap_or(0) != 0
    }

    /// Get all preset metadata (for caching)
    pub fn get_all_preset_metadata(&self) -> Vec<PresetMetadata> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT file_path, custom_name, notes, rating, play_count, hidden, subcategory FROM preset_metadata"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map([], |row| {
            Ok(PresetMetadata {
                file_path: row.get(0)?,
                custom_name: row.get(1).ok(),
                notes: row.get(2).ok(),
                rating: row.get(3).unwrap_or(0),
                play_count: row.get(4).unwrap_or(0),
                hidden: row.get::<_, i32>(5).unwrap_or(0) != 0,
                subcategory: row.get(6).ok(),
            })
        });

        match rows {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ========== Preset-Video Associations ==========

    /// Associate a video with a preset
    pub fn add_preset_video(&self, preset_path: &str, video_path: &str, priority: i32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO preset_videos (preset_path, video_path, priority)
             VALUES (?, ?, ?)",
            params![preset_path, video_path, priority],
        )?;
        log::info!("[DB] Associated video {} with preset {}", video_path, preset_path);
        Ok(())
    }

    /// Remove a video association from a preset
    pub fn remove_preset_video(&self, preset_path: &str, video_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM preset_videos WHERE preset_path = ? AND video_path = ?",
            params![preset_path, video_path],
        )?;
        Ok(())
    }

    /// Get all videos associated with a preset
    pub fn get_preset_videos(&self, preset_path: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT video_path FROM preset_videos WHERE preset_path = ? ORDER BY priority DESC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([preset_path], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Get presets associated with a video
    pub fn get_video_presets(&self, video_path: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT preset_path FROM preset_videos WHERE video_path = ? ORDER BY priority DESC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([video_path], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    // ========== Preset-Track Associations (Themes) ==========

    /// Associate a track/song with a preset
    pub fn add_preset_track(&self, preset_path: &str, track_name: &str, artist: Option<&str>, album: Option<&str>, priority: i32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO preset_tracks (preset_path, track_name, artist, album, priority)
             VALUES (?, ?, ?, ?, ?)",
            params![preset_path, track_name, artist, album, priority],
        )?;
        log::info!("[DB] Associated track '{}' by '{}' with preset {}", track_name, artist.unwrap_or("Unknown"), preset_path);
        Ok(())
    }

    /// Remove a track association from a preset
    pub fn remove_preset_track(&self, preset_path: &str, track_name: &str, artist: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM preset_tracks WHERE preset_path = ? AND track_name = ? AND (artist = ? OR (artist IS NULL AND ? IS NULL))",
            params![preset_path, track_name, artist, artist],
        )?;
        Ok(())
    }

    /// Get all tracks associated with a preset
    pub fn get_preset_tracks(&self, preset_path: &str) -> Vec<TrackInfo> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT track_name, artist, album FROM preset_tracks WHERE preset_path = ? ORDER BY priority DESC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([preset_path], |row| Ok(TrackInfo {
            name: row.get(0)?,
            artist: row.get(1).ok(),
            album: row.get(2).ok(),
        }))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Find presets associated with a track (for auto-switching)
    pub fn find_presets_for_track(&self, track_name: &str, artist: Option<&str>) -> Vec<String> {
        let conn = self.conn.lock().unwrap();

        // Try exact match first, then fall back to track name only
        let mut stmt = match conn.prepare(
            "SELECT preset_path FROM preset_tracks
             WHERE track_name = ? AND (artist = ? OR artist IS NULL)
             ORDER BY priority DESC, CASE WHEN artist = ? THEN 0 ELSE 1 END"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map(params![track_name, artist, artist], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Find presets associated with a track using fuzzy contains matching.
    /// Priority:
    /// 1. Track AND artist both match
    /// 2. Track name matches
    /// 3. Artist matches
    pub fn find_presets_for_track_fuzzy(
        &self,
        track_name: &str,
        artist: Option<&str>,
    ) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let track_lower = track_name.to_lowercase();
        let artist_lower = artist.map(|a| a.to_lowercase());

        let mut stmt = match conn.prepare(
            "SELECT preset_path, track_name, artist FROM preset_tracks ORDER BY priority DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows: Vec<(String, String, Option<String>)> = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        }) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(_) => return Vec::new(),
        };

        let mut matches = Vec::new();
        let mut push_unique = |path: &String| {
            if !matches.iter().any(|p| p == path) {
                matches.push(path.clone());
            }
        };

        if let Some(ref artist_search) = artist_lower {
            for (preset_path, db_track, db_artist) in &rows {
                let db_track_lower = db_track.to_lowercase();
                let db_artist_lower = db_artist.as_ref().map(|a| a.to_lowercase());
                let track_matches = db_track_lower.contains(&track_lower)
                    || track_lower.contains(&db_track_lower);
                let artist_matches = db_artist_lower
                    .as_ref()
                    .map(|da| da.contains(artist_search) || artist_search.contains(da))
                    .unwrap_or(false);
                if track_matches && artist_matches {
                    push_unique(preset_path);
                }
            }
        }

        for (preset_path, db_track, _) in &rows {
            let db_track_lower = db_track.to_lowercase();
            if db_track_lower.contains(&track_lower) || track_lower.contains(&db_track_lower) {
                push_unique(preset_path);
            }
        }

        if let Some(ref artist_search) = artist_lower {
            for (preset_path, _, db_artist) in &rows {
                if let Some(ref da) = db_artist {
                    let da_lower = da.to_lowercase();
                    if da_lower.contains(artist_search) || artist_search.contains(&da_lower) {
                        push_unique(preset_path);
                    }
                }
            }
        }

        matches
    }

    // ========== Video (Compel) Metadata ==========

    /// Set custom name for a video
    pub fn set_video_custom_name(&self, file_path: &str, custom_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO video_metadata (file_path, custom_name, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET custom_name = excluded.custom_name, updated_at = excluded.updated_at",
            params![file_path, custom_name],
        )?;
        Ok(())
    }

    /// Set hidden status for a video
    pub fn set_video_hidden(&self, file_path: &str, hidden: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO video_metadata (file_path, hidden, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET hidden = excluded.hidden, updated_at = excluded.updated_at",
            params![file_path, if hidden { 1 } else { 0 }],
        )?;
        Ok(())
    }

    /// Get video metadata
    pub fn get_video_metadata(&self, file_path: &str) -> Option<VideoMetadata> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT file_path, custom_name, notes, hidden, COALESCE(favorite, 0), thumbnail_path, COALESCE(thumbnail_generated, 0) FROM video_metadata WHERE file_path = ?",
            [file_path],
            |row| Ok(VideoMetadata {
                file_path: row.get(0)?,
                custom_name: row.get(1).ok(),
                notes: row.get(2).ok(),
                hidden: row.get::<_, i32>(3).unwrap_or(0) != 0,
                favorite: row.get::<_, i32>(4).unwrap_or(0) != 0,
                thumbnail_path: row.get(5).ok().flatten(),
                thumbnail_generated: row.get::<_, i32>(6).unwrap_or(0) != 0,
            }),
        ).ok()
    }

    /// Get all video metadata (for caching)
    pub fn get_all_video_metadata(&self) -> Vec<VideoMetadata> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT file_path, custom_name, notes, hidden, COALESCE(favorite, 0), thumbnail_path, COALESCE(thumbnail_generated, 0) FROM video_metadata"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map([], |row| {
            Ok(VideoMetadata {
                file_path: row.get(0)?,
                custom_name: row.get(1).ok(),
                notes: row.get(2).ok(),
                hidden: row.get::<_, i32>(3).unwrap_or(0) != 0,
                favorite: row.get::<_, i32>(4).unwrap_or(0) != 0,
                thumbnail_path: row.get(5).ok().flatten(),
                thumbnail_generated: row.get::<_, i32>(6).unwrap_or(0) != 0,
            })
        });

        match rows {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Set favorite status for a video
    pub fn set_video_favorite(&self, file_path: &str, favorite: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO video_metadata (file_path, favorite, updated_at)
             VALUES (?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET favorite = excluded.favorite, updated_at = excluded.updated_at",
            params![file_path, if favorite { 1 } else { 0 }],
        )?;
        Ok(())
    }

    /// Get all favorite video paths
    pub fn get_video_favorites(&self) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT file_path FROM video_metadata WHERE favorite = 1"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    // ========== Video Thumbnails ==========

    /// Set thumbnail path for a video
    pub fn set_video_thumbnail(&self, file_path: &str, thumbnail_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO video_metadata (file_path, thumbnail_path, thumbnail_generated, updated_at)
             VALUES (?, ?, 1, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET
                thumbnail_path = excluded.thumbnail_path,
                thumbnail_generated = 1,
                updated_at = excluded.updated_at",
            params![file_path, thumbnail_path],
        )?;
        Ok(())
    }

    /// Get thumbnail path for a video
    pub fn get_video_thumbnail(&self, file_path: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT thumbnail_path FROM video_metadata WHERE file_path = ? AND thumbnail_generated = 1",
            [file_path],
            |row| row.get(0),
        ).ok().flatten()
    }

    /// Get videos that need thumbnails generated
    pub fn get_videos_needing_thumbnails(&self) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT file_path FROM video_metadata WHERE thumbnail_generated = 0 OR thumbnail_generated IS NULL"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Mark that a video needs a thumbnail (create entry if doesn't exist)
    pub fn mark_video_needs_thumbnail(&self, file_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO video_metadata (file_path, thumbnail_generated, updated_at)
             VALUES (?, 0, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET
                thumbnail_generated = CASE WHEN thumbnail_path IS NULL THEN 0 ELSE thumbnail_generated END",
            params![file_path],
        )?;
        Ok(())
    }

    /// Add a video to recent videos list
    pub fn add_recent_video(&self, file_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Insert or update played_at timestamp
        conn.execute(
            "INSERT INTO recent_videos (file_path, played_at)
             VALUES (?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET played_at = strftime('%s', 'now')",
            [file_path],
        )?;
        // Keep only the 7 most recent
        conn.execute(
            "DELETE FROM recent_videos WHERE id NOT IN (
                SELECT id FROM recent_videos ORDER BY played_at DESC LIMIT 7
            )",
            [],
        )?;
        Ok(())
    }

    /// Get recent videos (most recent first, limited to 7)
    pub fn get_recent_videos(&self) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT file_path FROM recent_videos ORDER BY played_at DESC LIMIT 7"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    // ========== Video-Track Associations ==========

    /// Associate a track with a video
    pub fn add_video_track(&self, video_path: &str, track_name: &str, artist: Option<&str>, album: Option<&str>, priority: i32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO video_tracks (video_path, track_name, artist, album, priority)
             VALUES (?, ?, ?, ?, ?)",
            params![video_path, track_name, artist, album, priority],
        )?;
        log::info!("[DB] Associated track '{}' with video {}", track_name, video_path);
        Ok(())
    }

    /// Remove a track association from a video
    pub fn remove_video_track(&self, video_path: &str, track_name: &str, artist: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM video_tracks WHERE video_path = ? AND track_name = ? AND (artist = ? OR (artist IS NULL AND ? IS NULL))",
            params![video_path, track_name, artist, artist],
        )?;
        Ok(())
    }

    /// Get all tracks associated with a video
    pub fn get_video_tracks(&self, video_path: &str) -> Vec<TrackInfo> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT track_name, artist, album FROM video_tracks WHERE video_path = ? ORDER BY priority DESC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([video_path], |row| Ok(TrackInfo {
            name: row.get(0)?,
            artist: row.get(1).ok(),
            album: row.get(2).ok(),
        }))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Find videos associated with a track (for auto-switching)
    pub fn find_videos_for_track(&self, track_name: &str, artist: Option<&str>) -> Vec<String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = match conn.prepare(
            "SELECT video_path FROM video_tracks
             WHERE track_name = ? AND (artist = ? OR artist IS NULL)
             ORDER BY priority DESC, CASE WHEN artist = ? THEN 0 ELSE 1 END"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map(params![track_name, artist, artist], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Find a video for a track using "contains" matching with priority:
    /// 1. Track AND Artist both match (contains, case-insensitive)
    /// 2. Track name matches only
    /// 3. Artist matches only
    /// Returns the first matching video path, if any
    pub fn find_video_for_track_fuzzy(&self, track_name: &str, artist: Option<&str>) -> Option<String> {
        let conn = self.conn.lock().unwrap();

        // Normalize inputs for case-insensitive matching
        let track_lower = track_name.to_lowercase();
        let artist_lower = artist.map(|a| a.to_lowercase());

        // Get all video tracks
        let mut stmt = match conn.prepare(
            "SELECT video_path, track_name, artist FROM video_tracks ORDER BY priority DESC"
        ) {
            Ok(s) => s,
            Err(_) => return None,
        };

        let rows: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .ok()?
            .filter_map(|r| r.ok())
            .collect();

        // Priority 1: Track AND Artist both match
        if let Some(ref artist_search) = artist_lower {
            for (video_path, db_track, db_artist) in &rows {
                let db_track_lower = db_track.to_lowercase();
                let db_artist_lower = db_artist.as_ref().map(|a| a.to_lowercase());

                // Check if track contains or is contained by search term
                let track_matches = db_track_lower.contains(&track_lower)
                    || track_lower.contains(&db_track_lower);

                // Check if artist contains or is contained by search term
                let artist_matches = db_artist_lower.as_ref().map(|da| {
                    da.contains(artist_search) || artist_search.contains(da)
                }).unwrap_or(false);

                if track_matches && artist_matches {
                    return Some(video_path.clone());
                }
            }
        }

        // Priority 2: Track name matches only
        for (video_path, db_track, _) in &rows {
            let db_track_lower = db_track.to_lowercase();
            if db_track_lower.contains(&track_lower) || track_lower.contains(&db_track_lower) {
                return Some(video_path.clone());
            }
        }

        // Priority 3: Artist matches only
        if let Some(ref artist_search) = artist_lower {
            for (video_path, _, db_artist) in &rows {
                if let Some(ref da) = db_artist {
                    let da_lower = da.to_lowercase();
                    if da_lower.contains(artist_search) || artist_search.contains(&da_lower) {
                        return Some(video_path.clone());
                    }
                }
            }
        }

        None
    }

    // ========== Camera Settings ==========

    /// Load all camera settings from database
    /// Returns: HashMap<camera_index, (visibility_step, enabled)>
    pub fn load_camera_settings(&self) -> std::collections::HashMap<u32, (u8, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT camera_index, visibility_step, enabled FROM camera_settings"
        ) {
            Ok(s) => s,
            Err(_) => return std::collections::HashMap::new(),
        };

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, i32>(1)? as u8,
                row.get::<_, i32>(2)? != 0,
            ))
        });

        match rows {
            Ok(iter) => iter
                .filter_map(|r| r.ok())
                .map(|(idx, vis, en)| (idx, (vis, en)))
                .collect(),
            Err(_) => std::collections::HashMap::new(),
        }
    }

    /// Save a single camera's settings
    pub fn save_camera_setting(&self, camera_index: u32, visibility_step: u8, enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO camera_settings (camera_index, visibility_step, enabled, updated_at)
             VALUES (?, ?, ?, strftime('%s', 'now'))",
            params![camera_index as i32, visibility_step as i32, if enabled { 1 } else { 0 }],
        )?;
        Ok(())
    }

    /// Delete camera settings for a specific camera
    pub fn delete_camera_setting(&self, camera_index: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM camera_settings WHERE camera_index = ?",
            params![camera_index as i32],
        )?;
        Ok(())
    }

    // ========== Camera Media Settings (Background Layers) ==========

    /// Load all camera media settings from database
    /// Returns: HashMap<camera_index, (visibility_step, enabled)>
    pub fn load_camera_media_settings(&self) -> std::collections::HashMap<u32, (u8, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT camera_index, visibility_step, enabled FROM camera_media_settings"
        ) {
            Ok(s) => s,
            Err(_) => return std::collections::HashMap::new(),
        };

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, i32>(1)? as u8,
                row.get::<_, i32>(2)? != 0,
            ))
        });

        match rows {
            Ok(rows) => rows
                .filter_map(|r| r.ok())
                .map(|(idx, vis, en)| (idx, (vis, en)))
                .collect(),
            Err(_) => std::collections::HashMap::new(),
        }
    }

    /// Save a single camera's media settings
    pub fn save_camera_media_setting(&self, camera_index: u32, visibility_step: u8, enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO camera_media_settings (camera_index, visibility_step, enabled, updated_at)
             VALUES (?, ?, ?, strftime('%s', 'now'))",
            params![camera_index as i32, visibility_step as i32, if enabled { 1 } else { 0 }],
        )?;
        Ok(())
    }

    /// Delete camera media settings for a specific camera
    pub fn delete_camera_media_setting(&self, camera_index: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM camera_media_settings WHERE camera_index = ?",
            params![camera_index as i32],
        )?;
        Ok(())
    }

    // ========== MIDI Mappings ==========

    /// Save a MIDI mapping
    pub fn save_midi_mapping(&self, mapping: &MidiMappingRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO midi_mappings
             (device_name, channel, msg_type, msg_index, target, min_value, max_value, inverted)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                mapping.device_name,
                mapping.channel,
                mapping.msg_type,
                mapping.msg_index,
                mapping.target,
                mapping.min_value,
                mapping.max_value,
                mapping.inverted as i32
            ],
        )?;
        log::info!("[DB] Saved MIDI mapping: {} -> {}", mapping.device_name, mapping.target);
        Ok(())
    }

    /// Get all MIDI mappings
    pub fn get_midi_mappings(&self) -> Vec<MidiMappingRow> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT device_name, channel, msg_type, msg_index, target, min_value, max_value, inverted
             FROM midi_mappings"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| Ok(MidiMappingRow {
            device_name: row.get(0)?,
            channel: row.get(1)?,
            msg_type: row.get(2)?,
            msg_index: row.get(3)?,
            target: row.get(4)?,
            min_value: row.get(5)?,
            max_value: row.get(6)?,
            inverted: row.get::<_, i32>(7)? != 0,
        }))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Delete a MIDI mapping
    pub fn delete_midi_mapping(&self, device_name: &str, channel: u8, msg_type: &str, msg_index: u8) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM midi_mappings WHERE device_name = ? AND channel = ? AND msg_type = ? AND msg_index = ?",
            params![device_name, channel, msg_type, msg_index],
        )?;
        Ok(())
    }

    /// Delete all MIDI mappings
    pub fn clear_midi_mappings(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM midi_mappings", [])?;
        Ok(())
    }

    // ========== MIDI Preset Bindings ==========

    /// Save a MIDI preset binding
    pub fn save_midi_preset_binding(&self, binding: &MidiPresetBindingRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO midi_preset_bindings
             (device_name, channel, note, preset_id, name)
             VALUES (?, ?, ?, ?, ?)",
            params![
                binding.device_name,
                binding.channel as i32,
                binding.note as i32,
                binding.preset_id,
                binding.name
            ],
        )?;
        log::info!("[DB] Saved MIDI preset binding: Note {} -> {}", binding.note, binding.preset_id);
        Ok(())
    }

    /// Get all MIDI preset bindings
    pub fn get_midi_preset_bindings(&self) -> Vec<MidiPresetBindingRow> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT device_name, channel, note, preset_id, name
             FROM midi_preset_bindings
             ORDER BY note ASC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| Ok(MidiPresetBindingRow {
            device_name: row.get(0)?,
            channel: row.get::<_, i32>(1)? as u8,
            note: row.get::<_, i32>(2)? as u8,
            preset_id: row.get(3)?,
            name: row.get(4)?,
        }))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Delete a MIDI preset binding
    pub fn delete_midi_preset_binding(&self, device_name: &str, channel: u8, note: u8) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM midi_preset_bindings WHERE device_name = ? AND channel = ? AND note = ?",
            params![device_name, channel as i32, note as i32],
        )?;
        Ok(())
    }

    /// Delete all MIDI preset bindings
    pub fn clear_midi_preset_bindings(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM midi_preset_bindings", [])?;
        Ok(())
    }

    // ========== OSC Configuration ==========

    /// Load OSC configuration from settings
    pub fn load_osc_config(&self) -> crate::osc::OscConfig {
        crate::osc::OscConfig {
            target_ip: self.get_setting("osc_target_ip").unwrap_or_else(|| "compellerdmx.local".to_string()),
            target_port: self.get_setting("osc_target_port")
                .and_then(|s| s.parse().ok())
                .unwrap_or(7672),
            target_profile: crate::osc::TargetProfile::from_str(
                &self.get_setting("osc_target_profile").unwrap_or_else(|| "Generic".to_string())
            ),
            send_continuous: self.get_bool("osc_send_continuous", true),
            zone_cue_address: self.get_setting("osc_zone_cue_address")
                .unwrap_or_else(|| "/show/cue/index".to_string()),
            enable_kick_strobe: self.get_bool("osc_enable_kick_strobe", true),
            enable_strobe_hold: self.get_bool("osc_enable_strobe_hold", true),
            strobe_cooldown_ms: self.get_setting("osc_strobe_cooldown_ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(80),
            blackout_delay_secs: self.get_float("osc_blackout_delay_secs", 2.0),
            zone_cue_overrides: Vec::new(), // TODO: Add zone override persistence if needed
            trigger_mappings: Vec::new(),   // TODO: Add trigger mapping persistence if needed

            // Palette names
            palette_ice_name: self.get_setting("osc_palette_ice_name")
                .unwrap_or_else(|| "All White".to_string()),
            palette_cool_a_name: self.get_setting("osc_palette_cool_a_name")
                .unwrap_or_else(|| "Neon Retro".to_string()),
            palette_cool_b_name: self.get_setting("osc_palette_cool_b_name")
                .unwrap_or_else(|| "Vivid Saturated".to_string()),

            // Palette indices (for DMX software)
            palette_ice_index: self.get_setting("osc_palette_ice_index")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            palette_cool_a_index: self.get_setting("osc_palette_cool_a_index")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            palette_cool_b_index: self.get_setting("osc_palette_cool_b_index")
                .and_then(|s| s.parse().ok())
                .unwrap_or(2),

            // Pattern indices (zone → pattern)
            pattern_silent: self.get_setting("osc_pattern_silent")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            pattern_still: self.get_setting("osc_pattern_still")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            pattern_low: self.get_setting("osc_pattern_low")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            pattern_mid: self.get_setting("osc_pattern_mid")
                .and_then(|s| s.parse().ok())
                .unwrap_or(2),
            pattern_high: self.get_setting("osc_pattern_high")
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),

            // Mover speed range
            mover_speed_min: self.get_float("osc_mover_speed_min", 0.2),
            mover_speed_max: self.get_float("osc_mover_speed_max", 0.8),

            // FX palette
            enable_fx_palette: self.get_bool("osc_enable_fx_palette", true),

            // Genre profile - load from category/subcategory
            genre_profile: {
                let category = self.get_setting("osc_genre_category")
                    .unwrap_or_else(|| "Electronic".to_string());
                let subcategory = self.get_setting("osc_genre_subcategory")
                    .unwrap_or_else(|| "Tech House".to_string());
                crate::emerge::GenreProfile::from_genre(&category, &subcategory)
            },

            // Reactive lighting features
            enable_polarity_flash: self.get_bool("osc_enable_polarity_flash", true),
            enable_dominant_accent: self.get_bool("osc_enable_dominant_accent", true),
            enable_tonality_palettes: self.get_bool("osc_enable_tonality_palettes", true),
            enable_energy_slope: self.get_bool("osc_enable_energy_slope", true),

            // Polarity flash settings
            polarity_flash_duration_ms: self.get_setting("osc_polarity_flash_duration_ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(150),
            polarity_flash_cooldown_ms: self.get_setting("osc_polarity_flash_cooldown_ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(500),

            // Dominant accent settings
            dominant_flash_duration_ms: self.get_setting("osc_dominant_flash_duration_ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(80),
            dominant_flash_cooldown_ms: self.get_setting("osc_dominant_flash_cooldown_ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(250),

            // Reactivity control
            reactivity: self.get_float("osc_reactivity", 0.5),

            // Global brightness
            global_brightness: self.get_float("osc_global_brightness", 1.0),

            // UV accent level
            uv_level: self.get_float("osc_uv_level", 0.35),

            // Buildup macro master arm
            enable_buildup_macro: self.get_bool("osc_enable_buildup_macro", true),

            // Lighting style posture (unknown/missing → Auto)
            lighting_style: crate::emerge::LightingStyle::from_str(
                &self.get_setting("osc_lighting_style").unwrap_or_default(),
            ),
        }
    }

    /// Save OSC configuration to settings
    pub fn save_osc_config(&self, config: &crate::osc::OscConfig) -> Result<()> {
        self.set_setting("osc_target_ip", &config.target_ip)?;
        self.set_setting("osc_target_port", &config.target_port.to_string())?;
        self.set_setting("osc_target_profile", config.target_profile.as_str())?;
        self.set_bool("osc_send_continuous", config.send_continuous)?;
        self.set_setting("osc_zone_cue_address", &config.zone_cue_address)?;
        self.set_bool("osc_enable_kick_strobe", config.enable_kick_strobe)?;
        self.set_bool("osc_enable_strobe_hold", config.enable_strobe_hold)?;
        self.set_setting("osc_strobe_cooldown_ms", &config.strobe_cooldown_ms.to_string())?;
        self.set_float("osc_blackout_delay_secs", config.blackout_delay_secs)?;

        // Palette names
        self.set_setting("osc_palette_ice_name", &config.palette_ice_name)?;
        self.set_setting("osc_palette_cool_a_name", &config.palette_cool_a_name)?;
        self.set_setting("osc_palette_cool_b_name", &config.palette_cool_b_name)?;

        // Palette indices
        self.set_setting("osc_palette_ice_index", &config.palette_ice_index.to_string())?;
        self.set_setting("osc_palette_cool_a_index", &config.palette_cool_a_index.to_string())?;
        self.set_setting("osc_palette_cool_b_index", &config.palette_cool_b_index.to_string())?;

        // Pattern indices
        self.set_setting("osc_pattern_silent", &config.pattern_silent.to_string())?;
        self.set_setting("osc_pattern_still", &config.pattern_still.to_string())?;
        self.set_setting("osc_pattern_low", &config.pattern_low.to_string())?;
        self.set_setting("osc_pattern_mid", &config.pattern_mid.to_string())?;
        self.set_setting("osc_pattern_high", &config.pattern_high.to_string())?;

        // Mover speed range
        self.set_float("osc_mover_speed_min", config.mover_speed_min)?;
        self.set_float("osc_mover_speed_max", config.mover_speed_max)?;

        // FX palette
        self.set_bool("osc_enable_fx_palette", config.enable_fx_palette)?;

        // Genre profile
        self.set_setting("osc_genre_category", &config.genre_profile.category)?;
        self.set_setting("osc_genre_subcategory", &config.genre_profile.subcategory)?;

        // Reactive lighting features
        self.set_bool("osc_enable_polarity_flash", config.enable_polarity_flash)?;
        self.set_bool("osc_enable_dominant_accent", config.enable_dominant_accent)?;
        self.set_bool("osc_enable_tonality_palettes", config.enable_tonality_palettes)?;
        self.set_bool("osc_enable_energy_slope", config.enable_energy_slope)?;

        // Polarity flash settings
        self.set_setting("osc_polarity_flash_duration_ms", &config.polarity_flash_duration_ms.to_string())?;
        self.set_setting("osc_polarity_flash_cooldown_ms", &config.polarity_flash_cooldown_ms.to_string())?;

        // Dominant accent settings
        self.set_setting("osc_dominant_flash_duration_ms", &config.dominant_flash_duration_ms.to_string())?;
        self.set_setting("osc_dominant_flash_cooldown_ms", &config.dominant_flash_cooldown_ms.to_string())?;

        // Reactivity control
        self.set_float("osc_reactivity", config.reactivity)?;

        // Global brightness
        self.set_float("osc_global_brightness", config.global_brightness)?;

        // UV accent level
        self.set_float("osc_uv_level", config.uv_level)?;

        // Buildup macro master arm
        self.set_bool("osc_enable_buildup_macro", config.enable_buildup_macro)?;

        // Lighting style posture
        self.set_setting("osc_lighting_style", config.lighting_style.as_str())?;

        Ok(())
    }

    /// Check if OSC is enabled
    pub fn is_osc_enabled(&self) -> bool {
        self.get_bool("osc_enabled", false)
    }

    /// Set OSC enabled state
    pub fn set_osc_enabled(&self, enabled: bool) -> Result<()> {
        self.set_bool("osc_enabled", enabled)
    }

    // ========== Hue Persistence ==========

    /// Load Hue config from database
    pub fn load_hue_config(&self) -> crate::hue::HueConfig {
        let protocol_version = match self.get_setting("hue_protocol_version").as_deref() {
            Some("v1") => crate::hue::HueProtocolVersion::V1,
            Some("v2") => crate::hue::HueProtocolVersion::V2,
            _ => crate::hue::HueProtocolVersion::default(),
        };
        let area_id = self.get_setting("hue_area_id").unwrap_or_default();
        // Validate area ID format matches protocol: v2 requires UUID (contains dashes),
        // v1 uses numeric group IDs. Clear stale IDs from a previous protocol switch.
        let area_id_valid = match protocol_version {
            crate::hue::HueProtocolVersion::V2 => area_id.is_empty() || area_id.contains('-'),
            crate::hue::HueProtocolVersion::V1 => area_id.is_empty() || !area_id.contains('-'),
        };
        let (area_id, light_assignments) = if area_id_valid {
            let la = self.get_setting("hue_light_assignments")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            (area_id, la)
        } else {
            log::warn!("[Hue] Clearing stale area_id '{}' — incompatible with {:?}", area_id, protocol_version);
            (String::new(), Vec::new())
        };

        let mapping_mode = match self.get_setting("hue_mapping_mode").as_deref() {
            Some("multiband") => crate::hue::HueMappingMode::MultiBand,
            _ => crate::hue::HueMappingMode::Reactive,
        };

        crate::hue::HueConfig {
            enabled: self.get_bool("hue_enabled", false),
            bridge_ip: self.get_setting("hue_bridge_ip").unwrap_or_default(),
            username: self.get_setting("hue_username").unwrap_or_default(),
            clientkey: self.get_setting("hue_clientkey").unwrap_or_default(),
            entertainment_area_id: area_id,
            light_assignments,
            protocol_version,
            application_id: self.get_setting("hue_application_id").unwrap_or_default(),
            mapping_mode,
        }
    }

    /// Save Hue config to database
    pub fn save_hue_config(&self, config: &crate::hue::HueConfig) -> Result<()> {
        self.set_bool("hue_enabled", config.enabled)?;
        self.set_setting("hue_bridge_ip", &config.bridge_ip)?;
        self.set_setting("hue_username", &config.username)?;
        self.set_setting("hue_clientkey", &config.clientkey)?;
        self.set_setting("hue_area_id", &config.entertainment_area_id)?;
        if let Ok(json) = serde_json::to_string(&config.light_assignments) {
            self.set_setting("hue_light_assignments", &json)?;
        }
        let protocol_str = match config.protocol_version {
            crate::hue::HueProtocolVersion::V1 => "v1",
            crate::hue::HueProtocolVersion::V2 => "v2",
        };
        self.set_setting("hue_protocol_version", protocol_str)?;
        if !config.application_id.is_empty() {
            self.set_setting("hue_application_id", &config.application_id)?;
        }
        let mapping_mode_str = match config.mapping_mode {
            crate::hue::HueMappingMode::Reactive => "reactive",
            crate::hue::HueMappingMode::MultiBand => "multiband",
        };
        self.set_setting("hue_mapping_mode", mapping_mode_str)?;
        Ok(())
    }

    // ========== WLED Persistence ==========

    /// Load WLED config from database
    pub fn load_wled_config(&self) -> crate::wled::WledConfig {
        crate::wled::WledConfig {
            enabled: self.get_bool("wled_enabled", false),
            strips: self.get_setting("wled_strips")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
        }
    }

    /// Save WLED config to database
    pub fn save_wled_config(&self, config: &crate::wled::WledConfig) -> Result<()> {
        self.set_bool("wled_enabled", config.enabled)?;
        if let Ok(json) = serde_json::to_string(&config.strips) {
            self.set_setting("wled_strips", &json)?;
        }
        Ok(())
    }

    // ========== Audio Source Persistence ==========

    /// Get saved audio source as serialized string
    /// Format: "system" | "app:bundle_id:name" | "input:device_name" | "mic"
    pub fn get_audio_source(&self) -> Option<String> {
        self.get_setting("audio_source")
    }

    /// Set audio source as serialized string
    pub fn set_audio_source(&self, source: &str) -> Result<()> {
        self.set_setting("audio_source", source)
    }

    // ========== DMX Output Persistence ==========

    /// Saved DMX output backend key ("off" | "artnet" | "ola" | "usb").
    pub fn get_dmx_backend(&self) -> Option<String> {
        self.get_setting("dmx_backend")
    }

    /// Persist the chosen DMX output backend key.
    pub fn set_dmx_backend(&self, backend: &str) -> Result<()> {
        self.set_setting("dmx_backend", backend)
    }

    /// Saved USB DMX serial port path (e.g. `/dev/cu.usbserial-XXXX`).
    pub fn get_dmx_usb_port(&self) -> Option<String> {
        self.get_setting("dmx_usb_port")
    }

    /// Persist the USB DMX serial port path.
    pub fn set_dmx_usb_port(&self, port: &str) -> Result<()> {
        self.set_setting("dmx_usb_port", port)
    }

    /// Saved Art-Net target socket (e.g. `192.168.1.50:6454`).
    pub fn get_dmx_artnet_target(&self) -> Option<String> {
        self.get_setting("dmx_artnet_target")
    }

    /// Persist the Art-Net target socket.
    pub fn set_dmx_artnet_target(&self, target: &str) -> Result<()> {
        self.set_setting("dmx_artnet_target", target)
    }

    /// Saved OLA endpoint (`host:port`, e.g. `127.0.0.1:9010`).
    pub fn get_dmx_ola_endpoint(&self) -> Option<String> {
        self.get_setting("dmx_ola_endpoint")
    }

    /// Persist the OLA endpoint (`host:port`).
    pub fn set_dmx_ola_endpoint(&self, endpoint: &str) -> Result<()> {
        self.set_setting("dmx_ola_endpoint", endpoint)
    }

    /// Get saved Twitch stream key
    pub fn get_twitch_stream_key(&self) -> Option<String> {
        self.get_setting("twitch_stream_key")
    }

    /// Save Twitch stream key
    pub fn set_twitch_stream_key(&self, key: &str) -> Result<()> {
        self.set_setting("twitch_stream_key", key)
    }

    /// Get saved Twitch channel name
    pub fn get_twitch_channel(&self) -> Option<String> {
        self.get_setting("twitch_channel")
    }

    /// Save Twitch channel name
    pub fn set_twitch_channel(&self, channel: &str) -> Result<()> {
        self.set_setting("twitch_channel", channel)
    }

    /// Get saved Twitch stream title
    pub fn get_twitch_stream_title(&self) -> Option<String> {
        self.get_setting("twitch_stream_title")
    }

    /// Save Twitch stream title
    pub fn set_twitch_stream_title(&self, title: &str) -> Result<()> {
        self.set_setting("twitch_stream_title", title)
    }

    /// Get saved Twitch stream category
    pub fn get_twitch_stream_category(&self) -> Option<String> {
        self.get_setting("twitch_stream_category")
    }

    /// Save Twitch stream category
    pub fn set_twitch_stream_category(&self, category: &str) -> Result<()> {
        self.set_setting("twitch_stream_category", category)
    }

    /// Get saved Twitch stream tags
    pub fn get_twitch_stream_tags(&self) -> Option<String> {
        self.get_setting("twitch_stream_tags")
    }

    /// Save Twitch stream tags
    pub fn set_twitch_stream_tags(&self, tags: &str) -> Result<()> {
        self.set_setting("twitch_stream_tags", tags)
    }

    /// Get saved Kick ingest stream URL (from the Kick Creator Dashboard)
    pub fn get_kick_stream_url(&self) -> Option<String> {
        self.get_setting("kick_stream_url")
    }

    /// Save Kick ingest stream URL
    pub fn set_kick_stream_url(&self, url: &str) -> Result<()> {
        self.set_setting("kick_stream_url", url)
    }

    /// Get saved Kick stream key
    pub fn get_kick_stream_key(&self) -> Option<String> {
        self.get_setting("kick_stream_key")
    }

    /// Save Kick stream key
    pub fn set_kick_stream_key(&self, key: &str) -> Result<()> {
        self.set_setting("kick_stream_key", key)
    }

    /// Get saved Kick stream title
    pub fn get_kick_stream_title(&self) -> Option<String> {
        self.get_setting("kick_stream_title")
    }

    /// Save Kick stream title
    pub fn set_kick_stream_title(&self, title: &str) -> Result<()> {
        self.set_setting("kick_stream_title", title)
    }
}

/// MIDI mapping row for database storage
#[derive(Debug, Clone)]
pub struct MidiMappingRow {
    pub device_name: String,
    pub channel: u8,
    pub msg_type: String,
    pub msg_index: u8,
    pub target: String,
    pub min_value: f32,
    pub max_value: f32,
    pub inverted: bool,
}

/// MIDI preset binding row for database storage
#[derive(Debug, Clone)]
pub struct MidiPresetBindingRow {
    /// Device name (empty = any device)
    pub device_name: String,
    /// MIDI channel (255 = any channel)
    pub channel: u8,
    /// Note number
    pub note: u8,
    /// Preset ID to load
    pub preset_id: String,
    /// Optional custom name
    pub name: Option<String>,
}

impl MidiPresetBindingRow {
    /// Create from a MidiPresetBinding
    pub fn from_binding(binding: &crate::midi::MidiPresetBinding) -> Self {
        Self {
            device_name: binding.device_name.clone(),
            channel: binding.channel,
            note: binding.note,
            preset_id: binding.preset_id.clone(),
            name: binding.name.clone(),
        }
    }

    /// Convert to MidiPresetBinding
    pub fn to_binding(&self) -> crate::midi::MidiPresetBinding {
        crate::midi::MidiPresetBinding {
            device_name: self.device_name.clone(),
            channel: self.channel,
            note: self.note,
            preset_id: self.preset_id.clone(),
            name: self.name.clone(),
        }
    }
}

/// Video (Compel) metadata structure
#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub file_path: String,
    pub custom_name: Option<String>,
    pub notes: Option<String>,
    pub hidden: bool,
    pub favorite: bool,
    pub thumbnail_path: Option<String>,
    pub thumbnail_generated: bool,
}

impl VideoMetadata {
    /// Returns custom_name if set, otherwise extracts name from file_path
    pub fn display_name(&self) -> String {
        if let Some(ref name) = self.custom_name {
            name.clone()
        } else {
            std::path::Path::new(&self.file_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string()
        }
    }
}

/// Preset metadata structure
#[derive(Debug, Clone)]
pub struct PresetMetadata {
    pub file_path: String,
    pub custom_name: Option<String>,
    pub notes: Option<String>,
    pub rating: i32,
    pub play_count: i32,
    pub hidden: bool,
    /// Subfolder path within category (e.g., "City" for "Geometric/City/preset.milk")
    pub subcategory: Option<String>,
}

impl PresetMetadata {
    /// Returns custom_name if set, otherwise extracts name from file_path
    pub fn display_name(&self) -> String {
        if let Some(ref name) = self.custom_name {
            name.clone()
        } else {
            // Extract filename without extension from path
            std::path::Path::new(&self.file_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string()
        }
    }

    /// Returns the original file name without extension
    pub fn file_name(&self) -> String {
        std::path::Path::new(&self.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string()
    }

    /// Returns full hierarchical display: "custom_name - category - subcategory - file_name"
    /// If no custom name, just shows: "category - subcategory - file_name"
    pub fn full_display_name(&self, category: &str) -> String {
        let file_name = self.file_name();
        let mut parts = Vec::new();

        // Add custom name if different from file name
        if let Some(ref custom) = self.custom_name {
            if custom != &file_name {
                parts.push(custom.clone());
            }
        }

        // Add category
        parts.push(category.to_string());

        // Add subcategory if present
        if let Some(ref sub) = self.subcategory {
            // Split subcategory path and add each part
            for part in sub.split('/') {
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
            }
        }

        // Add file name
        parts.push(file_name);

        parts.join(" - ")
    }
}

/// Track info structure
#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub name: String,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// Recording metadata structure
#[derive(Debug, Clone)]
pub struct RecordingInfo {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    pub custom_name: Option<String>,
    pub notes: Option<String>,
    pub duration_secs: Option<f64>,
    pub file_size_bytes: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub fps: Option<f64>,
    pub created_at: i64,
    pub thumbnail_path: Option<String>,
    pub thumbnail_generated: bool,
}

impl RecordingInfo {
    /// Get display name (custom_name if set, otherwise file_name)
    pub fn display_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.file_name)
    }

    /// Format duration as MM:SS or HH:MM:SS
    pub fn duration_string(&self) -> String {
        match self.duration_secs {
            Some(secs) => {
                let total_secs = secs as u64;
                let hours = total_secs / 3600;
                let mins = (total_secs % 3600) / 60;
                let secs = total_secs % 60;
                if hours > 0 {
                    format!("{}:{:02}:{:02}", hours, mins, secs)
                } else {
                    format!("{}:{:02}", mins, secs)
                }
            }
            None => "?:??".to_string(),
        }
    }

    /// Format file size as human-readable
    pub fn size_string(&self) -> String {
        match self.file_size_bytes {
            Some(bytes) => {
                if bytes >= 1_073_741_824 {
                    format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
                } else if bytes >= 1_048_576 {
                    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
                } else {
                    format!("{} KB", bytes / 1024)
                }
            }
            None => "? MB".to_string(),
        }
    }
}

impl Database {
    // ========== Recording Methods ==========

    /// Add or update a recording in the database
    pub fn upsert_recording(&self, file_path: &str, file_name: &str, duration_secs: Option<f64>,
        file_size_bytes: Option<i64>, width: Option<i32>, height: Option<i32>, fps: Option<f64>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO recordings (file_path, file_name, duration_secs, file_size_bytes, width, height, fps, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET
                file_name = excluded.file_name,
                duration_secs = COALESCE(excluded.duration_secs, duration_secs),
                file_size_bytes = COALESCE(excluded.file_size_bytes, file_size_bytes),
                width = COALESCE(excluded.width, width),
                height = COALESCE(excluded.height, height),
                fps = COALESCE(excluded.fps, fps),
                updated_at = excluded.updated_at",
            params![file_path, file_name, duration_secs, file_size_bytes, width, height, fps],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Set custom name for a recording
    pub fn set_recording_custom_name(&self, file_path: &str, custom_name: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE recordings SET custom_name = ?, updated_at = strftime('%s', 'now') WHERE file_path = ?",
            params![custom_name, file_path],
        )?;
        Ok(())
    }

    /// Set notes for a recording
    pub fn set_recording_notes(&self, file_path: &str, notes: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE recordings SET notes = ?, updated_at = strftime('%s', 'now') WHERE file_path = ?",
            params![notes, file_path],
        )?;
        Ok(())
    }

    /// Get a single recording by path
    pub fn get_recording(&self, file_path: &str) -> Option<RecordingInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, file_path, file_name, custom_name, notes, duration_secs, file_size_bytes, width, height, fps, created_at, thumbnail_path, thumbnail_generated
             FROM recordings WHERE file_path = ?",
            [file_path],
            |row| Ok(RecordingInfo {
                id: row.get(0)?,
                file_path: row.get(1)?,
                file_name: row.get(2)?,
                custom_name: row.get(3).ok(),
                notes: row.get(4).ok(),
                duration_secs: row.get(5).ok(),
                file_size_bytes: row.get(6).ok(),
                width: row.get(7).ok(),
                height: row.get(8).ok(),
                fps: row.get(9).ok(),
                created_at: row.get(10)?,
                thumbnail_path: row.get(11).ok(),
                thumbnail_generated: row.get::<_, i32>(12).unwrap_or(0) != 0,
            }),
        ).ok()
    }

    /// Get all recordings, optionally filtered by directory
    pub fn get_recordings(&self, directory: Option<&str>) -> Vec<RecordingInfo> {
        let conn = self.conn.lock().unwrap();

        let (sql, param): (&str, Option<String>) = match directory {
            Some(dir) => (
                "SELECT id, file_path, file_name, custom_name, notes, duration_secs, file_size_bytes, width, height, fps, created_at, thumbnail_path, thumbnail_generated
                 FROM recordings WHERE file_path LIKE ? ORDER BY created_at DESC",
                Some(format!("{}%", dir))
            ),
            None => (
                "SELECT id, file_path, file_name, custom_name, notes, duration_secs, file_size_bytes, width, height, fps, created_at, thumbnail_path, thumbnail_generated
                 FROM recordings ORDER BY created_at DESC",
                None
            ),
        };

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let result = if let Some(ref p) = param {
            stmt.query_map([p], Self::row_to_recording_info)
        } else {
            stmt.query_map([], Self::row_to_recording_info)
        };

        result
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    fn row_to_recording_info(row: &rusqlite::Row) -> rusqlite::Result<RecordingInfo> {
        Ok(RecordingInfo {
            id: row.get(0)?,
            file_path: row.get(1)?,
            file_name: row.get(2)?,
            custom_name: row.get(3).ok(),
            notes: row.get(4).ok(),
            duration_secs: row.get(5).ok(),
            file_size_bytes: row.get(6).ok(),
            width: row.get(7).ok(),
            height: row.get(8).ok(),
            fps: row.get(9).ok(),
            created_at: row.get(10)?,
            thumbnail_path: row.get(11).ok(),
            thumbnail_generated: row.get::<_, i32>(12).unwrap_or(0) != 0,
        })
    }

    /// Delete a recording from the database (does NOT delete the file)
    pub fn delete_recording(&self, file_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM recordings WHERE file_path = ?", [file_path])?;
        Ok(())
    }

    /// Remove recordings from DB that no longer exist on disk
    pub fn prune_missing_recordings(&self, directory: &str) -> Result<usize> {
        let recordings = self.get_recordings(Some(directory));
        let mut removed = 0;
        for rec in recordings {
            if !std::path::Path::new(&rec.file_path).exists() {
                self.delete_recording(&rec.file_path)?;
                removed += 1;
            }
        }
        if removed > 0 {
            log::info!("[DB] Pruned {} missing recordings from database", removed);
        }
        Ok(removed)
    }
}

// ========== Sync Video Types ==========

/// Sync video status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncVideoStatus {
    Pending,
    Downloading,
    Paused,
    Completed,
    Failed,
}

impl SyncVideoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "downloading" => Self::Downloading,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// Sync video info from database
#[derive(Debug, Clone)]
pub struct SyncVideoInfo {
    pub id: i64,
    pub name: String,
    pub remote_url: String,
    pub remote_size: u64,
    pub local_size: u64,
    pub status: SyncVideoStatus,
    pub error: Option<String>,
}

impl SyncVideoInfo {
    pub fn progress(&self) -> f32 {
        if self.remote_size == 0 {
            0.0
        } else {
            self.local_size as f32 / self.remote_size as f32
        }
    }
}

impl Database {
    // ========== Sync Video Methods ==========

    /// Upsert a sync video (from remote listing)
    pub fn upsert_sync_video(&self, name: &str, remote_url: &str, remote_size: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_videos (name, remote_url, remote_size, updated_at)
             VALUES (?, ?, ?, strftime('%s', 'now'))
             ON CONFLICT(name) DO UPDATE SET
                remote_url = excluded.remote_url,
                remote_size = excluded.remote_size,
                updated_at = excluded.updated_at",
            params![name, remote_url, remote_size as i64],
        )?;
        Ok(())
    }

    /// Update sync video status
    pub fn set_sync_video_status(&self, name: &str, status: &SyncVideoStatus, error: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_videos SET status = ?, error = ?, updated_at = strftime('%s', 'now') WHERE name = ?",
            params![status.as_str(), error, name],
        )?;
        Ok(())
    }

    /// Update sync video local size (progress)
    pub fn set_sync_video_progress(&self, name: &str, local_size: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_videos SET local_size = ?, updated_at = strftime('%s', 'now') WHERE name = ?",
            params![local_size as i64, name],
        )?;
        Ok(())
    }

    /// Get all sync videos
    pub fn get_sync_videos(&self) -> Vec<SyncVideoInfo> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, name, remote_url, remote_size, local_size, status, error FROM sync_videos ORDER BY name"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| Ok(SyncVideoInfo {
            id: row.get(0)?,
            name: row.get(1)?,
            remote_url: row.get(2)?,
            remote_size: row.get::<_, i64>(3)? as u64,
            local_size: row.get::<_, i64>(4)? as u64,
            status: SyncVideoStatus::from_str(&row.get::<_, String>(5)?),
            error: row.get(6).ok(),
        }))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Get sync video by name
    pub fn get_sync_video(&self, name: &str) -> Option<SyncVideoInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, remote_url, remote_size, local_size, status, error FROM sync_videos WHERE name = ?",
            [name],
            |row| Ok(SyncVideoInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                remote_url: row.get(2)?,
                remote_size: row.get::<_, i64>(3)? as u64,
                local_size: row.get::<_, i64>(4)? as u64,
                status: SyncVideoStatus::from_str(&row.get::<_, String>(5)?),
                error: row.get(6).ok(),
            }),
        ).ok()
    }

    /// Get next video to sync (pending, paused, or interrupted downloads)
    pub fn get_next_sync_video(&self) -> Option<SyncVideoInfo> {
        let conn = self.conn.lock().unwrap();
        // Priority: downloading (interrupted) > paused > pending
        conn.query_row(
            "SELECT id, name, remote_url, remote_size, local_size, status, error
             FROM sync_videos
             WHERE status IN ('downloading', 'paused', 'pending')
             ORDER BY
                CASE status
                    WHEN 'downloading' THEN 1  -- Resume interrupted first
                    WHEN 'paused' THEN 2       -- Then paused
                    WHEN 'pending' THEN 3      -- Then new downloads
                END,
                name
             LIMIT 1",
            [],
            |row| Ok(SyncVideoInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                remote_url: row.get(2)?,
                remote_size: row.get::<_, i64>(3)? as u64,
                local_size: row.get::<_, i64>(4)? as u64,
                status: SyncVideoStatus::from_str(&row.get::<_, String>(5)?),
                error: row.get(6).ok(),
            }),
        ).ok()
    }

    /// Reset interrupted downloads (status = 'downloading') to pending
    /// Call this on app startup to clean up stale state
    pub fn reset_interrupted_downloads(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE sync_videos SET status = 'paused' WHERE status = 'downloading'",
            [],
        )?;
        if count > 0 {
            log::info!("[DB] Reset {} interrupted downloads to paused", count);
        }
        Ok(count)
    }

    /// Get sync stats (counts by status)
    /// Returns: (pending, downloading, completed, failed, paused, total_bytes, downloaded_bytes)
    pub fn get_sync_stats(&self) -> (usize, usize, usize, usize, usize, u64, u64) {
        let conn = self.conn.lock().unwrap();

        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_videos WHERE status = 'pending'", [], |r| r.get(0)
        ).unwrap_or(0);

        let downloading: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_videos WHERE status = 'downloading'", [], |r| r.get(0)
        ).unwrap_or(0);

        let completed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_videos WHERE status = 'completed'", [], |r| r.get(0)
        ).unwrap_or(0);

        let failed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_videos WHERE status = 'failed'", [], |r| r.get(0)
        ).unwrap_or(0);

        let paused: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_videos WHERE status = 'paused'", [], |r| r.get(0)
        ).unwrap_or(0);

        let total_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(remote_size), 0) FROM sync_videos WHERE status NOT IN ('completed')", [], |r| r.get(0)
        ).unwrap_or(0);

        let downloaded_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(local_size), 0) FROM sync_videos WHERE status NOT IN ('completed')", [], |r| r.get(0)
        ).unwrap_or(0);

        (pending as usize, downloading as usize, completed as usize, failed as usize, paused as usize,
         total_bytes as u64, downloaded_bytes as u64)
    }

    /// Clear all sync videos (for refresh)
    pub fn clear_sync_videos(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_videos", [])?;
        Ok(())
    }

    /// Mark completed videos that exist locally and update progress for partial downloads
    /// Also resets "completed" videos to pending if file is missing
    pub fn mark_existing_videos_completed(&self, video_dir: &str) -> Result<()> {
        let videos = self.get_sync_videos();
        for video in videos {
            let local_path = std::path::Path::new(video_dir).join(&video.name);
            let part_path = local_path.with_extension("mp4.part");

            // Check for completed file
            if local_path.exists() {
                if let Ok(meta) = std::fs::metadata(&local_path) {
                    let local_size = meta.len();
                    if local_size >= video.remote_size {
                        self.set_sync_video_status(&video.name, &SyncVideoStatus::Completed, None)?;
                        self.set_sync_video_progress(&video.name, local_size)?;
                        continue;
                    }
                }
            }

            // Check for partial download (.part file)
            if part_path.exists() {
                if let Ok(meta) = std::fs::metadata(&part_path) {
                    let partial_size = meta.len();
                    if partial_size > 0 && partial_size < video.remote_size {
                        // Update progress and mark as paused (resumable)
                        self.set_sync_video_progress(&video.name, partial_size)?;
                        if video.status == SyncVideoStatus::Pending || video.status == SyncVideoStatus::Downloading {
                            self.set_sync_video_status(&video.name, &SyncVideoStatus::Paused, None)?;
                        }
                        continue;
                    }
                }
            }

            // If marked completed but file doesn't exist, reset to pending for re-download
            if video.status == SyncVideoStatus::Completed && !local_path.exists() {
                log::info!("[DB] Resetting missing video to pending: {}", video.name);
                self.set_sync_video_status(&video.name, &SyncVideoStatus::Pending, None)?;
                self.set_sync_video_progress(&video.name, 0)?;
            }
        }
        Ok(())
    }
}

// ========== Compel Video Types ==========

/// Compel video info from database
#[derive(Debug, Clone)]
pub struct CompelVideoInfo {
    pub id: i64,
    pub compel_id: i64,
    pub video_id: i64,
    pub name: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub remote_size: u64,
    pub local_size: u64,
    pub status: SyncVideoStatus,
    pub error: Option<String>,
}

impl CompelVideoInfo {
    pub fn progress(&self) -> f32 {
        if self.remote_size == 0 {
            0.0
        } else {
            self.local_size as f32 / self.remote_size as f32
        }
    }
}

impl Database {
    // ========== Compel Video Methods ==========

    /// Upsert a compel video (from API listing)
    pub fn upsert_compel_video(&self, compel_id: i64, video_id: i64, name: &str, artist: Option<&str>, title: Option<&str>, remote_size: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO compel_videos (compel_id, video_id, name, artist, title, remote_size, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, strftime('%s', 'now'))
             ON CONFLICT(video_id) DO UPDATE SET
                compel_id = excluded.compel_id,
                name = excluded.name,
                artist = excluded.artist,
                title = excluded.title,
                remote_size = excluded.remote_size,
                updated_at = excluded.updated_at",
            params![compel_id, video_id, name, artist, title, remote_size as i64],
        )?;
        Ok(())
    }

    /// Update compel video status
    pub fn set_compel_video_status(&self, video_id: i64, status: &SyncVideoStatus, error: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE compel_videos SET status = ?, error = ?, updated_at = strftime('%s', 'now') WHERE video_id = ?",
            params![status.as_str(), error, video_id],
        )?;
        Ok(())
    }

    /// Update compel video local size (progress)
    pub fn set_compel_video_progress(&self, video_id: i64, local_size: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE compel_videos SET local_size = ?, updated_at = strftime('%s', 'now') WHERE video_id = ?",
            params![local_size as i64, video_id],
        )?;
        Ok(())
    }

    /// Get next compel video to sync (pending, paused, or interrupted downloads)
    pub fn get_next_compel_video(&self) -> Option<CompelVideoInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, compel_id, video_id, name, artist, title, remote_size, local_size, status, error
             FROM compel_videos
             WHERE status IN ('downloading', 'paused', 'pending')
             ORDER BY
                CASE status
                    WHEN 'downloading' THEN 1
                    WHEN 'paused' THEN 2
                    WHEN 'pending' THEN 3
                END,
                name
             LIMIT 1",
            [],
            |row| Ok(CompelVideoInfo {
                id: row.get(0)?,
                compel_id: row.get(1)?,
                video_id: row.get(2)?,
                name: row.get(3)?,
                artist: row.get(4).ok(),
                title: row.get(5).ok(),
                remote_size: row.get::<_, i64>(6)? as u64,
                local_size: row.get::<_, i64>(7)? as u64,
                status: SyncVideoStatus::from_str(&row.get::<_, String>(8)?),
                error: row.get(9).ok(),
            }),
        ).ok()
    }

    /// Get compel sync stats
    /// Returns: (pending, downloading, completed, failed, total)
    /// Returns: (pending, downloading, completed, failed, total, total_bytes, downloaded_bytes)
    pub fn get_compel_sync_stats(&self) -> (usize, usize, usize, usize, usize, u64, u64) {
        let conn = self.conn.lock().unwrap();

        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM compel_videos WHERE status = 'pending'", [], |r| r.get(0)
        ).unwrap_or(0);

        let downloading: i64 = conn.query_row(
            "SELECT COUNT(*) FROM compel_videos WHERE status = 'downloading'", [], |r| r.get(0)
        ).unwrap_or(0);

        let completed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM compel_videos WHERE status = 'completed'", [], |r| r.get(0)
        ).unwrap_or(0);

        let failed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM compel_videos WHERE status = 'failed'", [], |r| r.get(0)
        ).unwrap_or(0);

        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM compel_videos", [], |r| r.get(0)
        ).unwrap_or(0);

        let total_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(remote_size), 0) FROM compel_videos", [], |r| r.get(0)
        ).unwrap_or(0);

        let downloaded_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(local_size), 0) FROM compel_videos", [], |r| r.get(0)
        ).unwrap_or(0);

        (pending as usize, downloading as usize, completed as usize, failed as usize, total as usize,
         total_bytes as u64, downloaded_bytes as u64)
    }

    /// Reset interrupted compel downloads to paused on startup
    pub fn reset_interrupted_compel_downloads(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE compel_videos SET status = 'paused' WHERE status = 'downloading'",
            [],
        )?;
        if count > 0 {
            log::info!("[DB] Reset {} interrupted compel downloads to paused", count);
        }
        Ok(count)
    }

    /// Get all compel videos
    pub fn get_all_compel_videos(&self) -> Vec<(i64, String, u64, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT video_id, name, remote_size, status FROM compel_videos"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| Ok((
            row.get(0)?,
            row.get(1)?,
            row.get::<_, i64>(2)? as u64,
            row.get(3)?,
        )))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Get all compel videos as full CompelVideoInfo structs (for sync UI)
    pub fn get_compel_videos(&self) -> Vec<CompelVideoInfo> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, compel_id, video_id, name, artist, title, remote_size, local_size, status, error
             FROM compel_videos ORDER BY status = 'downloading' DESC, status = 'pending' DESC, name"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| Ok(CompelVideoInfo {
            id: row.get(0)?,
            compel_id: row.get(1)?,
            video_id: row.get(2)?,
            name: row.get(3)?,
            artist: row.get(4)?,
            title: row.get(5)?,
            remote_size: row.get::<_, i64>(6)? as u64,
            local_size: row.get::<_, i64>(7)? as u64,
            status: SyncVideoStatus::from_str(row.get_ref(8)?.as_str().unwrap_or("pending")),
            error: row.get(9)?,
        }))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Get last compel sync timestamp (max updated_at for `since` param)
    pub fn get_last_compel_sync(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(updated_at), 0) FROM compel_videos",
            [],
            |r| r.get(0),
        ).unwrap_or(0)
    }
}

// ========== Set Upload Types ==========

/// Set upload status (for uploading recordings to Compeller.ai)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetUploadStatus {
    Pending,
    Uploading,
    Paused,
    Completed,
    Failed,
}

impl SetUploadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Uploading => "uploading",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "uploading" => Self::Uploading,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// Set upload info from database
#[derive(Debug, Clone)]
pub struct SetUploadInfo {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    pub total_size: u64,
    pub uploaded_size: u64,
    pub session_token: Option<String>,
    pub status: SetUploadStatus,
    pub error: Option<String>,
    pub title: Option<String>,
    pub recorded_at: Option<String>,
    pub theme_used: Option<String>,
    pub audio_source: Option<String>,
    pub duration_seconds: Option<i64>,
    pub checksum: Option<String>,
    pub remote_set_id: Option<i64>,
}

impl SetUploadInfo {
    pub fn progress(&self) -> f32 {
        if self.total_size == 0 {
            0.0
        } else {
            self.uploaded_size as f32 / self.total_size as f32
        }
    }
}

impl Database {
    // ========== Set Upload Methods ==========

    /// Add a recording for upload
    pub fn add_set_upload(
        &self,
        file_path: &str,
        file_name: &str,
        total_size: u64,
        title: Option<&str>,
        recorded_at: Option<&str>,
        theme_used: Option<&str>,
        audio_source: Option<&str>,
        duration_seconds: Option<i64>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO set_uploads (file_path, file_name, total_size, title, recorded_at, theme_used, audio_source, duration_seconds, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, strftime('%s', 'now'))
             ON CONFLICT(file_path) DO UPDATE SET
                file_name = excluded.file_name,
                total_size = excluded.total_size,
                title = excluded.title,
                updated_at = excluded.updated_at",
            params![file_path, file_name, total_size as i64, title, recorded_at, theme_used, audio_source, duration_seconds],
        )?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }

    /// Update set upload status
    pub fn set_upload_status(&self, file_path: &str, status: &SetUploadStatus, error: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE set_uploads SET status = ?, error = ?, updated_at = strftime('%s', 'now') WHERE file_path = ?",
            params![status.as_str(), error, file_path],
        )?;
        Ok(())
    }

    /// Update set upload progress
    pub fn set_upload_progress(&self, file_path: &str, uploaded_size: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE set_uploads SET uploaded_size = ?, updated_at = strftime('%s', 'now') WHERE file_path = ?",
            params![uploaded_size as i64, file_path],
        )?;
        Ok(())
    }

    /// Update set upload session token
    pub fn set_upload_session(&self, file_path: &str, session_token: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE set_uploads SET session_token = ?, updated_at = strftime('%s', 'now') WHERE file_path = ?",
            params![session_token, file_path],
        )?;
        Ok(())
    }

    /// Update set upload checksum
    pub fn set_upload_checksum(&self, file_path: &str, checksum: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE set_uploads SET checksum = ?, updated_at = strftime('%s', 'now') WHERE file_path = ?",
            params![checksum, file_path],
        )?;
        Ok(())
    }

    /// Mark upload as completed with remote set ID
    pub fn set_upload_completed(&self, file_path: &str, remote_set_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE set_uploads SET status = 'completed', remote_set_id = ?, updated_at = strftime('%s', 'now') WHERE file_path = ?",
            params![remote_set_id, file_path],
        )?;
        Ok(())
    }

    /// Get all set uploads
    pub fn get_set_uploads(&self) -> Vec<SetUploadInfo> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, file_path, file_name, total_size, uploaded_size, session_token, status, error,
                    title, recorded_at, theme_used, audio_source, duration_seconds, checksum, remote_set_id
             FROM set_uploads ORDER BY created_at DESC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| Ok(SetUploadInfo {
            id: row.get(0)?,
            file_path: row.get(1)?,
            file_name: row.get(2)?,
            total_size: row.get::<_, i64>(3)? as u64,
            uploaded_size: row.get::<_, i64>(4)? as u64,
            session_token: row.get(5).ok(),
            status: SetUploadStatus::from_str(&row.get::<_, String>(6)?),
            error: row.get(7).ok(),
            title: row.get(8).ok(),
            recorded_at: row.get(9).ok(),
            theme_used: row.get(10).ok(),
            audio_source: row.get(11).ok(),
            duration_seconds: row.get(12).ok(),
            checksum: row.get(13).ok(),
            remote_set_id: row.get(14).ok(),
        }))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Get set upload by file path
    pub fn get_set_upload(&self, file_path: &str) -> Option<SetUploadInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, file_path, file_name, total_size, uploaded_size, session_token, status, error,
                    title, recorded_at, theme_used, audio_source, duration_seconds, checksum, remote_set_id
             FROM set_uploads WHERE file_path = ?",
            [file_path],
            |row| Ok(SetUploadInfo {
                id: row.get(0)?,
                file_path: row.get(1)?,
                file_name: row.get(2)?,
                total_size: row.get::<_, i64>(3)? as u64,
                uploaded_size: row.get::<_, i64>(4)? as u64,
                session_token: row.get(5).ok(),
                status: SetUploadStatus::from_str(&row.get::<_, String>(6)?),
                error: row.get(7).ok(),
                title: row.get(8).ok(),
                recorded_at: row.get(9).ok(),
                theme_used: row.get(10).ok(),
                audio_source: row.get(11).ok(),
                duration_seconds: row.get(12).ok(),
                checksum: row.get(13).ok(),
                remote_set_id: row.get(14).ok(),
            }),
        ).ok()
    }

    /// Get next set to upload (pending, paused, or interrupted uploads)
    pub fn get_next_set_upload(&self) -> Option<SetUploadInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, file_path, file_name, total_size, uploaded_size, session_token, status, error,
                    title, recorded_at, theme_used, audio_source, duration_seconds, checksum, remote_set_id
             FROM set_uploads
             WHERE status IN ('uploading', 'paused', 'pending')
             ORDER BY
                CASE status
                    WHEN 'uploading' THEN 1
                    WHEN 'paused' THEN 2
                    WHEN 'pending' THEN 3
                END,
                created_at
             LIMIT 1",
            [],
            |row| Ok(SetUploadInfo {
                id: row.get(0)?,
                file_path: row.get(1)?,
                file_name: row.get(2)?,
                total_size: row.get::<_, i64>(3)? as u64,
                uploaded_size: row.get::<_, i64>(4)? as u64,
                session_token: row.get(5).ok(),
                status: SetUploadStatus::from_str(&row.get::<_, String>(6)?),
                error: row.get(7).ok(),
                title: row.get(8).ok(),
                recorded_at: row.get(9).ok(),
                theme_used: row.get(10).ok(),
                audio_source: row.get(11).ok(),
                duration_seconds: row.get(12).ok(),
                checksum: row.get(13).ok(),
                remote_set_id: row.get(14).ok(),
            }),
        ).ok()
    }

    /// Reset interrupted uploads to paused
    pub fn reset_interrupted_uploads(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE set_uploads SET status = 'paused' WHERE status = 'uploading'",
            [],
        )?;
        if count > 0 {
            log::info!("[DB] Reset {} interrupted uploads to paused", count);
        }
        Ok(count)
    }

    /// Get upload stats (counts by status)
    pub fn get_upload_stats(&self) -> (usize, usize, usize, usize, usize, u64, u64) {
        let conn = self.conn.lock().unwrap();

        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM set_uploads WHERE status = 'pending'", [], |r| r.get(0)
        ).unwrap_or(0);

        let uploading: i64 = conn.query_row(
            "SELECT COUNT(*) FROM set_uploads WHERE status = 'uploading'", [], |r| r.get(0)
        ).unwrap_or(0);

        let completed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM set_uploads WHERE status = 'completed'", [], |r| r.get(0)
        ).unwrap_or(0);

        let failed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM set_uploads WHERE status = 'failed'", [], |r| r.get(0)
        ).unwrap_or(0);

        let paused: i64 = conn.query_row(
            "SELECT COUNT(*) FROM set_uploads WHERE status = 'paused'", [], |r| r.get(0)
        ).unwrap_or(0);

        let total_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(total_size), 0) FROM set_uploads WHERE status NOT IN ('completed')", [], |r| r.get(0)
        ).unwrap_or(0);

        let uploaded_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(uploaded_size), 0) FROM set_uploads WHERE status NOT IN ('completed')", [], |r| r.get(0)
        ).unwrap_or(0);

        (pending as usize, uploading as usize, completed as usize, failed as usize, paused as usize,
         total_bytes as u64, uploaded_bytes as u64)
    }

    /// Delete a set upload record
    pub fn delete_set_upload(&self, file_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM set_uploads WHERE file_path = ?", params![file_path])?;
        Ok(())
    }

    /// Get upload status for a specific file
    pub fn get_upload_status(&self, file_path: &str) -> Option<SetUploadStatus> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT status FROM set_uploads WHERE file_path = ?",
            [file_path],
            |row| {
                let status_str: String = row.get(0)?;
                Ok(SetUploadStatus::from_str(&status_str))
            }
        ).ok()
    }

    /// Get upload status map for all files (efficient batch lookup)
    pub fn get_upload_status_map(&self) -> std::collections::HashMap<String, SetUploadStatus> {
        let conn = self.conn.lock().unwrap();
        let mut map = std::collections::HashMap::new();

        if let Ok(mut stmt) = conn.prepare("SELECT file_path, status FROM set_uploads") {
            if let Ok(rows) = stmt.query_map([], |row| {
                let path: String = row.get(0)?;
                let status_str: String = row.get(1)?;
                Ok((path, SetUploadStatus::from_str(&status_str)))
            }) {
                for row in rows.flatten() {
                    map.insert(row.0, row.1);
                }
            }
        }
        map
    }

    /// Retry a failed upload (reset status to pending)
    pub fn retry_failed_upload(&self, file_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE set_uploads SET status = 'pending', error = NULL, uploaded_size = 0, session_token = NULL, updated_at = strftime('%s', 'now') WHERE file_path = ? AND status = 'failed'",
            params![file_path],
        )?;
        log::info!("[DB] Retry upload: {}", file_path);
        Ok(())
    }

    /// Retry all failed uploads
    pub fn retry_all_failed_uploads(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE set_uploads SET status = 'pending', error = NULL, uploaded_size = 0, session_token = NULL, updated_at = strftime('%s', 'now') WHERE status = 'failed'",
            [],
        )?;
        if count > 0 {
            log::info!("[DB] Retry {} failed uploads", count);
        }
        Ok(count)
    }

    /// Get all completed uploads with their remote set IDs
    pub fn get_completed_upload_remote_ids(&self) -> Vec<(String, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut result = Vec::new();

        if let Ok(mut stmt) = conn.prepare(
            "SELECT file_path, remote_set_id FROM set_uploads WHERE status = 'completed' AND remote_set_id IS NOT NULL"
        ) {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            }) {
                for row in rows.flatten() {
                    result.push(row);
                }
            }
        }
        result
    }

    /// Reset sync status for uploads whose remote_set_id is not in the valid set
    /// Returns the number of uploads that were reset or deleted
    pub fn reset_invalid_completed_uploads(&self, valid_remote_ids: &[i64]) -> Result<usize> {
        let completed = self.get_completed_upload_remote_ids();
        let mut reset_count = 0;

        let conn = self.conn.lock().unwrap();
        for (file_path, remote_id) in completed {
            if !valid_remote_ids.contains(&remote_id) {
                // Check if the file still exists
                if std::path::Path::new(&file_path).exists() {
                    // File exists but remote doesn't - reset to pending for re-upload
                    conn.execute(
                        "UPDATE set_uploads SET status = 'pending', remote_set_id = NULL, session_token = NULL, uploaded_size = 0, error = NULL, updated_at = strftime('%s', 'now') WHERE file_path = ?",
                        params![file_path],
                    )?;
                    log::info!("[DB] Reset invalid completed upload: {} (remote_id {} not found in SaaS)", file_path, remote_id);
                } else {
                    // File doesn't exist - delete the entry entirely
                    conn.execute("DELETE FROM set_uploads WHERE file_path = ?", params![file_path])?;
                    log::info!("[DB] Deleted orphaned completed upload: {} (file and remote both gone)", file_path);
                }
                reset_count += 1;
            }
        }

        if reset_count > 0 {
            log::info!("[DB] Processed {} uploads with invalid remote IDs", reset_count);
        }

        Ok(reset_count)
    }

    /// Remove orphaned upload entries for files that no longer exist on disk
    pub fn cleanup_orphaned_uploads(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();

        // Get all file paths from the uploads table
        let mut stmt = conn.prepare("SELECT file_path FROM set_uploads")?;
        let paths: Vec<String> = stmt.query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        // Check which files no longer exist
        let orphaned: Vec<&String> = paths.iter()
            .filter(|path| !std::path::Path::new(path).exists())
            .collect();

        if orphaned.is_empty() {
            return Ok(0);
        }

        // Delete orphaned entries
        let mut deleted = 0;
        for path in &orphaned {
            if conn.execute("DELETE FROM set_uploads WHERE file_path = ?", params![path]).is_ok() {
                log::info!("[DB] Removed orphaned upload entry: {}", path);
                deleted += 1;
            }
        }

        if deleted > 0 {
            log::info!("[DB] Cleaned up {} orphaned upload entries", deleted);
        }

        Ok(deleted)
    }
}

impl Database {
    /// Update or insert a CDJ device state
    pub fn upsert_cdj_device(
        &self,
        device_id: u8,
        name: &str,
        ip: &str,
        track_title: Option<&str>,
        track_artist: Option<&str>,
        is_master: bool,
        is_playing: bool,
        is_looping: bool,
        is_on_air: bool,
        bpm: Option<f32>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO cdj_devices
             (device_id, name, ip, track_title, track_artist, is_master, is_playing, is_looping, is_on_air, bpm, last_seen)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%s', 'now'))",
            params![
                device_id as i64,
                name,
                ip,
                track_title,
                track_artist,
                is_master as i64,
                is_playing as i64,
                is_looping as i64,
                is_on_air as i64,
                bpm,
            ],
        )?;
        Ok(())
    }

    /// Get all CDJ devices from database
    pub fn get_cdj_devices(&self) -> Vec<CdjDeviceRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT device_id, name, ip, track_title, track_artist, is_master, is_playing, is_looping, is_on_air, bpm, last_seen
             FROM cdj_devices
             ORDER BY device_id"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map([], |row| {
            Ok(CdjDeviceRecord {
                device_id: row.get::<_, i64>(0)? as u8,
                name: row.get(1)?,
                ip: row.get(2)?,
                track_title: row.get(3)?,
                track_artist: row.get(4)?,
                is_master: row.get::<_, i64>(5)? != 0,
                is_playing: row.get::<_, i64>(6)? != 0,
                is_looping: row.get::<_, i64>(7)? != 0,
                is_on_air: row.get::<_, i64>(8)? != 0,
                bpm: row.get(9)?,
                last_seen: row.get(10)?,
            })
        });

        match rows {
            Ok(r) => r.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Clear all CDJ devices from database
    pub fn clear_cdj_devices(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM cdj_devices", [])?;
        Ok(())
    }

    /// Remove stale CDJ devices (not seen for more than N seconds)
    pub fn prune_stale_cdj_devices(&self, max_age_secs: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM cdj_devices WHERE last_seen < strftime('%s', 'now') - ?",
            [max_age_secs],
        )?;
        Ok(())
    }

    // ========== Authentication Methods ==========

    /// Save auth credentials (after successful login)
    pub fn save_auth_credentials(
        &self,
        username: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        roles: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Clear any existing credentials first (only one user at a time)
        conn.execute("DELETE FROM auth_credentials", [])?;
        conn.execute(
            "INSERT INTO auth_credentials (username, access_token, refresh_token, roles, updated_at)
             VALUES (?, ?, ?, ?, strftime('%s', 'now'))",
            params![username, access_token, refresh_token, roles],
        )?;
        log::info!("[DB] Saved auth credentials for {}", username);
        Ok(())
    }

    /// Get saved auth credentials
    /// Returns: (username, access_token, refresh_token, roles)
    pub fn get_auth_credentials(&self) -> Option<(String, String, Option<String>, String)> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT username, access_token, refresh_token, roles FROM auth_credentials LIMIT 1",
            [],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            )),
        ).ok()
    }

    /// Clear auth credentials (logout)
    pub fn clear_auth_credentials(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM auth_credentials", [])?;
        log::info!("[DB] Cleared auth credentials");
        Ok(())
    }

    /// Update just the access token (after refresh)
    pub fn update_access_token(&self, access_token: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE auth_credentials SET access_token = ?, updated_at = strftime('%s', 'now')",
            [access_token],
        )?;
        Ok(())
    }

    /// Save login credentials for auto-login
    pub fn save_login_credentials(&self, username: &str, password: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM login_credentials", [])?;
        conn.execute(
            "INSERT INTO login_credentials (username, password) VALUES (?, ?)",
            params![username, password],
        )?;
        Ok(())
    }

    /// Get saved login credentials
    pub fn get_login_credentials(&self) -> Option<(String, String)> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT username, password FROM login_credentials LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).ok()
    }

    /// Clear login credentials
    pub fn clear_login_credentials(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM login_credentials", [])?;
        Ok(())
    }

    // ========== License Blob Methods ==========

    /// Save license blob for offline caching
    pub fn save_license_blob(&self, blob: &str, signature: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM license_blob", [])?;
        conn.execute(
            "INSERT INTO license_blob (blob, signature, fetched_at) VALUES (?, ?, strftime('%s', 'now'))",
            params![blob, signature],
        )?;
        log::info!("[DB] Saved license blob");
        Ok(())
    }

    /// Get cached license blob
    /// Returns: (blob, signature, fetched_at_unix)
    pub fn get_license_blob(&self) -> Option<(String, String, i64)> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT blob, signature, fetched_at FROM license_blob LIMIT 1",
            [],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            )),
        ).ok()
    }

    /// Clear cached license blob (on logout)
    pub fn clear_license_blob(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM license_blob", [])?;
        log::info!("[DB] Cleared license blob");
        Ok(())
    }

    // ========== Connected Accounts Methods ==========

    /// Save a connected account (OAuth provider)
    pub fn save_connected_account(&self, account: &crate::auth::ConnectedAccount) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let provider_str = account.provider.as_str();
        let state_str = match &account.state {
            crate::auth::ConnectedAccountState::Disconnected => "disconnected",
            crate::auth::ConnectedAccountState::Connecting => "connecting",
            crate::auth::ConnectedAccountState::Connected => "connected",
            crate::auth::ConnectedAccountState::Error(_) => "error",
            crate::auth::ConnectedAccountState::NeedsReauth => "needs_reauth",
        };
        let scopes = account.scopes_granted.join(",");

        conn.execute(
            "INSERT OR REPLACE INTO connected_accounts
             (provider, state, refresh_token, access_token, token_expires_at, scopes_granted,
              provider_user_id, provider_display_name, connected_at, last_refreshed_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%s', 'now'))",
            params![
                provider_str,
                state_str,
                account.refresh_token,
                account.access_token,
                account.token_expires_at.map(|v| v as i64),
                scopes,
                account.provider_user_id,
                account.provider_display_name,
                account.connected_at as i64,
                account.last_refreshed_at as i64,
            ],
        )?;
        log::info!("[DB] Saved connected account: {}", provider_str);
        Ok(())
    }

    /// Get all connected accounts
    pub fn get_connected_accounts(&self) -> Result<Vec<crate::auth::ConnectedAccount>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT provider, state, refresh_token, access_token, token_expires_at, scopes_granted,
                    provider_user_id, provider_display_name, connected_at, last_refreshed_at
             FROM connected_accounts"
        )?;

        let rows = stmt.query_map([], |row| {
            let provider_str: String = row.get(0)?;
            let state_str: String = row.get(1)?;
            let scopes_str: String = row.get(5)?;

            let provider = crate::auth::ProviderId::from_str(&provider_str)
                .unwrap_or(crate::auth::ProviderId::Google);

            let state = match state_str.as_str() {
                "connected" => crate::auth::ConnectedAccountState::Connected,
                "connecting" => crate::auth::ConnectedAccountState::Connecting,
                "error" => crate::auth::ConnectedAccountState::Error("Unknown error".to_string()),
                "needs_reauth" => crate::auth::ConnectedAccountState::NeedsReauth,
                _ => crate::auth::ConnectedAccountState::Disconnected,
            };

            let scopes: Vec<String> = scopes_str
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            Ok(crate::auth::ConnectedAccount {
                provider,
                state,
                refresh_token: row.get(2)?,
                access_token: row.get(3)?,
                token_expires_at: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                scopes_granted: scopes,
                provider_user_id: row.get(6)?,
                provider_display_name: row.get(7)?,
                connected_at: row.get::<_, i64>(8)? as u64,
                last_refreshed_at: row.get::<_, i64>(9)? as u64,
            })
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get a connected account by provider
    pub fn get_connected_account(&self, provider: &str) -> Option<crate::auth::ConnectedAccount> {
        self.get_connected_accounts()
            .ok()
            .and_then(|accounts: Vec<crate::auth::ConnectedAccount>| accounts.into_iter().find(|a| a.provider.as_str() == provider))
    }

    /// Delete a connected account
    pub fn delete_connected_account(&self, provider: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM connected_accounts WHERE provider = ?", [provider])?;
        log::info!("[DB] Deleted connected account: {}", provider);
        Ok(())
    }

    /// Update connected account tokens after refresh
    pub fn update_connected_account_tokens(
        &self,
        provider: &str,
        access_token: &str,
        expires_at: Option<u64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE connected_accounts
             SET access_token = ?, token_expires_at = ?, last_refreshed_at = strftime('%s', 'now'), updated_at = strftime('%s', 'now')
             WHERE provider = ?",
            params![access_token, expires_at.map(|v| v as i64), provider],
        )?;
        Ok(())
    }


    // ========== Segmented Recording Methods (Crash Recovery) ==========

    /// Create a new recording session
    pub fn create_recording_session(&self, session_id: &str, output_dir: &str, config: &RecordingSessionConfig) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO recording_sessions (session_id, output_dir, segment_duration_ms, width, height, fps, status)
             VALUES (?, ?, ?, ?, ?, ?, 'active')",
            params![session_id, output_dir, config.segment_duration_ms as i64, config.width, config.height, config.fps],
        )?;
        log::info!("[DB] Created recording session: {}", session_id);
        Ok(())
    }

    /// Begin a new segment (phase 1 of two-phase commit)
    pub fn begin_segment(&self, session_id: &str, segment_index: u32, filename_tmp: &str, first_pts_ns: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Use INSERT OR REPLACE to handle leftover entries from crashed sessions
        conn.execute(
            "INSERT OR REPLACE INTO segment_journal (session_id, segment_index, status, filename_tmp, first_pts_ns)
             VALUES (?, ?, 'begun', ?, ?)",
            params![session_id, segment_index, filename_tmp, first_pts_ns as i64],
        )?;
        Ok(())
    }

    /// Finalize a segment (phase 2a - segment written to disk but not yet committed)
    pub fn finalize_segment(&self, session_id: &str, segment_index: u32, metadata: &SegmentMetadata) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE segment_journal
             SET status = 'finalized', duration_ms = ?, last_pts_ns = ?, file_size_bytes = ?,
                 video_frames = ?, audio_samples = ?, updated_at = strftime('%s', 'now')
             WHERE session_id = ? AND segment_index = ?",
            params![
                metadata.duration_ms as i64,
                metadata.last_pts_ns as i64,
                metadata.file_size_bytes as i64,
                metadata.video_frames as i64,
                metadata.audio_samples as i64,
                session_id,
                segment_index
            ],
        )?;
        Ok(())
    }

    /// Commit a segment (phase 2b - atomic rename complete, segment is safe)
    pub fn commit_segment(&self, session_id: &str, segment_index: u32, filename_final: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE segment_journal
             SET status = 'committed', filename_final = ?, updated_at = strftime('%s', 'now')
             WHERE session_id = ? AND segment_index = ?",
            params![filename_final, session_id, segment_index],
        )?;

        // Update session totals
        conn.execute(
            "UPDATE recording_sessions
             SET total_segments = (SELECT COUNT(*) FROM segment_journal WHERE session_id = ? AND status = 'committed'),
                 total_duration_ms = (SELECT COALESCE(SUM(duration_ms), 0) FROM segment_journal WHERE session_id = ? AND status = 'committed'),
                 updated_at = strftime('%s', 'now')
             WHERE session_id = ?",
            params![session_id, session_id, session_id],
        )?;
        Ok(())
    }

    /// Mark session status (active, stopped, crashed, recovered)
    pub fn mark_session_status(&self, session_id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE recording_sessions SET status = ?, updated_at = strftime('%s', 'now') WHERE session_id = ?",
            params![status, session_id],
        )?;
        Ok(())
    }

    /// Get committed segments for a session (for recovery/stitching)
    pub fn get_committed_segments(&self, session_id: &str) -> Vec<CommittedSegment> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT segment_index, filename_final, duration_ms, first_pts_ns, last_pts_ns, file_size_bytes
             FROM segment_journal
             WHERE session_id = ? AND status = 'committed'
             ORDER BY segment_index ASC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([session_id], |row| {
            Ok(CommittedSegment {
                index: row.get::<_, i32>(0)? as u32,
                path: std::path::PathBuf::from(row.get::<_, String>(1)?),
                duration_ms: row.get::<_, i64>(2)? as u64,
                first_pts_ns: row.get::<_, i64>(3)? as u64,
                last_pts_ns: row.get::<_, i64>(4)? as u64,
                file_size_bytes: row.get::<_, i64>(5)? as u64,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Get all crashed sessions (status = 'active' means we crashed during recording)
    pub fn get_crashed_sessions(&self) -> Vec<CrashedSession> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT session_id, output_dir, segment_duration_ms, width, height, fps, total_segments, total_duration_ms, created_at
             FROM recording_sessions
             WHERE status = 'active'"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let sessions: Vec<_> = stmt.query_map([], |row| {
            Ok(CrashedSession {
                session_id: row.get(0)?,
                output_dir: std::path::PathBuf::from(row.get::<_, String>(1)?),
                segment_duration_ms: row.get::<_, i64>(2)? as u64,
                width: row.get::<_, i32>(3)? as u32,
                height: row.get::<_, i32>(4)? as u32,
                fps: row.get::<_, i32>(5)? as u32,
                total_segments: row.get::<_, i32>(6)? as u32,
                total_duration_ms: row.get::<_, i64>(7)? as u64,
                created_at: row.get(8)?,
                committed_segments: Vec::new(), // Will be filled in
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

        // Fill in committed segments for each session
        drop(stmt);
        drop(conn);

        sessions.into_iter().map(|mut s| {
            s.committed_segments = self.get_committed_segments(&s.session_id);
            s
        }).collect()
    }

    /// Clean up abandoned segments (begun but not finalized/committed)
    pub fn cleanup_abandoned_segments(&self, session_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE segment_journal SET status = 'abandoned' WHERE session_id = ? AND status IN ('begun', 'finalized')",
            [session_id],
        )?;
        Ok(count)
    }

    /// Delete all data for a session
    pub fn delete_recording_session(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM segment_journal WHERE session_id = ?", [session_id])?;
        conn.execute("DELETE FROM recording_sessions WHERE session_id = ?", [session_id])?;
        Ok(())
    }

    /// Clean up stale 'active' sessions from previous crashes (call on startup)
    /// Marks them as 'crashed' so they can be recovered or discarded
    pub fn cleanup_stale_active_sessions(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE recording_sessions SET status = 'crashed', updated_at = strftime('%s', 'now') WHERE status = 'active'",
            [],
        )?;
        if count > 0 {
            log::info!("[DB] Marked {} stale recording sessions as crashed", count);
        }
        Ok(count)
    }
}

// ========== Segmented Recording Types ==========

/// Configuration for a recording session
#[derive(Debug, Clone)]
pub struct RecordingSessionConfig {
    pub segment_duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// Metadata for a finalized segment
#[derive(Debug, Clone)]
pub struct SegmentMetadata {
    pub duration_ms: u64,
    pub first_pts_ns: u64,
    pub last_pts_ns: u64,
    pub file_size_bytes: u64,
    pub video_frames: u64,
    pub audio_samples: u64,
}

/// A committed segment that is safe for recovery
#[derive(Debug, Clone)]
pub struct CommittedSegment {
    pub index: u32,
    pub path: std::path::PathBuf,
    pub duration_ms: u64,
    pub first_pts_ns: u64,
    pub last_pts_ns: u64,
    pub file_size_bytes: u64,
}

/// A crashed session with recoverable segments
#[derive(Debug, Clone)]
pub struct CrashedSession {
    pub session_id: String,
    pub output_dir: std::path::PathBuf,
    pub segment_duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub total_segments: u32,
    pub total_duration_ms: u64,
    pub created_at: i64,
    pub committed_segments: Vec<CommittedSegment>,
}

impl CrashedSession {
    /// Get recovered duration (sum of committed segments)
    pub fn recovered_duration_ms(&self) -> u64 {
        self.committed_segments.iter().map(|s| s.duration_ms).sum()
    }

    /// Get max potential loss (one segment duration)
    pub fn max_loss_ms(&self) -> u64 {
        self.segment_duration_ms
    }

    /// Format recovered duration as HH:MM:SS
    pub fn recovered_duration_string(&self) -> String {
        let total_ms = self.recovered_duration_ms();
        let total_secs = total_ms / 1000;
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, mins, secs)
        } else {
            format!("{}:{:02}", mins, secs)
        }
    }
}

/// CDJ device record from database
#[derive(Debug, Clone)]
pub struct CdjDeviceRecord {
    pub device_id: u8,
    pub name: String,
    pub ip: String,
    pub track_title: Option<String>,
    pub track_artist: Option<String>,
    pub is_master: bool,
    pub is_playing: bool,
    pub is_looping: bool,
    pub is_on_air: bool,
    pub bpm: Option<f32>,
    pub last_seen: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings() {
        // Use in-memory database for tests
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER)",
            [],
        ).unwrap();

        let db = Database { conn: Arc::new(Mutex::new(conn)) };

        db.set_setting("test_key", "test_value").unwrap();
        assert_eq!(db.get_setting("test_key"), Some("test_value".to_string()));

        db.set_float("gain", 2.5).unwrap();
        assert_eq!(db.get_float("gain", 1.0), 2.5);

        db.set_bool("visible", true).unwrap();
        assert_eq!(db.get_bool("visible", false), true);
    }

    #[test]
    fn test_osc_uv_level_persist() {
        // UV Level operator knob: absent setting -> 0.35 default; a persisted
        // value loads back through load_osc_config (same seam as global_brightness).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER)",
            [],
        ).unwrap();
        let db = Database { conn: Arc::new(Mutex::new(conn)) };

        assert!((db.load_osc_config().uv_level - 0.35).abs() < 1e-6);

        db.set_float("osc_uv_level", 0.6).unwrap();
        assert!((db.load_osc_config().uv_level - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_osc_enable_buildup_macro_persist() {
        // Buildup macro master arm: absent setting -> armed (the shipped
        // behavior); a disarm round-trips through save/load_osc_config so a
        // restart doesn't silently re-arm the macro mid-A/B.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER)",
            [],
        ).unwrap();
        let db = Database { conn: Arc::new(Mutex::new(conn)) };

        assert!(db.load_osc_config().enable_buildup_macro);

        let mut config = db.load_osc_config();
        config.enable_buildup_macro = false;
        db.save_osc_config(&config).unwrap();
        assert!(!db.load_osc_config().enable_buildup_macro);
    }

    #[test]
    fn test_lfo_bank_persist_roundtrip() {
        // COM-499 Phase B4: bank persists as a JSON blob in `settings`.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER)",
            [],
        ).unwrap();
        let db = Database { conn: Arc::new(Mutex::new(conn)) };

        // Absent blob -> default (8 disabled slots): never a wipe or panic.
        let empty = db.load_lfo_bank();
        assert!(empty.slots.iter().all(|s| !s.enabled));

        // A configured bank round-trips; the runtime phase is NOT persisted.
        let mut bank = crate::emerge::LfoBankConfig::default();
        bank.slots[0].enabled = true;
        bank.slots[0].target = crate::emerge::LfoTarget::Brightness;
        bank.slots[0].amplitude = 0.42;
        bank.slots[0].phase = 0.9; // runtime field — must not survive
        db.save_lfo_bank(&bank).unwrap();

        let back = db.load_lfo_bank();
        assert!(back.slots[0].enabled);
        assert_eq!(back.slots[0].target, crate::emerge::LfoTarget::Brightness);
        assert!((back.slots[0].amplitude - 0.42).abs() < 1e-6);
        assert_eq!(back.slots[0].phase, 0.0, "runtime phase must not persist");
    }

    #[test]
    fn test_stage_cache() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE cached_stages (stage_id INTEGER PRIMARY KEY, patch_json TEXT NOT NULL, saas_updated_at TEXT, cached_at INTEGER)",
            [],
        )
        .unwrap();
        let db = Database { conn: Arc::new(Mutex::new(conn)) };

        // Patch cache round-trips, keyed by stage; re-cache overwrites.
        assert_eq!(db.cached_stage_patch(7), None);
        db.cache_stage_patch(7, "{\"fixtures\":[]}", Some("2026-05-29T00:00:00Z")).unwrap();
        assert_eq!(db.cached_stage_patch(7).as_deref(), Some("{\"fixtures\":[]}"));
        db.cache_stage_patch(7, "{\"fixtures\":[1]}", None).unwrap();
        assert_eq!(db.cached_stage_patch(7).as_deref(), Some("{\"fixtures\":[1]}"));
        assert_eq!(db.cached_stage_patch(99), None);

        // Stage list + last-stage-id round-trip via settings.
        assert_eq!(db.cached_stage_list(), None);
        db.cache_stage_list("[{\"id\":7}]").unwrap();
        assert_eq!(db.cached_stage_list().as_deref(), Some("[{\"id\":7}]"));
        assert_eq!(db.last_stage_id(), None);
        db.set_last_stage_id(7).unwrap();
        assert_eq!(db.last_stage_id(), Some(7));

        // Slice 3 part 1: key->id map round-trips per stage.
        assert_eq!(db.cached_stage_id_map(7), None);
        db.cache_stage_id_map(7, "{\"wash-1\":42}").unwrap();
        assert_eq!(db.cached_stage_id_map(7).as_deref(), Some("{\"wash-1\":42}"));
    }

    #[test]
    fn test_active_cue_store() {
        // COM-499 decouple slice A: the local active-cue store is the renderer's
        // offline source of truth. Round-trip upsert, stage scoping, status flip,
        // origin-filtered reconcile listing, and prune.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE active_cues (launch_id TEXT PRIMARY KEY, stage_id INTEGER, status TEXT NOT NULL, cue_json TEXT NOT NULL, origin TEXT NOT NULL DEFAULT 'saas', promoted_to TEXT, updated_at INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE cue_definitions (def_key TEXT PRIMARY KEY, label TEXT NOT NULL, cue_json TEXT NOT NULL, stage_id INTEGER, updated_at INTEGER)",
            [],
        )
        .unwrap();
        let db = Database { conn: Arc::new(Mutex::new(conn)) };

        // Empty store: nothing for the stage.
        assert!(db.active_cue_jsons_for_stage(7, None).is_empty());

        // A SaaS cue and an operator-local cue, both scoped to stage 7.
        db.upsert_active_cue("L1", 7, "ACTIVE", "{\"launch_id\":\"L1\"}", "saas")
            .unwrap();
        db.upsert_active_cue("L2", 7, "LAUNCH_REQUESTED", "{\"launch_id\":\"L2\"}", "local")
            .unwrap();
        assert_eq!(db.active_cue_jsons_for_stage(7, None).len(), 2);
        assert!(db.active_cue_jsons_for_stage(99, None).is_empty());
        // Origin filter: the SaaS overlay vs the operator-local overlay.
        assert_eq!(db.active_cue_jsons_for_stage(7, Some("saas")).len(), 1);
        assert_eq!(db.active_cue_jsons_for_stage(7, Some("local")).len(), 1);

        // Upsert on the same launch_id overwrites (no duplicate row).
        db.upsert_active_cue("L1", 7, "STOP_REQUESTED", "{\"launch_id\":\"L1\",\"v\":2}", "saas")
            .unwrap();
        assert_eq!(db.active_cue_jsons_for_stage(7, None).len(), 2);

        // Reconcile listing: origin filter sees only the SaaS cue; None sees both.
        assert_eq!(db.active_cue_ids_for_stage(7, Some("saas")), vec!["L1".to_string()]);
        assert_eq!(db.active_cue_ids_for_stage(7, None).len(), 2);

        // Prune (server dropped it / it finished).
        db.remove_active_cue("L1").unwrap();
        assert_eq!(db.active_cue_ids_for_stage(7, None), vec!["L2".to_string()]);

        // Cross-stage prune on a stage switch: another stage's SaaS row is dropped,
        // but operator-local rows (any stage) and the current stage survive.
        db.upsert_active_cue("L3", 9, "ACTIVE", "{\"launch_id\":\"L3\"}", "saas")
            .unwrap();
        db.upsert_active_cue("L4", 9, "ACTIVE", "{\"launch_id\":\"L4\"}", "local")
            .unwrap();
        db.prune_saas_active_cues_other_stages(7).unwrap();
        assert!(db.active_cue_ids_for_stage(9, Some("saas")).is_empty()); // L3 dropped
        assert_eq!(db.active_cue_ids_for_stage(9, Some("local")), vec!["L4".to_string()]); // L4 kept
        assert_eq!(db.active_cue_ids_for_stage(7, None), vec!["L2".to_string()]); // stage 7 untouched

        // cache-from-launches definitions: round-trip + upsert (latest wins) + picker.
        assert!(db.cue_definitions_for_stage(7).is_empty());
        assert_eq!(db.cue_definition_json("gobo-cycle"), None);
        db.upsert_cue_definition("gobo-cycle", "Gobo Cycle", "{\"speed\":0.5}", 7)
            .unwrap();
        assert_eq!(db.cue_definition_json("gobo-cycle").as_deref(), Some("{\"speed\":0.5}"));
        db.upsert_cue_definition("gobo-cycle", "Gobo Cycle", "{\"speed\":0.8}", 7)
            .unwrap(); // latest launch wins
        assert_eq!(db.cue_definition_json("gobo-cycle").as_deref(), Some("{\"speed\":0.8}"));
        assert_eq!(
            db.cue_definitions_for_stage(7),
            vec![("gobo-cycle".to_string(), "Gobo Cycle".to_string())]
        );
        assert!(db.cue_definitions_for_stage(9).is_empty());

        // promote-on-connectivity: a local cue lists un-promoted, then carries its
        // server launch_id after promotion (so the next poll can swap it out).
        db.upsert_active_cue("Lp", 7, "ACTIVE", "{\"launch_id\":\"Lp\"}", "local")
            .unwrap();
        assert!(db
            .local_active_cues_for_promote(7)
            .iter()
            .any(|(lid, _, promoted)| lid == "Lp" && promoted.is_none()));
        db.set_active_cue_promoted("Lp", "server-xyz").unwrap();
        assert!(db
            .local_active_cues_for_promote(7)
            .iter()
            .any(|(lid, _, promoted)| lid == "Lp" && promoted.as_deref() == Some("server-xyz")));
    }

    /// The app's own schema on an in-memory database, then one recorded segment
    /// through the crash-recovery journal. Every statement in `init_schema`
    /// runs against the bundled SQLite, so a statement a newer SQLite or
    /// rusqlite rejects fails here instead of at the next launch.
    #[test]
    fn a_recorded_segment_round_trips_through_the_real_schema() {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn: Arc::new(Mutex::new(conn)) };
        db.init_schema().unwrap();
        // A second pass is what every launch after the first one does.
        db.init_schema().unwrap();

        let config = RecordingSessionConfig {
            segment_duration_ms: 30_000,
            width: 1920,
            height: 1080,
            fps: 60,
        };
        db.create_recording_session("session-1", "/recordings/out", &config).unwrap();
        db.begin_segment("session-1", 0, "segment-0.tmp", 1_000).unwrap();

        // Sizes past 32 bits: a long take's byte count and timestamps are.
        let metadata = SegmentMetadata {
            duration_ms: 29_970,
            first_pts_ns: 1_000,
            last_pts_ns: 29_970_001_000,
            file_size_bytes: 5_000_000_000,
            video_frames: 1_798,
            audio_samples: 1_438_560,
        };
        db.finalize_segment("session-1", 0, &metadata).unwrap();
        assert!(
            db.get_committed_segments("session-1").is_empty(),
            "a finalized segment is not yet safe to recover"
        );
        db.commit_segment("session-1", 0, "segment-0.mp4").unwrap();

        let segments = db.get_committed_segments("session-1");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].index, 0);
        assert_eq!(segments[0].path, std::path::PathBuf::from("segment-0.mp4"));
        assert_eq!(segments[0].duration_ms, 29_970);
        assert_eq!(segments[0].first_pts_ns, 1_000);
        assert_eq!(segments[0].last_pts_ns, 29_970_001_000);
        assert_eq!(segments[0].file_size_bytes, 5_000_000_000);

        let (video_frames, audio_samples): (i64, i64) = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT video_frames, audio_samples FROM segment_journal WHERE session_id = ?",
                ["session-1"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((video_frames, audio_samples), (1_798, 1_438_560));

        // Still 'active' is how a session that never stopped looks after a crash.
        let crashed = db.get_crashed_sessions();
        assert_eq!(crashed.len(), 1);
        assert_eq!(crashed[0].segment_duration_ms, 30_000);
        assert_eq!((crashed[0].width, crashed[0].height, crashed[0].fps), (1920, 1080, 60));
        assert_eq!(crashed[0].total_segments, 1);
        assert_eq!(crashed[0].total_duration_ms, 29_970);
        assert_eq!(crashed[0].recovered_duration_ms(), 29_970);
    }
}
