//! System audio capture using ScreenCaptureKit (macOS) and cpal for input devices
//!
//! Supports capturing from:
//! - System audio (all apps)
//! - Specific applications (Apple Music, Spotify, etc.)
//! - Audio input devices (microphones, audio interfaces)
//!
//! PERFORMANCE: Uses lock-free rtrb ring buffer for audio samples.
//! Audio callbacks push samples without locking, render loop drains.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use screencapturekit::prelude::*;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::audio_bus::{self, AudioBusProducers, AudioFrame, create_audio_bus, fanout};


/// Device profile for known audio input devices (DJ mixers, audio interfaces)
///
/// Specifies the optimal channel pair to extract and recording boost.
/// Unknown devices fall back to ch0/ch1 with boost 2.0.
#[derive(Clone, Debug)]
pub struct InputDeviceProfile {
    /// Display name for logs
    pub name: &'static str,
    /// Left channel index (0-based)
    pub left_channel: usize,
    /// Right channel index (0-based)
    pub right_channel: usize,
    /// Recording volume boost (lower for line-level devices)
    pub recording_boost: f32,
}

/// Default profile for unknown input devices
const DEFAULT_INPUT_PROFILE: InputDeviceProfile = InputDeviceProfile {
    name: "Generic Input",
    left_channel: 2,
    right_channel: 3,
    recording_boost: 2.0,
};

