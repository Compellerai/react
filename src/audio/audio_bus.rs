//! AudioBus: canonical audio distribution from capture to all consumers.
//!
//! Creates one SPSC ring buffer per output destination. Capture callbacks
//! write complete stereo frames `[f32; 2]` to all producers. Each consumer
//! (viz, meters, NDI, recorder, RTMP) drains independently.
//!
//! Contracts:
//! - All queues carry `[f32; 2]` frames at canonical 48 kHz.
//! - Producers never block — if a consumer falls behind, the block is dropped
//!   for that destination and the drop counter incremented.
//! - Stereo desync is structurally impossible (complete frames only).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Canonical sample rate for the audio bus (all backends normalize to this).
pub const CANON_RATE_HZ: u32 = 48_000;

/// Frames per output chunk (20 ms at 48 kHz).
pub const CHUNK_FRAMES: usize = 960;

/// Ring buffer capacity per output destination (~250 ms at 48 kHz).
/// At 20 ms drain cadence this gives ~12 chunks of headroom.
pub const QUEUE_FRAMES: usize = 12_000;

/// Stereo frame type: [Left, Right]. Pushing a partial frame is impossible.
pub type AudioFrame = [f32; 2];

/// Per-destination drop statistics.
#[derive(Clone)]
pub struct AudioBusStats {
    pub drops_viz: Arc<AtomicU64>,
    pub drops_meters: Arc<AtomicU64>,
    pub drops_ndi: Arc<AtomicU64>,
    pub drops_rec: Arc<AtomicU64>,
    pub drops_rtmp: Arc<AtomicU64>,
}

