//! GStreamer-based audio capture for input devices (DJ mixers, audio interfaces, microphones)
//!
//! Primary capture backend for multi-channel USB audio devices. Uses GStreamer's
//! osxaudiosrc (AudioUnit-based) with proper sample rate conversion.
//!
//! Pipeline:
//!   osxaudiosrc → audioconvert → audioresample → capsfilter(F32LE/48k/2ch) → appsink
//!
//! For multi-channel devices (DJ mixers), a mix-matrix is applied on audioconvert
//! to route the correct L/R master pair to stereo output.

use anyhow::{anyhow, Result, Context};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gst::glib;
use glib::prelude::ToSendValue;
use gstreamer_audio as gst_audio;

use super::capture::InputDeviceProfile;
use super::audio_bus::{AudioBusProducers, fanout};

/// GStreamer-based audio capture pipeline
pub(crate) struct GstAudioCapture {
    pipeline: gst::Pipeline,
    _bus_watch: Option<gst::bus::BusWatchGuard>,
    device_name: String,
}

impl GstAudioCapture {
    /// Start a GStreamer capture pipeline for the given device.
    pub fn start(
        device_name: &str,
        profile: &InputDeviceProfile,
        in_channels: usize,
        producers: AudioBusProducers,
        last_audio_time: Arc<AtomicU64>,
        _overflow_count: Arc<AtomicU64>,
        frames_received: Arc<AtomicU64>,
    ) -> Result<Self> {
        gst::init().context("Failed to initialize GStreamer")?;

        let device_id = find_gst_device_id(device_name)
            .ok_or_else(|| anyhow!("[GstCapture] Could not find CoreAudio device for: {}", device_name))?;

        log::info!("[GstCapture] Found device ID: {} for '{}'", device_id, device_name);

        let pipeline = if in_channels > 2 {
            log::info!("[GstCapture] Multi-channel device ({} ch) — using profile [{},{}] ({})",
                in_channels, profile.left_channel, profile.right_channel, profile.name);
            build_pipeline(device_id, Some((in_channels, profile)))?
        } else {
            log::info!("[GstCapture] Standard device ({} ch) - direct stereo path", in_channels);
            build_pipeline(device_id, None)?
        };

        let appsink = pipeline.by_name("audio_sink")
            .ok_or_else(|| anyhow!("[GstCapture] Failed to find audio_sink in pipeline"))?
            .dynamic_cast::<gst_app::AppSink>()
            .map_err(|_| anyhow!("[GstCapture] audio_sink is not an AppSink"))?;

        let caps_verified = Arc::new(AtomicBool::new(false));
        let caps_verified_clone = caps_verified.clone();
        let caps_hash: Arc<Mutex<(i32, i32)>> = Arc::new(Mutex::new((0, 0)));
        let caps_hash_clone = caps_hash.clone();
        let cb_count = Arc::new(AtomicU64::new(0));
        let cb_count_clone = cb_count.clone();
        let last_audio_time_clone = last_audio_time.clone();
        let frames_received_clone = frames_received.clone();
        let device_name_log = device_name.to_string();
        let mut producers = producers;

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;

                    // Caps verification (first sample + renegotiation detection)
                    if let Some(caps) = sample.caps() {
                        if let Some(s) = caps.structure(0) {
                            let rate = s.get::<i32>("rate").unwrap_or(0);
                            let channels = s.get::<i32>("channels").unwrap_or(0);

                            if !caps_verified_clone.load(Ordering::Relaxed) {
                                let format = s.get::<&str>("format").unwrap_or("unknown");
                                let layout = s.get::<&str>("layout").unwrap_or("unknown");

                                if format != "F32LE" || rate != 48000 || channels != 2 || layout != "interleaved" {
                                    log::error!("[GstCapture] CAPS MISMATCH for '{}': format={}, rate={}, channels={}, layout={}. Expected F32LE/48000/2/interleaved. Stopping.",
                                        device_name_log, format, rate, channels, layout);
                                    return Err(gst::FlowError::Error);
                                }
                                log::info!("[GstCapture] Caps verified: F32LE/48000Hz/2ch/interleaved for '{}'", device_name_log);
                                *caps_hash_clone.lock() = (rate, channels);
                                caps_verified_clone.store(true, Ordering::Relaxed);
                            } else {
                                let expected = *caps_hash_clone.lock();
                                if (rate, channels) != expected {
                                    log::error!("[GstCapture] CAPS RENEGOTIATION for '{}': rate={}, channels={} (was rate={}, channels={}). Stopping.",
                                        device_name_log, rate, channels, expected.0, expected.1);
                                    return Err(gst::FlowError::Error);
                                }
                            }
                        }
                    }

                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    let bytes = map.as_slice();
                    if bytes.is_empty() || bytes.len() % 4 != 0 {
                        return Ok(gst::FlowSuccess::Ok);
                    }
                    let data: &[f32] = bytemuck::cast_slice(bytes);

                    let num_samples = (data.len() / 2) * 2;
                    if num_samples == 0 {
                        return Ok(gst::FlowSuccess::Ok);
                    }
                    let data = &data[..num_samples];
                    let num_stereo_frames = num_samples / 2;

                    let count = cb_count_clone.fetch_add(1, Ordering::Relaxed);
                    if count < 5 {
                        log::info!("[GstCapture] appsink #{}: {} samples ({} frames) from '{}'",
                            count, data.len(), num_stereo_frames, device_name_log);
                    }

                    last_audio_time_clone.store(super::capture::now_millis(), Ordering::SeqCst);
                    fanout(&mut producers, data);
                    frames_received_clone.fetch_add(num_stereo_frames as u64, Ordering::Relaxed);

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        // Bus watch for error monitoring
        let bus = pipeline.bus().ok_or_else(|| anyhow!("[GstCapture] Pipeline has no bus"))?;
        let device_name_err = device_name.to_string();
        let bus_watch = bus.add_watch(move |_, msg| {
            match msg.view() {
                gst::MessageView::Error(err) => {
                    log::error!("[GstCapture] Pipeline error for '{}': {} (debug: {:?})",
                        device_name_err, err.error(), err.debug());
                }
                gst::MessageView::Warning(warn) => {
                    log::warn!("[GstCapture] Pipeline warning for '{}': {} (debug: {:?})",
                        device_name_err, warn.error(), warn.debug());
                }
                gst::MessageView::Eos(_) => {
                    log::info!("[GstCapture] EOS received for '{}'", device_name_err);
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        }).map_err(|_| anyhow!("[GstCapture] Failed to add bus watch"))?;

        // Start pipeline
        pipeline.set_state(gst::State::Playing)
            .map_err(|e| anyhow!("[GstCapture] Failed to set pipeline to Playing: {:?}", e))?;

        log::info!("[GstCapture] Pipeline started for '{}'", device_name);

        Ok(Self {
            pipeline,
            _bus_watch: Some(bus_watch),
            device_name: device_name.to_string(),
        })
    }

    /// Stop the GStreamer pipeline.
    /// Spawns shutdown on a background thread to avoid blocking the main/render thread
    /// on CoreAudio device teardown (AudioDeviceStop can block waiting for IO mutex).
    pub fn stop(&mut self) {
        let pipeline = self.pipeline.clone();
        let name = self.device_name.clone();
        std::thread::Builder::new()
            .name("gst-capture-stop".into())
            .spawn(move || {
                log::info!("[GstCapture] Stopping pipeline for '{}' (background)", name);
                if let Err(e) = pipeline.set_state(gst::State::Null) {
                    log::error!("[GstCapture] Failed to stop pipeline: {:?}", e);
                }
                log::info!("[GstCapture] Pipeline stopped for '{}'", name);
            })
            .ok();
    }
}

impl Drop for GstAudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

// === Pipeline construction ===

/// Build the GStreamer capture pipeline.
///
/// Pipeline: osxaudiosrc → audioconvert → audioresample → capsfilter(F32LE/48k/2ch) → queue → appsink
///
/// The queue element provides ~100ms of jitter buffering to absorb scheduling
/// irregularities in the CoreAudio/USB delivery without causing pops.
///
/// If `mixer_config` is Some, applies a mix-matrix on audioconvert to route
/// the specified L/R pair from the multi-channel input to stereo output.
fn build_pipeline(device_id: i64, mixer_config: Option<(usize, &InputDeviceProfile)>) -> Result<gst::Pipeline> {
    let pipeline = gst::Pipeline::new();

    let src = gst::ElementFactory::make("osxaudiosrc")
        .property("device", device_id as i32)
        .build()
        .context("[GstCapture] Failed to create osxaudiosrc")?;

    // audioconvert: dithering=none avoids adding noise on int→float conversion
    let audioconvert = gst::ElementFactory::make("audioconvert")
        .name("convert")
        .build()
        .context("[GstCapture] Failed to create audioconvert")?;
    audioconvert.set_property_from_str("dithering", "none");

    // audioresample: quality=4 for clean resampling if device rate != 48kHz
    let audioresample = gst::ElementFactory::make("audioresample")
        .property("quality", 4i32)
        .build()
        .context("[GstCapture] Failed to create audioresample")?;

    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", gst_audio::AudioCapsBuilder::new_interleaved()
            .format(gst_audio::AudioFormat::F32le)
            .rate(48000)
            .channels(2)
            .build())
        .build()
        .context("[GstCapture] Failed to create capsfilter")?;

    // queue: jitter buffer to absorb USB/scheduling irregularities (100ms, ~4800 frames)
    let queue = gst::ElementFactory::make("queue")
        .property("max-size-time", 100_000_000u64)  // 100ms in nanoseconds
        .property("max-size-buffers", 0u32)          // no buffer count limit
        .property("max-size-bytes", 0u32)            // no byte limit
        .build()
        .context("[GstCapture] Failed to create queue")?;

    let appsink = gst::ElementFactory::make("appsink")
        .name("audio_sink")
        .property("sync", false)
        .build()
        .context("[GstCapture] Failed to create appsink")?;

    pipeline.add_many([&src, &audioconvert, &audioresample, &capsfilter, &queue, &appsink])
        .context("[GstCapture] Failed to add elements to pipeline")?;

    gst::Element::link_many([&src, &audioconvert, &audioresample, &capsfilter, &queue, &appsink])
        .context("[GstCapture] Failed to link pipeline elements")?;

    // Apply mix-matrix for multi-channel devices
    if let Some((in_channels, profile)) = mixer_config {
        let mix_matrix = build_mix_matrix(in_channels, profile.left_channel, profile.right_channel);
        audioconvert.set_property("mix-matrix", &mix_matrix);
        log::info!("[GstCapture] Pipeline: osxaudiosrc(dev={}) → audioconvert(mix [{},{}]) → audioresample(q4) → F32LE/48kHz/2ch → queue(100ms) → appsink",
            device_id, profile.left_channel, profile.right_channel);
    } else {
        log::info!("[GstCapture] Pipeline: osxaudiosrc(dev={}) → audioconvert → audioresample(q4) → F32LE/48kHz/2ch → queue(100ms) → appsink",
            device_id);
    }

    Ok(pipeline)
}

// === Device discovery ===

/// Find the CoreAudio AudioDeviceID for a device matching the given name.
///
/// Uses GstDeviceMonitor to enumerate audio sources, creates element from GstDevice
/// (preferred binding), then reads the `device` property as a variant integer type.
fn find_gst_device_id(device_name: &str) -> Option<i64> {
    let monitor = gst::DeviceMonitor::new();

    let caps = gst::Caps::builder("audio/x-raw").build();
    monitor.add_filter(Some("Audio/Source"), Some(&caps));

    if monitor.start().is_err() {
        log::error!("[GstCapture] Failed to start device monitor");
        return None;
    }

    let result = find_device_in_monitor(&monitor, device_name);
    monitor.stop();
    result
}

/// Search for a matching device in a running monitor.
/// Requires a unique match — if multiple devices match, fails with a log listing candidates.
fn find_device_in_monitor(monitor: &gst::DeviceMonitor, device_name: &str) -> Option<i64> {
    let devices = monitor.devices();

    // Collect all source devices and log them
    let mut candidates: Vec<(String, i64)> = Vec::new();

    for device in &devices {
        let display_name = device.display_name().to_string();
        let device_class = device.device_class().to_string();

        log::info!("[GstCapture] DeviceMonitor found: '{}' class='{}'", display_name, device_class);

        if !device_class.contains("Source") && !device_class.contains("Audio") {
            continue;
        }

        // Match by display name (exact preferred, then substring)
        let is_match = display_name == device_name
            || display_name.contains(device_name)
            || device_name.contains(&display_name);

        if !is_match {
            continue;
        }

        // Create element directly from GstDevice (preferred — sets all properties correctly)
        let element = match device.create_element(None) {
            Ok(el) => el,
            Err(e) => {
                log::warn!("[GstCapture] Device '{}' create_element failed: {:?}", display_name, e);
                continue;
            }
        };

        // Read device property as generic Value (support i32/i64/u32/u64)
        if let Some(id) = read_device_property_as_i64(&element) {
            candidates.push((display_name, id));
        } else {
            log::warn!("[GstCapture] Device '{}' has no readable 'device' property", display_name);
        }
    }

    match candidates.len() {
        0 => {
            log::error!("[GstCapture] No device matched '{}'", device_name);
            None
        }
        1 => {
            let (name, id) = &candidates[0];
            log::info!("[GstCapture] Unique match: '{}' → device ID {}", name, id);
            Some(*id)
        }
        _ => {
            // Multiple matches — prefer exact name match
            if let Some((name, id)) = candidates.iter().find(|(n, _)| n == device_name) {
                log::info!("[GstCapture] Exact match (of {} candidates): '{}' → device ID {}", candidates.len(), name, id);
                return Some(*id);
            }
            // No exact match — log all candidates and use first
            log::warn!("[GstCapture] {} devices matched '{}', using first:", candidates.len(), device_name);
            for (i, (name, id)) in candidates.iter().enumerate() {
                log::warn!("[GstCapture]   [{}] '{}' → device ID {}", i, name, id);
            }
            Some(candidates[0].1)
        }
    }
}

/// Read the `device` property from an osxaudiosrc element as i64.
/// Supports multiple integer types that different GStreamer/macOS versions may use.
fn read_device_property_as_i64(element: &gst::Element) -> Option<i64> {
    let value = element.property_value("device");
    // Try common integer types
    if let Ok(v) = value.get::<i32>() {
        return Some(v as i64);
    }
    if let Ok(v) = value.get::<i64>() {
        return Some(v);
    }
    if let Ok(v) = value.get::<u32>() {
        return Some(v as i64);
    }
    if let Ok(v) = value.get::<u64>() {
        return Some(v as i64);
    }
    None
}

// === Stereo pair probe ===

/// Probe window in milliseconds
const PROBE_WINDOW_MS: u64 = 1200;
/// Minimum frames required for a valid probe (≈100ms at 48kHz)
const PROBE_MIN_VALID_FRAMES: u64 = 480 * 10;
/// RMS floor — pairs below this are rejected as silence/noise
const PROBE_SILENCE_FLOOR: f64 = 0.005;
/// Balance threshold — pairs below this are penalized heavily (0..1, 1.0 = perfect balance)
const PROBE_BALANCE_MIN: f64 = 0.65;

/// Per-pair statistics accumulated during probe
#[derive(Clone, Default)]
struct PairStats {
    sum_sq_l: f64,
    sum_sq_r: f64,
    frames: u64,
}

/// Result of the probe pass
pub struct ProbeResult {
    pub left_ch: usize,
    pub right_ch: usize,
    pub in_channels: usize,
    pub energy: f64,
    pub balance: f64,
}

/// Run the GStreamer probe pass to detect the best stereo pair on a multi-channel device.
///
/// Initializes GStreamer if needed, finds the device by name, and runs a ~1.2s probe
/// to detect which adjacent channel pair carries the strongest stereo signal.
/// Returns the probed channel pair or an error.
pub fn probe_device_channels(device_name: &str) -> Result<ProbeResult> {
    gst::init().context("Failed to initialize GStreamer for probe")?;
    let device_id = find_gst_device_id(device_name)
        .ok_or_else(|| anyhow!("[GstCapture] Probe: Could not find device '{}'", device_name))?;
    let result = probe_best_pair(device_id, device_name)?;
    // Allow CoreAudio to fully release the device handle after the probe pipeline teardown.
    // AudioDeviceStop can have an async tail even after GStreamer reports Null state.
    log::info!("[GstCapture] Probe complete — waiting 300ms for CoreAudio device release...");
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(result)
}

/// Probe all stereo pairs on a multi-channel device.
///
/// Builds a temporary pipeline: osxaudiosrc → audioconvert → capsfilter(F32LE,48k) → appsink
/// The channel count is read from GStreamer's negotiated caps (NOT from cpal).
/// Collects ~1200ms of raw multi-channel audio, computes per-pair energy + balance,
/// and returns the pair with the highest score.
///
/// Score = energy * (0.5 + 0.5 * balance)
/// where balance = 1 - |rmsL - rmsR| / (rmsL + rmsR + eps)
fn probe_best_pair(device_id: i64, device_name: &str) -> Result<ProbeResult> {
    log::info!("[GstCapture] Starting probe pass for '{}' ({}ms window)...", device_name, PROBE_WINDOW_MS);

    // Build probe pipeline — no channel constraint, let GStreamer negotiate native count
    let pipeline = gst::Pipeline::new();

    let src = gst::ElementFactory::make("osxaudiosrc")
        .property("device", device_id as i32)
        .build()
        .context("[GstCapture] Probe: Failed to create osxaudiosrc")?;

    let audioconvert = gst::ElementFactory::make("audioconvert")
        .build()
        .context("[GstCapture] Probe: Failed to create audioconvert")?;

    // Caps: F32LE, 48kHz, interleaved — but NO channel count specified.
    // GStreamer will negotiate the source's native channel count.
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", gst_audio::AudioCapsBuilder::new_interleaved()
            .format(gst_audio::AudioFormat::F32le)
            .rate(48000)
            .build())
        .build()
        .context("[GstCapture] Probe: Failed to create capsfilter")?;