/// Known DJ mixer / audio interface profiles
/// Matched by substring in the device name (case-insensitive)
/// Channel defaults are fallbacks — the probe pass auto-detects the active pair.
const KNOWN_DEVICES: &[(&str, InputDeviceProfile)] = &[
    // Pioneer DJ mixers
    ("DJM-A9", InputDeviceProfile { name: "Pioneer DJM-A9", left_channel: 0, right_channel: 1, recording_boost: 2.0 }),
    ("DJM-V10", InputDeviceProfile { name: "Pioneer DJM-V10", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    ("DJM-900", InputDeviceProfile { name: "Pioneer DJM-900NXS2", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    ("DJM-750", InputDeviceProfile { name: "Pioneer DJM-750MK2", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    ("DJM-450", InputDeviceProfile { name: "Pioneer DJM-450", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    ("DJM-250", InputDeviceProfile { name: "Pioneer DJM-250MK2", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    // Allen & Heath DJ mixers
    ("Xone", InputDeviceProfile { name: "Allen & Heath Xone", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    ("MODEL 1", InputDeviceProfile { name: "Allen & Heath MODEL 1", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    // Native Instruments
    ("Traktor Audio", InputDeviceProfile { name: "NI Traktor Audio", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    // Denon DJ
    ("X1800", InputDeviceProfile { name: "Denon X1800", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    ("X1850", InputDeviceProfile { name: "Denon X1850", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    // Rane
    ("Seventy", InputDeviceProfile { name: "Rane Seventy", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    ("MP2015", InputDeviceProfile { name: "Rane MP2015", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
    // Generic USB audio interfaces (Focusrite, Universal Audio, etc.)
    ("Scarlett", InputDeviceProfile { name: "Focusrite Scarlett", left_channel: 2, right_channel: 3, recording_boost: 3.0 }),
    ("Apollo", InputDeviceProfile { name: "UA Apollo", left_channel: 2, right_channel: 3, recording_boost: 2.0 }),
];

/// Match a device name against known profiles
pub fn get_device_profile(device_name: &str) -> InputDeviceProfile {
    let name_lower = device_name.to_lowercase();
    for (pattern, profile) in KNOWN_DEVICES {
        if name_lower.contains(&pattern.to_lowercase()) {
            log::info!("[AudioCapture] Matched known device: {} ({})", profile.name, device_name);
            return profile.clone();
        }
    }
    log::info!("[AudioCapture] Unknown device '{}', using default profile (ch0/ch1, boost=2.0)", device_name);
    DEFAULT_INPUT_PROFILE
}

/// Audio source types
#[derive(Clone, Debug, PartialEq)]
pub enum AudioSource {
    /// Capture all system audio output
    SystemAudio,
    /// Capture audio from a specific application by bundle ID
    Application { bundle_id: String, name: String },
    /// Capture from an audio input device (microphone, audio interface)
    InputDevice { name: String, device_name: String },
    /// Legacy microphone option (redirects to default input)
    Microphone,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self::SystemAudio
    }
}

/// Information about an available audio source
#[derive(Clone, Debug)]
pub struct AudioSourceInfo {
    pub source: AudioSource,
    pub display_name: String,
}

/// Get list of available audio sources
pub fn get_available_sources() -> Vec<AudioSourceInfo> {
    let mut sources = vec![
        AudioSourceInfo {
            source: AudioSource::SystemAudio,
            display_name: "System Audio (All)".to_string(),
        },
    ];

    // Get running applications from ScreenCaptureKit
    if let Ok(content) = SCShareableContent::get() {
        let apps = content.applications();

        // Filter to common audio apps
        let audio_app_bundles = [
            ("com.apple.Music", "Apple Music"),
            ("com.spotify.client", "Spotify"),
            ("com.apple.Safari", "Safari"),
            ("com.google.Chrome", "Chrome"),
            ("org.mozilla.firefox", "Firefox"),
            ("com.brave.Browser", "Brave"),
            ("tv.plex.desktop", "Plex"),
            ("com.apple.TV", "Apple TV"),
            ("com.apple.QuickTimePlayerX", "QuickTime"),
            ("com.apple.podcasts", "Podcasts"),
            ("com.soundcloud.desktop", "SoundCloud"),
            ("com.tidal.desktop", "TIDAL"),
            ("com.audirvana.Audirvana-Plus", "Audirvana"),
            ("com.rogue-amoeba.Airfoil", "Airfoil"),
            ("us.zoom.xos", "Zoom"),
            ("com.microsoft.teams2", "Teams"),
            ("com.hnc.Discord", "Discord"),
            ("com.electron.reaper", "Reaper"),
            ("com.ableton.live", "Ableton Live"),
            ("com.native-instruments.Traktor", "Traktor"),
            ("com.algoriddim.djay-pro-mac", "djay Pro"),
            ("com.serato.seratodj", "Serato DJ"),
        ];

        for app in apps {
            let bundle_id = app.bundle_identifier();

            // Check if this is a known audio app
            if let Some((_, display_name)) = audio_app_bundles.iter()
                .find(|(id, _)| bundle_id.as_str() == *id)
            {
                sources.push(AudioSourceInfo {
                    source: AudioSource::Application {
                        bundle_id: bundle_id.clone(),
                        name: display_name.to_string(),
                    },
                    display_name: display_name.to_string(),
                });
            }
        }
    }

    // Add audio input devices using cpal
    let host = cpal::default_host();
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                // Skip aggregate devices and virtual devices that might cause issues
                if name.contains("Aggregate") || name.contains("CADefaultDevice") {
                    continue;
                }

                sources.push(AudioSourceInfo {
                    source: AudioSource::InputDevice {
                        name: name.clone(),
                        device_name: name.clone(),
                    },
                    display_name: format!("🎤 {}", name),
                });
            }
        }
    }

    sources
}

/// Get all running applications (for custom selection)
pub fn get_running_applications() -> Vec<AudioSourceInfo> {
    let mut sources = Vec::new();

    if let Ok(content) = SCShareableContent::get() {
        let apps = content.applications();

        for app in apps {
            let bundle_id = app.bundle_identifier();
            let app_name = app.application_name();

            // Skip system/background apps
            if bundle_id.is_empty() || app_name.is_empty() {
                continue;
            }

            sources.push(AudioSourceInfo {
                source: AudioSource::Application {
                    bundle_id: bundle_id.clone(),
                    name: app_name.clone(),
                },
                display_name: app_name,
            });
        }
    }

    // Sort by name
    sources.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
    sources
}

/// Active capture backend identifier for diagnostics
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaptureBackend {
    None,
    ScreenCaptureKit,
    GStreamer,
    Cpal,
}

impl std::fmt::Display for CaptureBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "NONE"),
            Self::ScreenCaptureKit => write!(f, "SCK"),
            Self::GStreamer => write!(f, "GST"),
            Self::Cpal => write!(f, "cpal"),
        }
    }
}

/// Audio diagnostics snapshot for UI overlay
#[derive(Clone, Debug)]
pub struct AudioDiagnostics {
    pub backend: CaptureBackend,
    pub sample_rate: u32,
    pub frames_received: u64,
    pub frames_dropped: u64,
}

/// Audio capture from system audio or specific app
///
/// PERFORMANCE: Uses lock-free rtrb ring buffers for zero-contention audio delivery.
/// AudioBus fans out to viz, meters, NDI, recorder, and RTMP consumers independently.
/// Each output drains at its own cadence — fully decoupled from the render loop.
pub struct AudioCapture {
    /// Current audio source
    source: AudioSource,

    /// Whether capture is running
    running: Arc<AtomicBool>,

    /// Sample rate
    pub sample_rate: u32,

    /// Lock-free ring buffer consumer for audio frames (visualization/render loop)
    sample_consumer: Option<rtrb::Consumer<AudioFrame>>,

    /// Lock-free ring buffer consumer for audio meters (dedicated path)
    meters_consumer: Option<rtrb::Consumer<AudioFrame>>,

    /// Output consumers — taken by OutputManager for independent drain threads
    ndi_consumer: Option<rtrb::Consumer<AudioFrame>>,
    rec_consumer: Option<rtrb::Consumer<AudioFrame>>,
    rtmp_consumer: Option<rtrb::Consumer<AudioFrame>>,

    /// Timestamp of last received audio (milliseconds since epoch)
    last_audio_time: Arc<AtomicU64>,

    /// Actual detected sample rate (may differ from requested rate)
    detected_sample_rate: Arc<AtomicU32>,

    /// cpal input stream (for input device capture)
    cpal_stream: Option<cpal::Stream>,

    /// Cached shareable content from async prepare phase
    prepared_content: Arc<parking_lot::Mutex<Option<SCShareableContent>>>,

    /// Whether async preparation is complete
    prepare_complete: Arc<AtomicBool>,

    /// Counter for ring buffer overflow events (samples dropped due to full buffer)
    overflow_count: Arc<AtomicU64>,

    /// GStreamer capture pipeline (for input devices, replaces cpal)
    gst_capture: Option<super::gst_capture::GstAudioCapture>,

    /// Active capture backend (for diagnostics)
    capture_backend: CaptureBackend,

    /// Total stereo frames received by capture callbacks (monotonically increasing)
    frames_received: Arc<AtomicU64>,
}

impl AudioCapture {
    /// Create a new audio capture with default source (system audio)
    pub fn new() -> Self {
        Self {
            source: AudioSource::default(),
            running: Arc::new(AtomicBool::new(false)),
            sample_rate: 48000,
            sample_consumer: None,
            meters_consumer: None,
            ndi_consumer: None,
            rec_consumer: None,
            rtmp_consumer: None,
            last_audio_time: Arc::new(AtomicU64::new(0)),
            detected_sample_rate: Arc::new(AtomicU32::new(48000)),
            cpal_stream: None,
            prepared_content: Arc::new(parking_lot::Mutex::new(None)),
            prepare_complete: Arc::new(AtomicBool::new(false)),
            overflow_count: Arc::new(AtomicU64::new(0)),
            gst_capture: None,
            capture_backend: CaptureBackend::None,
            frames_received: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get current audio diagnostics snapshot
    pub fn diagnostics(&self) -> AudioDiagnostics {
        AudioDiagnostics {
            backend: self.capture_backend,
            sample_rate: self.get_detected_sample_rate(),
            frames_received: self.frames_received.load(Ordering::Relaxed),
            frames_dropped: self.overflow_count.load(Ordering::Relaxed),
        }
    }

    /// Get the number of overflow events (samples dropped due to full ring buffer)
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count.load(Ordering::Relaxed)
    }

    /// Take the output consumers (NDI, recorder, RTMP) for independent drain threads.
    /// Returns (ndi, rec, rtmp) consumers. Can only be called once per capture session.
    pub fn take_output_consumers(&mut self) -> (
        Option<rtrb::Consumer<AudioFrame>>,
        Option<rtrb::Consumer<AudioFrame>>,
        Option<rtrb::Consumer<AudioFrame>>,
    ) {
        (self.ndi_consumer.take(), self.rec_consumer.take(), self.rtmp_consumer.take())
    }

    /// Start async preparation of SCShareableContent (call early, runs in background)
    ///
    /// This queries SCShareableContent::get() in a background thread to avoid
    /// blocking the main thread during startup. The result is cached for use
    /// when start() is called later.
    pub fn prepare_async(&self) {
        let content_cache = self.prepared_content.clone();
        let complete_flag = self.prepare_complete.clone();

        std::thread::spawn(move || {
            log::info!("[AudioCapture] Starting async prepare (SCShareableContent query)...");
            match SCShareableContent::get() {
                Ok(content) => {
                    *content_cache.lock() = Some(content);
                    log::info!("[AudioCapture] Async prepare complete - content cached");
                }
                Err(e) => {
                    log::error!("[AudioCapture] Async prepare failed: {:?}", e);
                }
            }
            complete_flag.store(true, Ordering::SeqCst);
        });
    }

    /// Check if async preparation is complete
    pub fn is_prepared(&self) -> bool {
        self.prepare_complete.load(Ordering::SeqCst)
    }

    /// Take the meters audio consumer (can only be called once per capture session)
    /// Returns the consumer for fundsp-based audio metering (frame-based)
    pub fn take_meters_consumer(&mut self) -> Option<rtrb::Consumer<[f32; 2]>> {
        self.meters_consumer.take()
    }

    /// Get the detected audio sample rate (updated from actual SCK format description)
    /// This may differ from the requested sample_rate if the system uses a different rate
    pub fn get_detected_sample_rate(&self) -> u32 {
        self.detected_sample_rate.load(Ordering::SeqCst)
    }

    /// Drain available audio frames from the lock-free ring buffer.
    ///
    /// PERFORMANCE: This is the preferred way to get audio samples.
    /// Call from render loop - no blocking, no contention with audio thread.
    ///
    /// FRAME-BASED: Ring buffer stores [f32; 2] frames. Each pop is a complete
    /// stereo frame — stereo desync is structurally impossible.
    /// Output is interleaved [L, R, L, R, ...] for compatibility with downstream.
    ///
    /// Returns the number of samples drained (always even).
    pub fn drain_samples(&mut self, out: &mut Vec<f32>) -> usize {
        let mut count = 0;
        if let Some(ref mut consumer) = self.sample_consumer {
            while let Ok(frame) = consumer.pop() {
                out.push(frame[0]);
                out.push(frame[1]);
                count += 2;
            }
        }
        count
    }

    /// Get current timestamp in milliseconds
    fn now_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Check if audio capture has gone stale (no samples for given seconds)
    /// Returns true if capture is running but no audio received for `timeout_secs`
    pub fn is_stale(&self, timeout_secs: u64) -> bool {
        if !self.running.load(Ordering::SeqCst) {
            return false;
        }

        let last = self.last_audio_time.load(Ordering::SeqCst);
        if last == 0 {
            // Haven't received any audio yet - give it some time to start
            return false;
        }

        let now = Self::now_millis();
        let elapsed_secs = (now.saturating_sub(last)) / 1000;
        elapsed_secs >= timeout_secs
    }

    /// Reconnect audio capture (stop and restart)
    pub fn reconnect(&mut self) -> Result<()> {
        log::info!("[AudioCapture] Reconnecting audio capture...");
        self.stop();
        std::thread::sleep(std::time::Duration::from_millis(200));
        self.start()
    }

    /// Set the audio source
    pub fn set_source(&mut self, source: AudioSource) {
        // If running, we need to restart with new source
        let was_running = self.is_running();
        if was_running {
            self.stop();
        }

        self.source = source;

        if was_running {
            let _ = self.start();
        }
    }

    /// Get current audio source
    pub fn get_source(&self) -> &AudioSource {
        &self.source
    }

    /// Start capturing audio from the selected source
    pub fn start(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            log::warn!("[AudioCapture] Already running");
            return Ok(());
        }

        // Clone source to avoid borrow issues
        let source = self.source.clone();
        match source {
            AudioSource::SystemAudio => self.start_system_audio(),
            AudioSource::Application { bundle_id, name } => {
                match self.start_app_audio(&bundle_id, &name) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        log::warn!("[AudioCapture] App capture failed ({}), falling back to system audio", e);
                        self.start_system_audio()
                    }
                }
            }
            AudioSource::InputDevice { name, device_name } => {
                self.start_input_device(&device_name)
            }
            AudioSource::Microphone => {
                // Use default input device
                self.start_default_input()
            }
        }
    }

    /// Start capturing all system audio
    fn start_system_audio(&mut self) -> Result<()> {
        log::info!("[AudioCapture] Starting system audio capture");

        // Use cached content from prepare_async() if available (non-blocking path)
        // Otherwise fall back to synchronous query (blocking)
        let content = self.prepared_content.lock().take()
            .map(Ok)
            .unwrap_or_else(|| {
                log::info!("[AudioCapture] No cached content, querying SCShareableContent now (blocking)...");
                SCShareableContent::get()
                    .map_err(|e| anyhow!("Failed to get shareable content: {:?}. Grant Screen Recording permission.", e))
            })?;

        // Get first display
        let displays = content.displays();
        let display = displays.first()
            .ok_or_else(|| anyhow!("No display found"))?;

        // Create filter for the display (audio-only)
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build();

        self.start_capture_with_filter(filter)
    }

    /// Start capturing audio from a specific application
    fn start_app_audio(&mut self, bundle_id: &str, name: &str) -> Result<()> {
        log::info!("[AudioCapture] Starting audio capture for app: {} ({})", name, bundle_id);

        // Get shareable content
        let content = SCShareableContent::get()
            .map_err(|e| anyhow!("Failed to get shareable content: {:?}", e))?;

        // Find the application
        let apps = content.applications();
        let _app = apps.iter()
            .find(|a| a.bundle_identifier() == bundle_id)
            .ok_or_else(|| anyhow!("Application '{}' not running", name))?;

        // Get windows for this application (needed for content filter)
        let windows = content.windows();
        let app_windows: Vec<&SCWindow> = windows.iter()
            .filter(|w| {
                if let Some(owning_app) = w.owning_application() {
                    owning_app.bundle_identifier() == bundle_id
                } else {
                    false
                }
            })
            .collect();

        // Create content filter - need display + app or windows
        let displays = content.displays();
        let display = displays.first()
            .ok_or_else(|| anyhow!("No display found"))?;

        // Use the display with app filtering
        // Note: ScreenCaptureKit app-specific audio requires including the app's windows
        let filter = if !app_windows.is_empty() {
            // Filter to just this app's windows - exclude everything except app windows
            let excluding: Vec<&SCWindow> = windows.iter()
                .filter(|w| !app_windows.iter().any(|aw| std::ptr::eq(*aw, *w)))
                .collect();

            SCContentFilter::create()
                .with_display(display)
                .with_excluding_windows(&excluding)
                .build()
        } else {
            // App has no windows, fall back to display filter
            // This may capture all audio - not ideal but better than nothing
            log::warn!("[AudioCapture] App '{}' has no windows, capturing all audio", name);
            SCContentFilter::create()
                .with_display(display)
                .with_excluding_windows(&[])
                .build()
        };

        self.start_capture_with_filter(filter)
    }

    /// Start capture with a given content filter
    fn start_capture_with_filter(&mut self, filter: SCContentFilter) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        self.capture_backend = CaptureBackend::ScreenCaptureKit;
        self.frames_received.store(0, Ordering::Relaxed);
        self.last_audio_time.store(0, Ordering::SeqCst);

        log::info!("[AudioCapture] SCK capture starting — delivering: {} Hz, 2 ch, F32 interleaved", self.sample_rate);

        let config = SCStreamConfiguration::new()
            .with_width(64)
            .with_height(64)
            .with_captures_audio(true)
            .with_excludes_current_process_audio(false)
            .with_sample_rate(self.sample_rate as i32)
            .with_channel_count(2);

        // Create AudioBus: 5 ring buffers (viz, meters, ndi, rec, rtmp)
        let (producers, consumers) = create_audio_bus();
        self.sample_consumer = Some(consumers.viz);
        self.meters_consumer = Some(consumers.meters);
        self.ndi_consumer = Some(consumers.ndi);
        self.rec_consumer = Some(consumers.rec);
        self.rtmp_consumer = Some(consumers.rtmp);

        let running = self.running.clone();
        let last_audio_time = self.last_audio_time.clone();
        let detected_sample_rate = self.detected_sample_rate.clone();
        let frames_received = self.frames_received.clone();

        std::thread::spawn(move || {
            if let Err(e) = run_capture_loop(filter, config, producers, running, last_audio_time, detected_sample_rate, frames_received) {
                log::error!("[AudioCapture] Capture error: {}", e);
            }
        });

        Ok(())
    }

    /// Start capturing from default input device.
    ///
    /// Strategy: GStreamer primary for multi-channel (proper USB clock recovery),
    /// cpal primary for stereo (simple, low-latency).
    fn start_default_input(&mut self) -> Result<()> {
        log::info!("[AudioCapture] Starting default input device capture");

        let host = cpal::default_host();
        let device = host.default_input_device()
            .ok_or_else(|| anyhow!("No default input device available"))?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
        log::info!("[AudioCapture] Using default input: {}", device_name);

        let config = device.default_input_config()
            .map_err(|e| anyhow!("Failed to get default input config: {:?}", e))?;
        let in_channels = config.channels() as usize;
        let profile = get_device_profile(&device_name);

        if in_channels > 2 {
            // Multi-channel: GStreamer primary (cpal under-delivers for multi-ch USB)
            log::info!("[AudioCapture] Multi-channel default input ({} ch) — using GStreamer primary ({})",
                in_channels, profile.name);
            match self.start_gst_capture(&device_name, &profile, in_channels) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("[AudioCapture] GStreamer failed for default input: {}, falling back to cpal", e);
                }
            }
            match self.start_cpal_capture(device, Some(profile)) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    anyhow::bail!("Both GStreamer and cpal failed for default input: {}", e);
                }
            }
        } else {
            // Stereo: cpal primary
            match self.start_cpal_capture(device, Some(profile.clone())) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("[AudioCapture] cpal failed for default input: {}, falling back to GStreamer", e);
                }
            }
            self.start_gst_capture(&device_name, &profile, in_channels)
        }
    }

    /// Start capturing from a specific input device by name.
    ///
    /// Strategy: GStreamer primary for multi-channel USB devices (proper clock recovery —
    /// cpal's CoreAudio IOProc under-delivers ~40kHz instead of 48kHz for multi-channel USB).
    /// cpal primary for stereo devices (simple, low-latency IOProc).
    fn start_input_device(&mut self, device_name: &str) -> Result<()> {
        log::info!("[AudioCapture] Starting input device capture: {}", device_name);

        let host = cpal::default_host();
        let device = host.input_devices()
            .map_err(|e| anyhow!("Failed to enumerate input devices: {:?}", e))?
            .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
            .ok_or_else(|| anyhow!("Input device '{}' not found", device_name))?;

        // Probe channel count from cpal (quick, no stream opened)
        let config = device.default_input_config()
            .map_err(|e| anyhow!("Failed to get default input config: {:?}", e))?;
        let in_channels = config.channels() as usize;
        let profile = get_device_profile(device_name);

        if in_channels > 2 {
            // Multi-channel USB devices: GStreamer primary (cpal's CoreAudio IOProc
            // under-delivers samples for multi-channel USB audio — ~40kHz instead of 48kHz).
            // GStreamer's osxaudiosrc uses AudioUnit with proper clock recovery.
            log::info!("[AudioCapture] Multi-channel device ({} ch) — using GStreamer primary ({})",
                in_channels, profile.name);
            match self.start_gst_capture(device_name, &profile, in_channels) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("[AudioCapture] GStreamer failed for '{}': {}, falling back to cpal", device_name, e);
                }
            }
            // Fallback: cpal with inline channel detector
            match self.start_cpal_capture(device, Some(profile)) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    anyhow::bail!("Both GStreamer and cpal failed for '{}': {}", device_name, e);
                }
            }
        } else {
            // Stereo devices: cpal primary (simple, low-latency)
            match self.start_cpal_capture(device, Some(profile.clone())) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::warn!("[AudioCapture] cpal failed for '{}': {}, falling back to GStreamer", device_name, e);
                }
            }
            self.start_gst_capture(device_name, &profile, in_channels)
        }
    }

    /// Start GStreamer-based capture for an input device.
    ///
    /// Creates AudioBus, builds a GStreamer pipeline with osxaudiosrc,
    /// and configures the appsink to fanout to all ring buffers.
    fn start_gst_capture(&mut self, device_name: &str, profile: &InputDeviceProfile, in_channels: usize) -> Result<()> {
        let (producers, consumers) = create_audio_bus();

        let gst_capture = super::gst_capture::GstAudioCapture::start(
            device_name,
            profile,
            in_channels,
            producers,
            self.last_audio_time.clone(),
            self.overflow_count.clone(),
            self.frames_received.clone(),
        )?;

        self.running.store(true, Ordering::SeqCst);
        self.capture_backend = CaptureBackend::GStreamer;
        self.frames_received.store(0, Ordering::Relaxed);
        self.last_audio_time.store(0, Ordering::SeqCst);
        self.overflow_count.store(0, Ordering::Relaxed);
        self.sample_rate = 48000;
        self.sample_consumer = Some(consumers.viz);
        self.meters_consumer = Some(consumers.meters);
        self.ndi_consumer = Some(consumers.ndi);
        self.rec_consumer = Some(consumers.rec);
        self.rtmp_consumer = Some(consumers.rtmp);
        self.gst_capture = Some(gst_capture);

        log::info!("[AudioCapture] GStreamer capture started for '{}' — delivering: 48000 Hz, 2 ch, F32 interleaved", device_name);
        Ok(())
    }

    /// Start cpal capture from a device
    ///
    /// For multi-channel devices (DJ mixers), extracts the specific L/R master output
    /// channels defined in the device profile (or probe override) and pushes stereo to all ring buffers.
    /// For mono/stereo devices, audio goes directly to all ring buffers.
    ///
    /// `profile_override`: If Some, use this profile's channel mapping instead of the
    /// default lookup. Used when the GStreamer probe pass has detected the active pair.
    fn start_cpal_capture(&mut self, device: cpal::Device, profile_override: Option<InputDeviceProfile>) -> Result<()> {
        // Get supported config
        let config = device.default_input_config()
            .map_err(|e| anyhow!("Failed to get default input config: {:?}", e))?;

        // Canonical output rate: all backends deliver 48kHz to ring buffers.
        // Native device rate is captured in `sample_rate` local for resampler config.
        self.sample_rate = 48000;

        // === DIAGNOSTICS: Log full device info ===
        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
        log::info!("[AudioCapture] === INPUT DEVICE DIAGNOSTICS ===");
        log::info!("[AudioCapture]   device_name: {}", device_name);
        log::info!("[AudioCapture]   sample_rate: {} Hz", config.sample_rate().0);
        log::info!("[AudioCapture]   channels: {}", config.channels());
        log::info!("[AudioCapture]   sample_format: {:?}", config.sample_format());
        log::info!("[AudioCapture]   buffer_size: {:?}", config.buffer_size());
        if let Ok(supported) = device.supported_input_configs() {
            for (i, cfg) in supported.enumerate() {
                log::info!("[AudioCapture]   supported_config[{}]: channels={}, rate={:?}-{:?}, fmt={:?}",
                    i, cfg.channels(), cfg.min_sample_rate(), cfg.max_sample_rate(), cfg.sample_format());
            }
        }
        log::info!("[AudioCapture] ================================");

        self.running.store(true, Ordering::SeqCst);
        self.capture_backend = CaptureBackend::Cpal;
        self.frames_received.store(0, Ordering::Relaxed);
        self.last_audio_time.store(0, Ordering::SeqCst);
        self.overflow_count.store(0, Ordering::Relaxed);

        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate().0;
        let sample_format = config.sample_format();
        let needs_resample = sample_rate != 48000;

        if needs_resample {
            log::info!("[AudioCapture] cpal: device rate {}Hz != 48kHz — will resample (linear interpolation)", sample_rate);
        } else {
            log::info!("[AudioCapture] cpal: device rate matches canonical 48kHz — direct passthrough");
        }

        // Use probe-detected profile if available, otherwise look up by device name
        let profile = profile_override.unwrap_or_else(|| get_device_profile(&device_name));

        // Create AudioBus: 5 ring buffers for all destinations
        let (producers, consumers) = create_audio_bus();
        self.sample_consumer = Some(consumers.viz);
        self.meters_consumer = Some(consumers.meters);
        self.ndi_consumer = Some(consumers.ndi);
        self.rec_consumer = Some(consumers.rec);
        self.rtmp_consumer = Some(consumers.rtmp);

        let last_audio_time = self.last_audio_time.clone();
        let running = self.running.clone();
        let frames_received = self.frames_received.clone();
        let callback_count = Arc::new(AtomicU64::new(0));

        let err_fn = |err| log::error!("[AudioCapture] cpal stream error: {:?}", err);

        if channels > 2 {
            // === MULTI-CHANNEL: Extract specific L/R channels from device profile ===
            let left_ch = profile.left_channel;
            let right_ch = profile.right_channel;
            log::info!("[AudioCapture] Multi-channel device ({} ch) - extracting channels [{}, {}] (profile: {})",
                channels, left_ch, right_ch, profile.name);

            if left_ch >= channels || right_ch >= channels {
                log::warn!("[AudioCapture] Profile channels [{}, {}] out of range for {}-channel device, using [0, 1]",
                    left_ch, right_ch, channels);
            }

            let mut producers = producers;

            let stream = match sample_format {
                cpal::SampleFormat::F32 => {
                    let cb_count = callback_count.clone();
                    let mut resampler = if needs_resample {
                        Some(LinearResampler::new(sample_rate, 48000))
                    } else {
                        None
                    };
                    let mut resample_input: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut resample_output: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut interleaved: Vec<f32> = Vec::with_capacity(8192);

                    let mut detector = ChannelDetector::new(channels, left_ch, right_ch);

                    device.build_input_stream(
                        &config.into(),
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if !running.load(Ordering::SeqCst) {
                                return;
                            }
                            let count = cb_count.fetch_add(1, Ordering::Relaxed);
                            last_audio_time.store(now_millis(), Ordering::SeqCst);
                            let num_frames = data.len() / channels;

                            // Feed detector during detection phase
                            detector.feed(data, channels, num_frames);
                            let (active_l, active_r) = detector.active_pair();

                            if count < 5 {
                                log::info!("[AudioCapture] multichan callback #{}: {} samples, ch[{},{}]",
                                    count, data.len(), active_l, active_r);
                            }

                            interleaved.clear();

                            if let Some(ref mut rs) = resampler {
                                resample_input.clear();
                                for f in 0..num_frames {
                                    let base = f * channels;
                                    resample_input.push([data[base + active_l], data[base + active_r]]);
                                }
                                let resampled = rs.process(&resample_input);
                                resample_output.clear();
                                resample_output.extend_from_slice(resampled);
                                for &frame in &resample_output {
                                    interleaved.push(frame[0]);
                                    interleaved.push(frame[1]);
                                }
                            } else {
                                for f in 0..num_frames {
                                    let base = f * channels;
                                    interleaved.push(data[base + active_l]);
                                    interleaved.push(data[base + active_r]);
                                }
                            }
                            fanout(&mut producers, &interleaved);
                            frames_received.fetch_add((interleaved.len() / 2) as u64, Ordering::Relaxed);
                        },
                        err_fn,
                        None,
                    )
                }
                cpal::SampleFormat::I16 => {
                    let running2 = self.running.clone();
                    let last_audio_time2 = self.last_audio_time.clone();
                    let frames_received2 = self.frames_received.clone();
                    let cb_count2 = callback_count.clone();
                    let mut resampler2 = if needs_resample {
                        Some(LinearResampler::new(sample_rate, 48000))
                    } else {
                        None
                    };
                    let mut resample_input2: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut resample_output2: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut interleaved2: Vec<f32> = Vec::with_capacity(8192);
                    let mut detector2 = ChannelDetector::new(channels, left_ch, right_ch);

                    device.build_input_stream(
                        &config.into(),
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if !running2.load(Ordering::SeqCst) {
                                return;
                            }
                            let count = cb_count2.fetch_add(1, Ordering::Relaxed);
                            last_audio_time2.store(now_millis(), Ordering::SeqCst);
                            let num_frames = data.len() / channels;

                            detector2.feed_i16(data, channels, num_frames);
                            let (active_l, active_r) = detector2.active_pair();

                            if count < 5 {
                                log::info!("[AudioCapture] multichan i16 callback #{}: {} samples, ch[{},{}]",
                                    count, data.len(), active_l, active_r);
                            }

                            interleaved2.clear();

                            if let Some(ref mut rs) = resampler2 {
                                resample_input2.clear();
                                for f in 0..num_frames {
                                    let base = f * channels;
                                    resample_input2.push([
                                        data[base + active_l] as f32 / 32768.0,
                                        data[base + active_r] as f32 / 32768.0,
                                    ]);
                                }
                                let resampled = rs.process(&resample_input2);
                                resample_output2.clear();
                                resample_output2.extend_from_slice(resampled);
                                for &frame in &resample_output2 {
                                    interleaved2.push(frame[0]);
                                    interleaved2.push(frame[1]);
                                }
                            } else {
                                for f in 0..num_frames {
                                    let base = f * channels;
                                    interleaved2.push(data[base + active_l] as f32 / 32768.0);
                                    interleaved2.push(data[base + active_r] as f32 / 32768.0);
                                }
                            }
                            fanout(&mut producers, &interleaved2);
                            frames_received2.fetch_add((interleaved2.len() / 2) as u64, Ordering::Relaxed);
                        },
                        err_fn,
                        None,
                    )
                }
                _ => {
                    return Err(anyhow!("Unsupported sample format: {:?}", sample_format));
                }
            }.map_err(|e| anyhow!("Failed to build input stream: {:?}", e))?;

            stream.play().map_err(|e| anyhow!("Failed to start stream: {:?}", e))?;
            self.cpal_stream = Some(stream);

        } else {
            // === MONO/STEREO: direct path ===
            log::info!("[AudioCapture] Device channels: {} (profile: {})", channels, profile.name);

            let mut producers = producers;

            let stream = match sample_format {
                cpal::SampleFormat::F32 => {
                    let cb_count = callback_count.clone();
                    let mut resampler = if needs_resample {
                        Some(LinearResampler::new(sample_rate, 48000))
                    } else {
                        None
                    };
                    let mut resample_input: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut resample_output: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut interleaved: Vec<f32> = Vec::with_capacity(8192);

                    device.build_input_stream(
                        &config.into(),
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if !running.load(Ordering::SeqCst) {
                                return;
                            }
                            let count = cb_count.fetch_add(1, Ordering::Relaxed);
                            if count < 5 {
                                log::info!("[AudioCapture] callback #{}: {} samples, {} ch",
                                    count, data.len(), channels);
                            }
                            last_audio_time.store(now_millis(), Ordering::SeqCst);
                            interleaved.clear();

                            if let Some(ref mut rs) = resampler {
                                resample_input.clear();
                                if channels == 1 {
                                    for &s in data { resample_input.push([s, s]); }
                                } else {
                                    for chunk in data.chunks_exact(2) { resample_input.push([chunk[0], chunk[1]]); }
                                }
                                let resampled = rs.process(&resample_input);
                                resample_output.clear();
                                resample_output.extend_from_slice(resampled);
                                for &frame in &resample_output {
                                    interleaved.push(frame[0]);
                                    interleaved.push(frame[1]);
                                }
                            } else {
                                if channels == 1 {
                                    for &s in data { interleaved.push(s); interleaved.push(s); }
                                } else {
                                    interleaved.extend_from_slice(data);
                                }
                            }
                            fanout(&mut producers, &interleaved);
                            frames_received.fetch_add((interleaved.len() / 2) as u64, Ordering::Relaxed);
                        },
                        err_fn,
                        None,
                    )
                }
                cpal::SampleFormat::I16 => {
                    let running2 = self.running.clone();
                    let last_audio_time2 = self.last_audio_time.clone();
                    let frames_received2 = self.frames_received.clone();
                    let cb_count2 = callback_count.clone();
                    let mut resampler2 = if needs_resample {
                        Some(LinearResampler::new(sample_rate, 48000))
                    } else {
                        None
                    };
                    let mut resample_input2: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut resample_output2: Vec<[f32; 2]> = Vec::with_capacity(4096);
                    let mut interleaved2: Vec<f32> = Vec::with_capacity(8192);

                    device.build_input_stream(
                        &config.into(),
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if !running2.load(Ordering::SeqCst) {
                                return;
                            }
                            let count = cb_count2.fetch_add(1, Ordering::Relaxed);
                            if count < 5 {
                                log::info!("[AudioCapture] i16 callback #{}: {} samples, {} ch",
                                    count, data.len(), channels);
                            }
                            last_audio_time2.store(now_millis(), Ordering::SeqCst);
                            interleaved2.clear();

                            if let Some(ref mut rs) = resampler2 {
                                resample_input2.clear();
                                if channels == 1 {
                                    for &s in data {
                                        let f = s as f32 / 32768.0;
                                        resample_input2.push([f, f]);
                                    }
                                } else {
                                    for chunk in data.chunks_exact(2) {
                                        resample_input2.push([
                                            chunk[0] as f32 / 32768.0,
                                            chunk[1] as f32 / 32768.0,
                                        ]);
                                    }
                                }
                                let resampled = rs.process(&resample_input2);
                                resample_output2.clear();
                                resample_output2.extend_from_slice(resampled);
                                for &frame in &resample_output2 {
                                    interleaved2.push(frame[0]);
                                    interleaved2.push(frame[1]);
                                }
                            } else {
                                if channels == 1 {
                                    for &s in data {
                                        let f = s as f32 / 32768.0;
                                        interleaved2.push(f);
                                        interleaved2.push(f);
                                    }
                                } else {
                                    for chunk in data.chunks_exact(2) {
                                        interleaved2.push(chunk[0] as f32 / 32768.0);
                                        interleaved2.push(chunk[1] as f32 / 32768.0);
                                    }
                                }
                            }
                            fanout(&mut producers, &interleaved2);
                            frames_received2.fetch_add((interleaved2.len() / 2) as u64, Ordering::Relaxed);
                        },
                        err_fn,
                        None,
                    )
                }
                _ => {
                    return Err(anyhow!("Unsupported sample format: {:?}", sample_format));
                }
            }.map_err(|e| anyhow!("Failed to build input stream: {:?}", e))?;

            stream.play().map_err(|e| anyhow!("Failed to start stream: {:?}", e))?;
            self.cpal_stream = Some(stream);
        }

        log::info!("[AudioCapture] cpal capture started — delivering: 48000 Hz (canonical), 2 ch, F32 interleaved (device: {} Hz, {} ch, {:?}{})",
            sample_rate, channels, sample_format,
            if needs_resample { ", resampled" } else { "" });
        Ok(())
    }

    /// Stop capturing
    pub fn stop(&mut self) {
        log::info!("[AudioCapture] Stopping audio capture");
        self.running.store(false, Ordering::SeqCst);
        self.capture_backend = CaptureBackend::None;

        // Stop GStreamer pipeline if active
        if let Some(mut gst) = self.gst_capture.take() {
            gst.stop();
            log::info!("[AudioCapture] GStreamer pipeline stopped");
        }

        // Stop cpal stream if active
        if let Some(stream) = self.cpal_stream.take() {
            drop(stream);
            log::info!("[AudioCapture] cpal stream stopped");
        }

        // Give the capture thread time to stop
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Clear ring buffer by dropping consumer
        self.sample_consumer = None;
    }

    /// Check if capture is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get latest audio samples (stereo interleaved)
    /// DEPRECATED: Use drain_samples() instead for lock-free operation
    pub fn get_samples(&mut self, max_samples: usize) -> Vec<f32> {
        let max_frames = max_samples / 2;
        let mut samples = Vec::with_capacity(max_frames * 2);
        if let Some(ref mut consumer) = self.sample_consumer {
            for _ in 0..max_frames {
                match consumer.pop() {
                    Ok(frame) => {
                        samples.push(frame[0]);
                        samples.push(frame[1]);
                    }
                    Err(_) => break,
                }
            }
        }
        samples
    }

    /// Get approximate buffer length (samples available to read)
    pub fn buffer_len(&self) -> usize {
        self.sample_consumer.as_ref().map(|c| c.slots() * 2).unwrap_or(0)
    }
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Get current timestamp in milliseconds (for capture thread)
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Linear-interpolation resampler for converting SCK/cpal audio to canonical 48kHz.
///
/// Maintains fractional phase across calls for seamless block-boundary transitions.
/// Cost: O(n) per block, no allocations after warmup (reuses output buffer).
struct LinearResampler {
    /// input_rate / output_rate (e.g., 44100/48000 = 0.91875)
    increment: f64,
    /// Fractional position in input space (carried across calls)
    phase: f64,
    /// Last frame from previous call (for interpolation across block boundaries)
    last_frame: [f32; 2],
    /// Reusable output buffer
    output: Vec<[f32; 2]>,
}

