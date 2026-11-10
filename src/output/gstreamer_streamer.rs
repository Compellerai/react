//! GStreamer-based RTMP streamer
//!
//! Uses GStreamer's native pipeline architecture for proper A/V synchronization.
//! The flvmux element handles interleaving audio and video correctly.
//! Push-based: audio and video are pushed directly to appsrc from the main pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use anyhow::{Result, Context};
use parking_lot::Mutex;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_audio as gst_audio;

use super::FrameRing;
use super::ffmpeg_streamer::OutputMode;

// ============================================================================
// Stream Platform
// ============================================================================

/// Which platform this streamer is targeting. Drives bitrate selection and
/// encoder rate-control settings (Twitch/Kick want CBR per their bitrate table,
/// YouTube tolerates higher VBR-ish rates).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StreamPlatform {
    #[default]
    YouTube,
    Twitch,
    /// Kick uses the same RTMP CBR profile as Twitch (6000 kbps cap, no
    /// B-frames). Ingest is an `rtmps://` URL the user pastes from their
    /// Kick Creator Dashboard rather than a pinged server list.
    Kick,
}

impl StreamPlatform {
    /// Human-readable name for log lines and user-facing error messages.
    pub fn label(&self) -> &'static str {
        match self {
            StreamPlatform::YouTube => "YouTube",
            StreamPlatform::Twitch => "Twitch",
            StreamPlatform::Kick => "Kick",
        }
    }
}

// ============================================================================
// Stream State
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StreamState {
    #[default]
    Idle,
    Starting,
    Streaming,
    Stopping,
    Stopped,
    Error,
}

// ============================================================================
// Stream Statistics
// ============================================================================

#[derive(Clone, Default)]
pub struct StreamStats {
    pub state: StreamState,
    pub duration_secs: f32,
    pub video_frames_sent: u64,
    pub audio_frames_sent: u64,
    pub current_mode: OutputMode,
    /// Human-readable error description when state is Error
    pub error_message: Option<String>,
}

// ============================================================================
// GStreamer Streamer
// ============================================================================

pub struct GStreamerStreamer {
    state: Arc<Mutex<StreamState>>,
    start_time: Arc<Mutex<Option<Instant>>>,
    video_frames_sent: Arc<AtomicU64>,
    audio_frames_sent: Arc<AtomicU64>,
    frame_ring: Arc<FrameRing>,
    stop_flag: Arc<AtomicBool>,
    mode: OutputMode,
    platform: StreamPlatform,
    source_audio_sample_rate: u32,

    // Pipeline handles - set when streaming starts
    pipeline: Arc<Mutex<Option<gst::Pipeline>>>,
    video_src: Arc<Mutex<Option<gst_app::AppSrc>>>,
    audio_src: Arc<Mutex<Option<gst_app::AppSrc>>>,
    last_frame_num: Arc<Mutex<u32>>,
    input_dimensions: Arc<Mutex<(u32, u32)>>,

    // PTS tracking for proper timestamps
    audio_pts: Arc<AtomicU64>,
    video_pts: Arc<AtomicU64>,

    // Pipeline bus — polled from get_stats() for error/EOS detection.
    // NOT bus.add_watch(): that callback only fires under a running GLib main
    // loop, and REACT (egui/eframe) runs none on this thread — which also
    // exits right after the pipeline is built.
    bus: Arc<Mutex<Option<gst::Bus>>>,

    /// Human-readable error reason (set on pipeline failure)
    error_message: Arc<Mutex<Option<String>>>,

    /// Direct audio buffer for InputDevice dispatch (bypasses render loop)
    /// Drained by push_video/drain_direct_audio calls from the render loop.
    direct_audio_buffer: Arc<Mutex<Vec<f32>>>,
}