    let appsink = gst::ElementFactory::make("appsink")
        .name("probe_sink")
        .property("sync", false)
        .property("max-buffers", 16u32)
        .property("drop", true)
        .build()
        .context("[GstCapture] Probe: Failed to create appsink")?;

    pipeline.add_many([&src, &audioconvert, &capsfilter, &appsink])
        .context("[GstCapture] Probe: Failed to add elements")?;
    gst::Element::link_many([&src, &audioconvert, &capsfilter, &appsink])
        .context("[GstCapture] Probe: Failed to link elements")?;

    let appsink = appsink.dynamic_cast::<gst_app::AppSink>()
        .map_err(|_| anyhow!("[GstCapture] Probe: not an AppSink"))?;

    // Shared state for the callback — fixed-size arrays allocated after first buffer
    let stats: Arc<Mutex<Option<Vec<PairStats>>>> = Arc::new(Mutex::new(None));
    let negotiated_channels: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let stats_clone = stats.clone();
    let neg_ch_clone = negotiated_channels.clone();

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;

                // Read channel count from negotiated caps on first buffer
                let ch_count = {
                    let mut neg = neg_ch_clone.lock();
                    if let Some(ch) = *neg {
                        ch
                    } else {
                        let caps = sample.caps().ok_or(gst::FlowError::Error)?;
                        let s = caps.structure(0).ok_or(gst::FlowError::Error)?;
                        let channels = s.get::<i32>("channels").unwrap_or(0) as usize;
                        if channels < 2 {
                            log::error!("[GstCapture] Probe: negotiated only {} channels", channels);
                            return Err(gst::FlowError::Error);
                        }
                        log::info!("[GstCapture] Probe: negotiated {} channels from device", channels);
                        *neg = Some(channels);

                        // Initialize stats array
                        let num_pairs = channels / 2;
                        *stats_clone.lock() = Some(vec![PairStats::default(); num_pairs]);

                        channels
                    }
                };

                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let bytes = map.as_slice();
                if bytes.len() < 4 || bytes.len() % 4 != 0 {
                    return Ok(gst::FlowSuccess::Ok);
                }
                let data: &[f32] = bytemuck::cast_slice(bytes);
                let num_frames = data.len() / ch_count;
                let num_pairs = ch_count / 2;