impl LinearResampler {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        log::info!("[Resampler] Created: {} Hz → {} Hz (ratio={:.6})",
            input_rate, output_rate, input_rate as f64 / output_rate as f64);
        Self {
            increment: input_rate as f64 / output_rate as f64,
            phase: 0.0,
            last_frame: [0.0; 2],
            output: Vec::with_capacity(4096),
        }
    }

    /// Resample a block of stereo frames.
    ///
    /// Phase convention: `phase` is the position in the current input block.
    /// Index -1 maps to `self.last_frame` (saved from previous call).
    /// After processing, phase is adjusted for the next call.
    fn process(&mut self, input: &[[f32; 2]]) -> &[[f32; 2]] {
        self.output.clear();
        if input.is_empty() {
            return &self.output;
        }

        let n = input.len();

        // Generate output frames while we can interpolate between two input frames
        while self.phase < (n - 1) as f64 {
            let idx = self.phase.floor() as isize;
            let frac = (self.phase - idx as f64) as f32;

            let frame_a = if idx < 0 {
                self.last_frame
            } else {
                input[idx as usize]
            };
            let frame_b = input[(idx + 1).max(0) as usize];

            self.output.push([
                frame_a[0] + (frame_b[0] - frame_a[0]) * frac,
                frame_a[1] + (frame_b[1] - frame_a[1]) * frac,
            ]);
            self.phase += self.increment;
        }

        // Carry phase into next call (subtract consumed input length)
        self.phase -= n as f64;
        // Save last frame for next call's boundary interpolation
        self.last_frame = input[n - 1];

        &self.output
    }
}