impl GStreamerStreamer {
    pub fn new(
        frame_ring: Arc<FrameRing>,
        mode: OutputMode,
        platform: StreamPlatform,
        source_audio_sample_rate: u32,
    ) -> Result<Self> {
        gst::init().context("Failed to initialize GStreamer")?;

        Ok(Self {
            state: Arc::new(Mutex::new(StreamState::Idle)),
            start_time: Arc::new(Mutex::new(None)),
            video_frames_sent: Arc::new(AtomicU64::new(0)),
            audio_frames_sent: Arc::new(AtomicU64::new(0)),
            frame_ring,
            stop_flag: Arc::new(AtomicBool::new(false)),
            mode,
            platform,
            source_audio_sample_rate,
            pipeline: Arc::new(Mutex::new(None)),
            video_src: Arc::new(Mutex::new(None)),
            audio_src: Arc::new(Mutex::new(None)),
            last_frame_num: Arc::new(Mutex::new(0)),
            input_dimensions: Arc::new(Mutex::new((0, 0))),
            audio_pts: Arc::new(AtomicU64::new(0)),
            video_pts: Arc::new(AtomicU64::new(0)),
            bus: Arc::new(Mutex::new(None)),
            error_message: Arc::new(Mutex::new(None)),
            direct_audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(96000))),
        })
    }

    /// Get Arc reference to the direct audio buffer (for direct sink dispatch from capture callback)
    pub fn get_audio_buffer_arc(&self) -> Arc<Mutex<Vec<f32>>> {
        self.direct_audio_buffer.clone()
    }

    /// Drain any direct audio buffer samples and push to GStreamer
    /// Call this from the render loop when using direct dispatch
    pub fn drain_direct_audio(&self) {
        let samples: Vec<f32> = {
            let mut buf = self.direct_audio_buffer.lock();
            if buf.is_empty() {
                return;
            }
            buf.drain(..).collect()
        };
        self.push_audio(&samples);
    }

    /// Push audio samples directly to GStreamer
    pub fn push_audio(&self, samples: &[f32]) {
        let state = *self.state.lock();
        if !matches!(state, StreamState::Streaming) {
            return;
        }

        // Ensure we have complete stereo sample pairs (2 samples = 8 bytes)
        let aligned_len = (samples.len() / 2) * 2;
        if aligned_len == 0 {
            return;
        }

        let audio_src = self.audio_src.lock();
        if let Some(ref src) = *audio_src {
            let bytes: Vec<u8> = samples[..aligned_len].iter()
                .flat_map(|s| s.to_le_bytes())
                .collect();

            if let Ok(mut buffer) = gst::Buffer::with_size(bytes.len()) {
                // Calculate PTS based on sample count (stereo pairs)
                let num_stereo_samples = (aligned_len / 2) as u64;
                let current_pts = self.audio_pts.fetch_add(num_stereo_samples, Ordering::SeqCst);
                let sample_rate = self.source_audio_sample_rate as u64;

                // Convert sample position to nanoseconds: (samples * 1_000_000_000) / sample_rate
                let pts_ns = (current_pts * 1_000_000_000) / sample_rate;
                let duration_ns = (num_stereo_samples * 1_000_000_000) / sample_rate;

                if let Some(buffer_mut) = buffer.get_mut() {
                    // Set PTS and duration for proper timing
                    buffer_mut.set_pts(gst::ClockTime::from_nseconds(pts_ns));
                    buffer_mut.set_duration(gst::ClockTime::from_nseconds(duration_ns));

                    if let Ok(mut map) = buffer_mut.map_writable() {
                        map.copy_from_slice(&bytes);
                    }
                }
                match src.push_buffer(buffer) {
                    Ok(_) => {
                        let prev = self.audio_frames_sent.fetch_add(num_stereo_samples, Ordering::Relaxed);
                        if prev == 0 {
                            log::info!("[GStreamer] First audio push: {} stereo samples, rate={}Hz",
                                num_stereo_samples, self.source_audio_sample_rate);
                        }
                    }
                    Err(e) => {
                        log::warn!("[GStreamer] Audio push failed: {:?}", e);
                    }
                }
            }
        }
    }

    /// Push video frame directly to GStreamer (call from render loop)
    /// Timestamps are set by appsrc (do-timestamp=true) based on wall clock.
    /// The videorate element downstream enforces constant framerate by
    /// duplicating frames when the render loop delivers below target fps.
    pub fn push_video(&self) {
        let state = *self.state.lock();
        if !matches!(state, StreamState::Streaming) {
            return;
        }

        let (width, height) = *self.input_dimensions.lock();
        if width == 0 || height == 0 {
            return;
        }

        if let Some((pixels, w, h, frame_num)) = self.frame_ring.try_read() {
            let mut last = self.last_frame_num.lock();
            if frame_num != *last && w == width && h == height {
                *last = frame_num;

                let video_src = self.video_src.lock();
                if let Some(ref src) = *video_src {
                    if let Ok(mut buffer) = gst::Buffer::with_size(pixels.len()) {
                        if let Some(buffer_mut) = buffer.get_mut() {
                            if let Ok(mut map) = buffer_mut.map_writable() {
                                map.copy_from_slice(&pixels);
                            }
                        }
                        match src.push_buffer(buffer) {
                            Ok(_) => {}
                            Err(e) => {
                                log::error!("[GStreamer] Video push failed: {:?}", e);
                            }
                        }
                        let sent = self.video_frames_sent.fetch_add(1, Ordering::Relaxed);
                        if sent == 0 {
                            log::info!("[GStreamer] First video frame pushed ({}x{}, {} bytes)", w, h, pixels.len());
                        } else if sent % 300 == 0 {
                            log::info!("[GStreamer] Video frames sent: {}", sent + 1);
                        }
                    }
                }
            }
        }
    }

    pub fn get_state(&self) -> StreamState {
        *self.state.lock()
    }

    pub fn is_streaming(&self) -> bool {
        let state = *self.state.lock();
        matches!(state, StreamState::Starting | StreamState::Streaming)
    }

    /// Drain pending pipeline-bus messages and update stream state.
    ///
    /// This is the only path by which a dropped connection or encoder error
    /// reaches the UI: GStreamer posts the error to the bus, but
    /// `bus.add_watch()` callbacks only fire under a running GLib main loop,
    /// which REACT does not run. So we poll the bus directly instead.
    fn poll_bus(&self) {
        let bus = match self.bus.lock().clone() {
            Some(bus) => bus,
            None => return,
        };
        while let Some(msg) = bus.pop_filtered(&[
            gst::MessageType::Error,
            gst::MessageType::Eos,
            gst::MessageType::Warning,
        ]) {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    let element_name = msg.src()
                        .map(|s| s.name().to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    let gst_error = err.error().to_string();
                    let debug_info = err.debug().unwrap_or_default();
                    let platform_label = self.platform.label();

                    let user_msg = if element_name.contains("rtmp") {
                        if gst_error.contains("denied") || gst_error.contains("auth")
                            || debug_info.contains("NetStream.Publish.BadName") {
                            format!("Stream key rejected by {}. Verify your stream key is correct.", platform_label)
                        } else {
                            format!("Lost connection to {}. Check your network and try again.", platform_label)
                        }
                    } else if element_name.contains("aac") || element_name.contains("audio") {
                        format!("Audio encoder error: {}", gst_error)
                    } else if element_name.contains("h264") || element_name.contains("vtenc") {
                        format!("Video encoder error: {}", gst_error)
                    } else {
                        format!("Pipeline error ({}): {}", element_name, gst_error)
                    };

                    log::error!("[GStreamer] {} (element: {}, debug: {})", user_msg, element_name, debug_info);
                    *self.error_message.lock() = Some(user_msg);
                    *self.state.lock() = StreamState::Error;
                }
                MessageView::Eos(_) => {
                    log::info!("[GStreamer] End of stream");
                    *self.state.lock() = StreamState::Stopped;
                }
                MessageView::Warning(warn) => {
                    log::warn!("[GStreamer] Pipeline warning: {} (debug: {:?})",
                        warn.error(), warn.debug());
                }
                _ => {}
            }
        }
    }

    pub fn get_stats(&self) -> StreamStats {
        // Pick up disconnects / encoder errors before reporting state.
        self.poll_bus();
        let state = *self.state.lock();
        let duration_secs = self.start_time.lock()
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);

        StreamStats {
            state,
            duration_secs,
            video_frames_sent: self.video_frames_sent.load(Ordering::Relaxed),
            audio_frames_sent: self.audio_frames_sent.load(Ordering::Relaxed),
            current_mode: self.mode,
            error_message: self.error_message.lock().clone(),
        }
    }

    pub fn start(&mut self, rtmp_url: String) -> Result<()> {
        if self.is_streaming() {
            anyhow::bail!("Already streaming");
        }

        log::info!("[GStreamer] Starting stream to {}", rtmp_url);

        *self.state.lock() = StreamState::Starting;
        *self.error_message.lock() = None;
        self.stop_flag.store(false, Ordering::Release);
        self.video_frames_sent.store(0, Ordering::Relaxed);
        self.audio_frames_sent.store(0, Ordering::Relaxed);
        self.audio_pts.store(0, Ordering::SeqCst);
        self.video_pts.store(0, Ordering::SeqCst);
        *self.start_time.lock() = Some(Instant::now());
        *self.last_frame_num.lock() = 0;

        // Clone for background thread (pipeline setup only)
        let frame_ring = self.frame_ring.clone();
        let stop_flag = self.stop_flag.clone();
        let state = self.state.clone();
        let error_msg = self.error_message.clone();
        let mode = self.mode;
        let platform = self.platform;
        let source_audio_sample_rate = self.source_audio_sample_rate;
        let pipeline_handle = self.pipeline.clone();
        let video_src_handle = self.video_src.clone();
        let audio_src_handle = self.audio_src.clone();
        let input_dims_handle = self.input_dimensions.clone();
        let bus_handle = self.bus.clone();

        std::thread::spawn(move || {
            if let Err(e) = setup_pipeline(
                rtmp_url,
                frame_ring,
                stop_flag,
                state.clone(),
                mode,
                platform,
                source_audio_sample_rate,
                pipeline_handle,
                video_src_handle,
                audio_src_handle,
                input_dims_handle,
                bus_handle,
            ) {
                log::error!("[GStreamer] Pipeline error: {}", e);
                *error_msg.lock() = Some(e.to_string());
                *state.lock() = StreamState::Error;
            }
        });

        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        // Tear down whenever a stream is live OR has errored — a dropped
        // stream still needs its pipeline shut down. Only Idle/Stopped no-op.
        if matches!(*self.state.lock(), StreamState::Idle | StreamState::Stopped) {
            return Ok(());
        }

        log::info!("[GStreamer] Stopping stream...");
        *self.state.lock() = StreamState::Stopping;
        self.stop_flag.store(true, Ordering::Release);

        // Send EOS and stop pipeline
        if let Some(ref src) = *self.video_src.lock() {
            let _ = src.end_of_stream();
        }
        if let Some(ref src) = *self.audio_src.lock() {
            let _ = src.end_of_stream();
        }
        if let Some(ref pipeline) = *self.pipeline.lock() {
            let _ = pipeline.set_state(gst::State::Null);
        }

        // Clear handles
        *self.pipeline.lock() = None;
        *self.video_src.lock() = None;
        *self.audio_src.lock() = None;
        *self.bus.lock() = None;

        *self.state.lock() = StreamState::Stopped;
        log::info!("[GStreamer] Stopped");
        Ok(())
    }
}