                // Accumulate stats — no allocations, fixed array
                let mut stats_guard = stats_clone.lock();
                if let Some(ref mut pair_stats) = *stats_guard {
                    for frame in 0..num_frames {
                        let base = frame * ch_count;
                        for p in 0..num_pairs {
                            let l = data[base + p * 2] as f64;
                            let r = data[base + p * 2 + 1] as f64;
                            pair_stats[p].sum_sq_l += l * l;
                            pair_stats[p].sum_sq_r += r * r;
                            pair_stats[p].frames += 1;
                        }
                    }
                }

                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    // Start probe pipeline
    pipeline.set_state(gst::State::Playing)
        .map_err(|e| anyhow!("[GstCapture] Probe: Failed to start: {:?}", e))?;

    // Wait for probe window
    std::thread::sleep(std::time::Duration::from_millis(PROBE_WINDOW_MS));

    // Stop probe pipeline — wait for state change to complete so CoreAudio releases the device
    let _ = pipeline.set_state(gst::State::Null);
    // Block until the pipeline has fully transitioned to Null (device handle released)
    let _ = pipeline.state(gst::ClockTime::from_seconds(3));
    drop(pipeline);

    // Read results
    let in_channels = negotiated_channels.lock()
        .ok_or_else(|| anyhow!("[GstCapture] Probe: no caps negotiated (device didn't produce audio)"))?;
    let num_pairs = in_channels / 2;