/// Run the SCK capture loop in a separate thread.
///
/// Uses AudioBus fanout: all 5 consumers get frames independently.
fn run_capture_loop(
    filter: SCContentFilter,
    config: SCStreamConfiguration,
    producers: AudioBusProducers,
    running: Arc<AtomicBool>,
    last_audio_time: Arc<AtomicU64>,
    detected_sample_rate: Arc<AtomicU32>,
    frames_received: Arc<AtomicU64>,
) -> Result<()> {
    // Create the stream
    let mut stream = SCStream::new(&filter, &config);

    let last_audio_time_clone = last_audio_time.clone();
    let frames_received_clone = frames_received.clone();
    let sample_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sample_count_clone = sample_count.clone();

    // PERFORMANCE: Wrap mutable state in RefCell for interior mutability.
    // RefCell is fine here because the callback runs single-threaded from the audio thread.
    let producers = RefCell::new(producers);
    let conversion_buffer: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(4096));
    // Resampler: initialized on first callback if SCK delivers at != 48kHz
    let resampler: RefCell<Option<LinearResampler>> = RefCell::new(None);
    // Pre-allocated buffers for resampling (avoids per-callback allocation)
    let resample_input: RefCell<Vec<[f32; 2]>> = RefCell::new(Vec::with_capacity(4096));
    let resample_output_buf: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(8192));

    // Add audio output handler
    stream.add_output_handler(
        move |sample_buffer: CMSampleBuffer, of_type: SCStreamOutputType| {
            if !matches!(of_type, SCStreamOutputType::Audio) {
                return;
            }

            if let Some(audio_buffer_list) = sample_buffer.audio_buffer_list() {
                let buffer_count = audio_buffer_list.iter().count();
                let count = sample_count_clone.fetch_add(1, Ordering::Relaxed);

                // Log format info on first callback and initialize resampler if needed
                if count == 0 {
                    log::info!("[AudioCapture] SCK delivering {} audio buffer(s) per callback", buffer_count);
                    for (i, ab) in audio_buffer_list.iter().enumerate() {
                        log::info!("[AudioCapture] Buffer {}: {} bytes, {} channels",
                            i, ab.data().len(), ab.number_channels);
                    }
                    // Log and store actual sample rate from format description
                    if let Some(fmt) = sample_buffer.format_description() {
                        if let Some(rate) = fmt.audio_sample_rate() {
                            let actual_rate = rate as u32;
                            log::info!("[AudioCapture] SCK ACTUAL sample rate: {}Hz", actual_rate);
                            detected_sample_rate.store(actual_rate, Ordering::SeqCst);

                            // Initialize resampler if SCK delivers at != 48kHz
                            if actual_rate != 48000 {
                                log::info!("[AudioCapture] SCK rate mismatch: {}Hz != 48000Hz — enabling linear resampler", actual_rate);
                                *resampler.borrow_mut() = Some(LinearResampler::new(actual_rate, 48000));
                            } else {
                                log::info!("[AudioCapture] SCK rate matches canonical 48kHz — direct passthrough");
                            }
                        }
                        if let Some(channels) = fmt.audio_channel_count() {
                            log::info!("[AudioCapture] SCK ACTUAL channels: {}", channels);
                        }
                    }
                }

                // Handle based on buffer count
                if buffer_count == 1 {
                    // Single buffer = interleaved stereo (normal case)
                    let audio_buffer = audio_buffer_list.iter().next().unwrap();
                    let data = audio_buffer.data();
                    let mut buf = conversion_buffer.borrow_mut();
                    buf.clear();

                    // Convert raw bytes to f32 samples
                    for chunk in data.chunks_exact(4) {
                        let bytes: [u8; 4] = chunk.try_into().unwrap();
                        buf.push(f32::from_le_bytes(bytes));
                    }

                    // Update last audio timestamp (watchdog)
                    last_audio_time_clone.store(now_millis(), Ordering::SeqCst);

                    if count < 5 {
                        let max_val = buf.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
                        let min_val = buf.iter().map(|x| x.abs()).filter(|x| *x > 0.0001).fold(f32::MAX, f32::min);
                        log::info!("[AudioCapture] Callback #{}: {} samples (interleaved), max={:.4}, min_nonzero={:.6}",
                            count, buf.len(), max_val, min_val);
                    }

                    // Fanout to all ring buffers (resampled to 48k if needed)
                    {
                        let mut prods = producers.borrow_mut();
                        let mut rs = resampler.borrow_mut();

                        if let Some(ref mut resampler_active) = *rs {
                            let mut input_buf = resample_input.borrow_mut();
                            input_buf.clear();
                            for chunk in buf.chunks_exact(2) {
                                input_buf.push([chunk[0], chunk[1]]);
                            }
                            let resampled = resampler_active.process(&input_buf);
                            let mut out = resample_output_buf.borrow_mut();
                            out.clear();
                            for &frame in resampled {
                                out.push(frame[0]);
                                out.push(frame[1]);
                            }
                            frames_received_clone.fetch_add((out.len() / 2) as u64, Ordering::Relaxed);
                            fanout(&mut *prods, &out);
                        } else {
                            frames_received_clone.fetch_add((buf.len() / 2) as u64, Ordering::Relaxed);
                            fanout(&mut *prods, &buf);
                        }
                    }
                } else if buffer_count == 2 {
                    // Two buffers = non-interleaved (L in buffer 0, R in buffer 1)
                    // Need to interleave them ourselves
                    let buffers: Vec<_> = audio_buffer_list.iter().collect();
                    let left_data = buffers[0].data();
                    let right_data = buffers[1].data();

                    let mut buf = conversion_buffer.borrow_mut();
                    buf.clear();

                    // Convert and interleave: L0, R0, L1, R1, ...
                    let left_chunks = left_data.chunks_exact(4);
                    let right_chunks = right_data.chunks_exact(4);

                    for (l_chunk, r_chunk) in left_chunks.zip(right_chunks) {
                        let l_bytes: [u8; 4] = l_chunk.try_into().unwrap();
                        let r_bytes: [u8; 4] = r_chunk.try_into().unwrap();
                        buf.push(f32::from_le_bytes(l_bytes));
                        buf.push(f32::from_le_bytes(r_bytes));
                    }

                    // Update last audio timestamp (watchdog)
                    last_audio_time_clone.store(now_millis(), Ordering::SeqCst);

                    if count < 5 {
                        let max_val = buf.iter().map(|x| x.abs()).fold(0.0f32, f32::max);

                        // Check stereo separation - are L and R different?
                        let mut l_sum = 0.0f32;
                        let mut r_sum = 0.0f32;
                        let mut diff_sum = 0.0f32;
                        for i in 0..buf.len()/2 {
                            let l = buf.get(i * 2).copied().unwrap_or(0.0);
                            let r = buf.get(i * 2 + 1).copied().unwrap_or(0.0);
                            l_sum += l.abs();
                            r_sum += r.abs();
                            diff_sum += (l - r).abs();
                        }
                        let samples_per_ch = buf.len() / 2;
                        let l_avg = l_sum / samples_per_ch as f32;
                        let r_avg = r_sum / samples_per_ch as f32;
                        let diff_avg = diff_sum / samples_per_ch as f32;

                        log::info!("[AudioCapture] Callback #{}: {} samples, max={:.4}, L_avg={:.6}, R_avg={:.6}, LR_diff={:.6}",
                            count, buf.len(), max_val, l_avg, r_avg, diff_avg);
                    }

                    // Fanout to all ring buffers (resampled to 48k if needed)
                    {
                        let mut prods = producers.borrow_mut();
                        let mut rs = resampler.borrow_mut();

                        if let Some(ref mut resampler_active) = *rs {
                            let mut input_buf = resample_input.borrow_mut();
                            input_buf.clear();
                            for chunk in buf.chunks_exact(2) {
                                input_buf.push([chunk[0], chunk[1]]);
                            }
                            let resampled = resampler_active.process(&input_buf);
                            let mut out = resample_output_buf.borrow_mut();
                            out.clear();
                            for &frame in resampled {
                                out.push(frame[0]);
                                out.push(frame[1]);
                            }
                            frames_received_clone.fetch_add((out.len() / 2) as u64, Ordering::Relaxed);
                            fanout(&mut *prods, &out);
                        } else {
                            frames_received_clone.fetch_add((buf.len() / 2) as u64, Ordering::Relaxed);
                            fanout(&mut *prods, &buf);
                        }
                    }
                }
            }
        },
        SCStreamOutputType::Audio,
    );

    log::info!("[AudioCapture] Starting stream...");

    // Start the stream
    stream.start_capture()
        .map_err(|e| anyhow!("Failed to start capture: {:?}", e))?;

    log::info!("[AudioCapture] Stream started successfully");

    // Wait loop - poll quickly for responsive shutdown
    while running.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Stop the stream
    stream.stop_capture()
        .map_err(|e| anyhow!("Failed to stop capture: {:?}", e))?;

    log::info!("[AudioCapture] Stream stopped");
    Ok(())
}

