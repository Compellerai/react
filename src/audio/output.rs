//! Audio output for scrub playback
//!
//! Plays audio samples to the default output device using cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::Arc;

/// Audio output for playing scrub audio
pub struct AudioOutput {
    /// Output stream (kept alive)
    _stream: Option<cpal::Stream>,
    /// Ring buffer for samples to play
    buffer: Arc<Mutex<ScrubBuffer>>,
    /// Whether output is active
    active: bool,
    /// Sample rate
    sample_rate: u32,
}

/// Ring buffer for scrub audio
struct ScrubBuffer {
    samples: Vec<f32>,
    read_pos: usize,
    write_pos: usize,
    capacity: usize,
}

impl ScrubBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            samples: vec![0.0; capacity],
            read_pos: 0,
            write_pos: 0,
            capacity,
        }
    }

    fn write(&mut self, samples: &[f32]) {
        for &s in samples {
            self.samples[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
    }

    fn read(&mut self, output: &mut [f32]) {
        for out in output.iter_mut() {
            *out = self.samples[self.read_pos];
            self.read_pos = (self.read_pos + 1) % self.capacity;
        }
    }

    fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.capacity - self.read_pos + self.write_pos
        }
    }
}

impl AudioOutput {
    /// Create new audio output (inactive by default)
    pub fn new() -> Self {
        Self {
            _stream: None,
            buffer: Arc::new(Mutex::new(ScrubBuffer::new(48000))), // 1 second buffer
            active: false,
            sample_rate: 48000,
        }
    }

    /// Start audio output stream
    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.active {
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device found"))?;

        let config = device.default_output_config()?;
        self.sample_rate = config.sample_rate().0;

        let buffer = self.buffer.clone();
        let channels = config.channels() as usize;

        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut buf = buffer.lock();
                let available = buf.available();

                if available >= data.len() {
                    // Read interleaved samples
                    buf.read(data);
                } else {
                    // Not enough data - output silence
                    data.fill(0.0);
                }
            },
            |err| {
                log::error!("[AudioOutput] Stream error: {}", err);
            },
            None,
        )?;

        stream.play()?;
        self._stream = Some(stream);
        self.active = true;

        log::info!("[AudioOutput] Started at {}Hz, {} channels", self.sample_rate, channels);
        Ok(())
    }

    /// Stop audio output
    pub fn stop(&mut self) {
        self._stream = None;
        self.active = false;
        log::info!("[AudioOutput] Stopped");
    }

    /// Push samples to play
    pub fn push_samples(&self, samples: &[f32]) {
        if self.active {
            self.buffer.lock().write(samples);
        }
    }

    /// Check if output is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl Default for AudioOutput {
    fn default() -> Self {
        Self::new()
    }
}