impl Drop for GStreamerStreamer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ============================================================================
// Pipeline Setup (runs in background thread, then returns)
// ============================================================================

fn setup_pipeline(
    rtmp_url: String,
    frame_ring: Arc<FrameRing>,
    stop_flag: Arc<AtomicBool>,
    state: Arc<Mutex<StreamState>>,
    mode: OutputMode,
    platform: StreamPlatform,
    source_audio_sample_rate: u32,
    pipeline_handle: Arc<Mutex<Option<gst::Pipeline>>>,
    video_src_handle: Arc<Mutex<Option<gst_app::AppSrc>>>,
    audio_src_handle: Arc<Mutex<Option<gst_app::AppSrc>>>,
    input_dims_handle: Arc<Mutex<(u32, u32)>>,
    bus_handle: Arc<Mutex<Option<gst::Bus>>>,
) -> Result<()> {
    // Wait for valid dimensions
    let start_wait = Instant::now();
    let (width, height) = loop {
        if stop_flag.load(Ordering::Acquire) {
            anyhow::bail!("Stopped before getting dimensions");
        }
        let dims = frame_ring.dimensions();
        if dims.0 > 0 && dims.1 > 0 {
            break dims;
        }
        if start_wait.elapsed().as_secs() > 10 {
            anyhow::bail!("Timeout waiting for frame dimensions");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    *input_dims_handle.lock() = (width, height);

    let fps = mode.fps();
    // Twitch and Kick both want per-resolution CBR, each capped at 6000 —
    // Twitch's recommended max, and IVS's safe ceiling for Kick. YouTube uses
    // the OutputMode default.
    let video_bitrate = match platform {
        StreamPlatform::Twitch => mode.twitch_video_bitrate_kbps(),
        StreamPlatform::Kick => mode.kick_video_bitrate_kbps(),
        StreamPlatform::YouTube => mode.video_bitrate_kbps(),
    };
    let audio_bitrate = mode.audio_bitrate_kbps();
    let (out_width, out_height) = mode.resolution();

    log::info!("[GStreamer] {:?} input: {}x{}, output: {}x{} @ {}fps, video={}kbps, audio={}kbps",
        platform, width, height, out_width, out_height, fps, video_bitrate, audio_bitrate);

    // Twitch/Kick: no B-frames + steady bitrate. Kick (AWS IVS) additionally
    // gets true CBR (rate-control=cbr) plus a hard 2s wall-clock keyframe cap
    // — IVS requires <=2s keyframes and drops streams whose bitrate overshoots
    // its ceiling, which ABR mode does on busy visuals. Twitch stays on ABR
    // (frame-reordering disabled = effectively CBR, matches OBS VT defaults).
    let vtenc_extra = match platform {
        StreamPlatform::Kick => "allow-frame-reordering=false rate-control=cbr max-keyframe-interval-duration=2000000000",
        StreamPlatform::Twitch => "allow-frame-reordering=false",
        StreamPlatform::YouTube => "",
    };

    // Pick the RTMP sink by scheme. `rtmpsink` is backed by rtmpdump's
    // librtmp (2011) whose RTMPS does TLS *without SNI* — so an `rtmps://`
    // handshake to an SNI-routed CDN (AWS IVS, which Kick uses) connects to a
    // default backend and the publish never reaches the channel (REACT shows
    // LIVE, the platform shows offline). `rtmp2sink` does TLS via
    // glib-networking (GnuTLS, SNI-correct) and takes the full URL with no
    // `live=1` flag. Plain `rtmp://` (Twitch/YouTube native ingest) keeps
    // `rtmpsink`, which works today.
    let rtmp_sink = if rtmp_url.starts_with("rtmps://") {
        log::info!("[GStreamer] rtmps:// ingest — using rtmp2sink (SNI-capable TLS)");
        format!("rtmp2sink location=\"{}\"", rtmp_url)
    } else {
        format!("rtmpsink location=\"{} live=1\"", rtmp_url)
    };

    // Build pipeline for low-latency live streaming
    // - Input leaky queue: absorbs encoder jitter, drops old frames rather than stalling render loop
    // - Pre-mux queues: minimal (200ms) to reduce source-side latency
    // - flvmux latency=100ms: forces mux output even if other stream hasn't arrived yet
    let pipeline_str = format!(
        r#"
        appsrc name=video_src format=time is-live=true do-timestamp=true !
        queue max-size-buffers=5 max-size-time=200000000 leaky=downstream !
        videoflip method=vertical-flip !
        videoconvert !
        videoscale !
        video/x-raw,width={out_width},height={out_height},framerate={fps}/1 !
        vtenc_h264 bitrate={video_kbps} max-keyframe-interval={gop} realtime=true {vtenc_extra} !
        h264parse !
        queue max-size-buffers=5 max-size-time=200000000 !
        flvmux name=mux streamable=true latency=100000000 !
        {rtmp_sink}

        appsrc name=audio_src format=time is-live=true do-timestamp=true !
        audioconvert !
        audioresample !
        fdkaacenc bitrate={audio_bps} !
        queue max-size-buffers=5 max-size-time=200000000 !
        mux.
        "#,
        out_width = out_width,
        out_height = out_height,
        fps = fps,
        video_kbps = video_bitrate,
        audio_bps = audio_bitrate * 1000,
        gop = fps * 2,
        vtenc_extra = vtenc_extra,
        rtmp_sink = rtmp_sink,
    );

    log::debug!("[GStreamer] Pipeline string:\n{}", pipeline_str);
    let pipeline = gst::parse::launch(&pipeline_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse pipeline: {}", e))?
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("Failed to cast to Pipeline"))?;

    // Get appsrc elements
    let video_src = pipeline
        .by_name("video_src")
        .and_then(|e| e.dynamic_cast::<gst_app::AppSrc>().ok())
        .context("Failed to get video appsrc")?;

    let audio_src = pipeline
        .by_name("audio_src")
        .and_then(|e| e.dynamic_cast::<gst_app::AppSrc>().ok())
        .context("Failed to get audio appsrc")?;

    // Set caps
    let video_caps = gst_video::VideoCapsBuilder::new()
        .format(gst_video::VideoFormat::Rgba)
        .width(width as i32)
        .height(height as i32)
        .framerate(gst::Fraction::new(fps as i32, 1))
        .build();
    video_src.set_caps(Some(&video_caps));

    let audio_caps = gst_audio::AudioCapsBuilder::new()
        .format(gst_audio::AudioFormat::F32le)
        .rate(source_audio_sample_rate as i32)
        .channels(2)
        .layout(gst_audio::AudioLayout::Interleaved)
        .build();
    audio_src.set_caps(Some(&audio_caps));

    // Store handles for direct push access
    *pipeline_handle.lock() = Some(pipeline.clone());
    *video_src_handle.lock() = Some(video_src);
    *audio_src_handle.lock() = Some(audio_src);

    // Store the pipeline bus for polling. We deliberately do NOT use
    // bus.add_watch(): its callback only fires under a running GLib main loop
    // (which REACT doesn't run), and this setup thread exits as soon as setup
    // returns. GStreamerStreamer::poll_bus() drains this bus from get_stats()
    // every frame instead — that's what surfaces a dropped connection to the UI.
    let bus = pipeline.bus().context("Failed to get pipeline bus")?;
    *bus_handle.lock() = Some(bus);

    // Verify critical elements are available before starting
    let registry = gst::Registry::get();
    for elem_name in &["vtenc_h264", "h264parse", "fdkaacenc", "rtmpsink", "flvmux", "videoflip", "videoconvert", "appsrc"] {
        let factory = registry.find_feature(*elem_name, gst::ElementFactory::static_type());
        if factory.is_some() {
            log::info!("[GStreamer] Element available: {}", elem_name);
        } else {
            log::error!("[GStreamer] Element NOT FOUND in registry: {}", elem_name);
        }
    }

    // Start pipeline - state change is async since rtmpsink needs data to complete
    // the RTMP handshake. We allow data to flow immediately and rely on bus watch
    // for error detection.
    pipeline.set_state(gst::State::Playing)?;
    *state.lock() = StreamState::Streaming;
    log::info!("[GStreamer] Pipeline started - streaming data, RTMP connecting...");

    Ok(())
}