    let stats_guard = stats.lock();
    let pair_stats = stats_guard.as_ref()
        .ok_or_else(|| anyhow!("[GstCapture] Probe: no stats collected"))?;

    // Check minimum frames
    let max_frames = pair_stats.iter().map(|s| s.frames).max().unwrap_or(0);
    if max_frames < PROBE_MIN_VALID_FRAMES {
        return Err(anyhow!("[GstCapture] Probe: insufficient data ({} frames, need {})",
            max_frames, PROBE_MIN_VALID_FRAMES));
    }

    // Score each pair
    let mut best_pair = 0usize;
    let mut best_score = -1.0f64;
    let mut best_energy = 0.0f64;
    let mut best_balance = 0.0f64;

    for p in 0..num_pairs {
        let s = &pair_stats[p];
        if s.frames == 0 {
            log::info!("[GstCapture] Probe pair [{},{}]: no frames", p * 2, p * 2 + 1);
            continue;
        }

        let rms_l = (s.sum_sq_l / s.frames as f64).sqrt();
        let rms_r = (s.sum_sq_r / s.frames as f64).sqrt();
        let energy = 0.5 * (rms_l + rms_r);
        let balance = 1.0 - (rms_l - rms_r).abs() / (rms_l + rms_r + 1e-10);
        let score = energy * (0.5 + 0.5 * balance);

        let rejected = energy < PROBE_SILENCE_FLOOR;
        log::info!("[GstCapture] Probe pair [{},{}]: rmsL={:.5} rmsR={:.5} energy={:.5} balance={:.3} score={:.5}{}",
            p * 2, p * 2 + 1, rms_l, rms_r, energy, balance, score,
            if rejected { " [REJECTED: below silence floor]" } else { "" });

        if rejected {
            continue;
        }

        if score > best_score {
            best_score = score;
            best_pair = p;
            best_energy = energy;
            best_balance = balance;
        }
    }