/// Inline channel detector for multi-channel cpal devices.
///
/// Scans all adjacent stereo pairs during the first ~1 second of audio callbacks
/// to find which pair carries the strongest signal. Avoids needing a GStreamer probe
/// (which disrupts USB audio devices).
///
/// During detection, audio is still output from the current best pair (starts with
/// profile default, switches as soon as a better pair is found).
struct ChannelDetector {
    /// Sum of squares per pair: pair i covers channels [2*i, 2*i+1]
    pair_energy: Vec<f64>,
    /// Total frames analyzed so far
    frames_analyzed: usize,
    /// Detection window in frames (~1s at 48kHz)
    detection_frames: usize,
    /// Currently active channels
    active_left: usize,
    active_right: usize,
    /// Whether detection is complete (locked to final pair)
    locked: bool,
    /// Total channels on device
    total_channels: usize,
}

impl ChannelDetector {
    /// Create a new detector. Starts with the profile's default pair.
    fn new(total_channels: usize, default_left: usize, default_right: usize) -> Self {
        let num_pairs = total_channels / 2;
        // Use profile default, clamped to valid range
        let (left, right) = if default_left < total_channels && default_right < total_channels {
            (default_left, default_right)
        } else {
            (0, 1)
        };
        Self {
            pair_energy: vec![0.0; num_pairs],
            frames_analyzed: 0,
            detection_frames: 48_000, // ~1 second at 48kHz
            active_left: left,
            active_right: right,
            locked: false,
            total_channels,
        }
    }