impl AudioBusStats {
    fn new() -> Self {
        Self {
            drops_viz: Arc::new(AtomicU64::new(0)),
            drops_meters: Arc::new(AtomicU64::new(0)),
            drops_ndi: Arc::new(AtomicU64::new(0)),
            drops_rec: Arc::new(AtomicU64::new(0)),
            drops_rtmp: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Total frames dropped across all destinations.
    pub fn total_drops(&self) -> u64 {
        self.drops_viz.load(Ordering::Relaxed)
            + self.drops_meters.load(Ordering::Relaxed)
            + self.drops_ndi.load(Ordering::Relaxed)
            + self.drops_rec.load(Ordering::Relaxed)
            + self.drops_rtmp.load(Ordering::Relaxed)
    }
}

/// All ring buffer producers — moved into the capture callback.
pub struct AudioBusProducers {
    pub viz: rtrb::Producer<AudioFrame>,
    pub meters: rtrb::Producer<AudioFrame>,
    pub ndi: rtrb::Producer<AudioFrame>,
    pub rec: rtrb::Producer<AudioFrame>,
    pub rtmp: rtrb::Producer<AudioFrame>,
    pub stats: AudioBusStats,
}

/// All ring buffer consumers — distributed to output subsystems.
pub struct AudioBusConsumers {
    pub viz: rtrb::Consumer<AudioFrame>,
    pub meters: rtrb::Consumer<AudioFrame>,
    pub ndi: rtrb::Consumer<AudioFrame>,
    pub rec: rtrb::Consumer<AudioFrame>,
    pub rtmp: rtrb::Consumer<AudioFrame>,
    pub stats: AudioBusStats,
}

/// Create a fresh audio bus: returns (producers, consumers).
///
/// Call this when capture starts. Hand producers to the capture callback,
/// hand consumers to viz/meters/output subsystems.
pub fn create_audio_bus() -> (AudioBusProducers, AudioBusConsumers) {
    let (viz_p, viz_c) = rtrb::RingBuffer::<AudioFrame>::new(QUEUE_FRAMES);
    let (meters_p, meters_c) = rtrb::RingBuffer::<AudioFrame>::new(QUEUE_FRAMES);
    let (ndi_p, ndi_c) = rtrb::RingBuffer::<AudioFrame>::new(QUEUE_FRAMES);
    let (rec_p, rec_c) = rtrb::RingBuffer::<AudioFrame>::new(QUEUE_FRAMES);
    let (rtmp_p, rtmp_c) = rtrb::RingBuffer::<AudioFrame>::new(QUEUE_FRAMES);

    let stats = AudioBusStats::new();

    let producers = AudioBusProducers {
        viz: viz_p,
        meters: meters_p,
        ndi: ndi_p,
        rec: rec_p,
        rtmp: rtmp_p,
        stats: stats.clone(),
    };

    let consumers = AudioBusConsumers {
        viz: viz_c,
        meters: meters_c,
        ndi: ndi_c,
        rec: rec_c,
        rtmp: rtmp_c,
        stats,
    };

    log::info!("[AudioBus] Created bus: {} frames/queue ({}ms), chunk={} frames ({}ms)",
        QUEUE_FRAMES, QUEUE_FRAMES * 1000 / CANON_RATE_HZ as usize,
        CHUNK_FRAMES, CHUNK_FRAMES * 1000 / CANON_RATE_HZ as usize);

    (producers, consumers)
}

/// Push a block of interleaved stereo samples to a single producer.
/// Drops the entire block if the ring is full (never blocks).
/// `data` must be even-length interleaved `[L, R, L, R, ...]`.
#[inline]
pub fn push_block(
    producer: &mut rtrb::Producer<AudioFrame>,
    data: &[f32],
    drop_counter: &AtomicU64,
) {
    let frame_count = data.len() / 2;
    if frame_count == 0 {
        return;
    }

    // Check if the ring has enough space for the whole block
    if producer.slots() < frame_count {
        // Drop the entire block for this destination
        drop_counter.fetch_add(frame_count as u64, Ordering::Relaxed);
        return;
    }

    // Push all frames (space was verified above, so this won't fail)
    for chunk in data.chunks_exact(2) {
        let _ = producer.push([chunk[0], chunk[1]]);
    }
}

/// Push all frames to all producers. Drops per-destination if a ring is full.
/// `data` must be even-length interleaved stereo.
#[inline]
pub fn fanout(producers: &mut AudioBusProducers, data: &[f32]) {
    push_block(&mut producers.viz, data, &producers.stats.drops_viz);
    push_block(&mut producers.meters, data, &producers.stats.drops_meters);
    push_block(&mut producers.ndi, data, &producers.stats.drops_ndi);
    push_block(&mut producers.rec, data, &producers.stats.drops_rec);
    push_block(&mut producers.rtmp, data, &producers.stats.drops_rtmp);
}

/// Drain up to `max_frames` from a consumer into an interleaved f32 buffer.
/// Returns the number of frames actually drained.
#[inline]
pub fn drain_frames(
    consumer: &mut rtrb::Consumer<AudioFrame>,
    out: &mut Vec<f32>,
    max_frames: usize,
) -> usize {
    let mut count = 0;
    for _ in 0..max_frames {
        match consumer.pop() {
            Ok(frame) => {
                out.push(frame[0]);
                out.push(frame[1]);
                count += 1;
            }
            Err(_) => break,
        }
    }
    count
}

/// Drain exactly `CHUNK_FRAMES` from a consumer, padding with zeros if short.
/// Returns interleaved f32 buffer of exactly `CHUNK_FRAMES * 2` samples.
#[inline]
pub fn drain_chunk_padded(
    consumer: &mut rtrb::Consumer<AudioFrame>,
    out: &mut [f32; CHUNK_FRAMES * 2],
) -> usize {
    let mut frames_read = 0;
    for i in 0..CHUNK_FRAMES {
        match consumer.pop() {
            Ok(frame) => {
                out[i * 2] = frame[0];
                out[i * 2 + 1] = frame[1];
                frames_read += 1;
            }
            Err(_) => {
                // Pad remainder with zeros
                for j in i..CHUNK_FRAMES {
                    out[j * 2] = 0.0;
                    out[j * 2 + 1] = 0.0;
                }
                break;
            }
        }
    }
    frames_read
}