    if best_score < 0.0 {
        return Err(anyhow!("[GstCapture] Probe: all pairs below silence floor ({:.4})", PROBE_SILENCE_FLOOR));
    }

    let left_ch = best_pair * 2;
    let right_ch = best_pair * 2 + 1;

    log::info!("[GstCapture] Probe WINNER: pair [{},{}] energy={:.5} balance={:.3} score={:.5} | {} ch from '{}'",
        left_ch, right_ch, best_energy, best_balance, best_score, in_channels, device_name);

    Ok(ProbeResult {
        left_ch,
        right_ch,
        in_channels,
        energy: best_energy,
        balance: best_balance,
    })
}

// === Mix-matrix ===

/// Build a 2×N routing matrix: row0 selects left_ch, row1 selects right_ch.
/// Strictly routing (1.0 at selected channel, 0.0 elsewhere).
fn build_mix_matrix(in_channels: usize, left_ch: usize, right_ch: usize) -> gst::Array {
    let mut rows: Vec<glib::SendValue> = Vec::with_capacity(2);

    // Row 0: output Left
    let mut row_l: Vec<glib::SendValue> = Vec::with_capacity(in_channels);
    for ch in 0..in_channels {
        let val: f32 = if ch == left_ch { 1.0 } else { 0.0 };
        row_l.push(val.to_send_value());
    }
    rows.push(gst::Array::from_values(row_l).to_send_value());

    // Row 1: output Right
    let mut row_r: Vec<glib::SendValue> = Vec::with_capacity(in_channels);
    for ch in 0..in_channels {
        let val: f32 = if ch == right_ch { 1.0 } else { 0.0 };
        row_r.push(val.to_send_value());
    }
    rows.push(gst::Array::from_values(row_r).to_send_value());

    gst::Array::from_values(rows)
}