    /// Feed a callback's worth of multi-channel data. Accumulates per-pair energy.
    /// After detection_frames, locks to the best pair.
    #[inline]
    fn feed(&mut self, data: &[f32], channels: usize, num_frames: usize) {
        if self.locked {
            return;
        }

        let num_pairs = channels / 2;

        // Accumulate energy for all pairs
        for f in 0..num_frames {
            let base = f * channels;
            for p in 0..num_pairs {
                let l = data[base + p * 2];
                let r = data[base + p * 2 + 1];
                self.pair_energy[p] += (l * l + r * r) as f64;
            }
        }

        self.frames_analyzed += num_frames;

        // Check if detection window is complete
        if self.frames_analyzed >= self.detection_frames {
            self.finalize();
        }
    }

    /// Also works with i16 data (converted to f32 first)
    #[inline]
    fn feed_i16(&mut self, data: &[i16], channels: usize, num_frames: usize) {
        if self.locked {
            return;
        }

        let num_pairs = channels / 2;

        for f in 0..num_frames {
            let base = f * channels;
            for p in 0..num_pairs {
                let l = data[base + p * 2] as f32 / 32768.0;
                let r = data[base + p * 2 + 1] as f32 / 32768.0;
                self.pair_energy[p] += (l * l + r * r) as f64;
            }
        }

        self.frames_analyzed += num_frames;

        if self.frames_analyzed >= self.detection_frames {
            self.finalize();
        }
    }

    /// Finalize detection: pick the pair with highest energy.
    fn finalize(&mut self) {
        self.locked = true;

        let num_pairs = self.pair_energy.len();
        if num_pairs == 0 {
            return;
        }

        // Find pair with maximum energy
        let mut best_pair = 0;
        let mut best_energy = 0.0f64;
        for (i, &energy) in self.pair_energy.iter().enumerate() {
            if energy > best_energy {
                best_energy = energy;
                best_pair = i;
            }
        }

        let new_left = best_pair * 2;
        let new_right = best_pair * 2 + 1;

        // Log all pair energies for debugging
        let energies: Vec<String> = self.pair_energy.iter().enumerate()
            .map(|(i, e)| format!("[{},{}]={:.4}", i * 2, i * 2 + 1, e / self.frames_analyzed as f64))
            .collect();
        log::info!("[AudioCapture] Channel detection complete ({} frames): best=[{},{}] energy={:.6} | all: {}",
            self.frames_analyzed, new_left, new_right,
            best_energy / self.frames_analyzed as f64,
            energies.join(" "));

        if new_left != self.active_left || new_right != self.active_right {
            log::info!("[AudioCapture] Switching channels [{},{}] → [{},{}]",
                self.active_left, self.active_right, new_left, new_right);
        }

        self.active_left = new_left;
        self.active_right = new_right;
    }

    /// Get the currently active channel pair.
    #[inline]
    fn active_pair(&self) -> (usize, usize) {
        (self.active_left, self.active_right)
    }
}
