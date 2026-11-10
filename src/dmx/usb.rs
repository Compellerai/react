//! USB DMX output for FTDI "Open DMX USB" cables (FT232 / FT232RL bit-bang DMX512).
//!
//! These cables — Enttec Open DMX USB, and the generic FT232RL USB→DMX leads sold
//! for FreeStyler etc. — have NO microcontroller. Unlike the Enttec USB *Pro* there
//! is no `0x7E`/`0xE7` framed protocol: DMX512 is bit-banged straight down the FTDI
//! UART (250 kbaud 8N2, a BREAK + mark-after-break before each frame, and continuous
//! refresh because the cable has no frame buffer).
//!
//! The [`open_dmx`] crate owns all of that (serial port + break timing + a private
//! background refresh thread), so this is a thin wrapper that slots into REACT's
//! `DmxOutput` exactly like [`crate::dmx::ArtNetSender`] / [`crate::dmx::OlaDmxSender`].
//!
//! Channel mapping: REACT's frame is 512 channel bytes with `data[0]` = DMX channel 1
//! and no start code; `DMXSerial::set_channels([u8; 512])` takes the same and prepends
//! the null start code itself.

use std::io;

use open_dmx::DMXSerial;

const DMX_SLOTS: usize = 512;

/// USB (Open DMX) sender. `open_dmx::DMXSerial` runs its own background thread that
/// continuously re-transmits the latest channel buffer to the FTDI cable; `send_frame`
/// only swaps that buffer, so the render thread never eats the ~22 ms serial write.
pub struct UsbDmxSender {
    dmx: DMXSerial,
    port_name: String,
}

impl UsbDmxSender {
    /// Open `port_name` (e.g. `/dev/cu.usbserial-AB0MWWLR`) and start streaming.
    /// Returns `Err` if the port can't be opened so the caller falls back to no
    /// output — the same graceful pattern as the Art-Net / OLA builders.
    pub fn open(port_name: String) -> io::Result<Self> {
        let dmx = DMXSerial::open(&port_name)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        Ok(Self { dmx, port_name })
    }

    /// Push the latest DMX frame. `universe` is ignored — a single Open DMX cable
    /// carries one universe. Infallible at this layer (`set_channels` buffers to the
    /// background thread), so it never spams the per-frame error log even if the
    /// cable is unplugged mid-show.
    pub fn send_frame(&mut self, _universe: u16, data: &[u8]) -> io::Result<usize> {
        let frame = to_dmx_frame(data);
        self.dmx.set_channels(frame);
        Ok(data.len().min(DMX_SLOTS))
    }

    /// Whether the cable's background agent is still alive (drives the UI status dot).
    pub fn connected(&self) -> bool {
        self.dmx.check_agent().is_ok()
    }

    /// The serial port this sender is streaming to.
    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Best-effort auto-detect of an FTDI USB-serial cable when no explicit port is
    /// configured: the first `/dev` entry that looks like an FTDI callout device
    /// (macOS `cu.usbserial*`) or a Linux FTDI tty (`ttyUSB*`).
    pub fn autodetect_port() -> Option<String> {
        let mut found: Vec<String> = std::fs::read_dir("/dev")
            .ok()?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.starts_with("cu.usbserial") || name.starts_with("ttyUSB"))
            .map(|name| format!("/dev/{name}"))
            .collect();
        found.sort();
        match found.first() {
            None => None,
            Some(chosen) => {
                if found.len() > 1 {
                    log::info!(
                        "[DMX] {} USB serial cables found; auto-selected {} \
                         (set REACT_DMX_USB_PORT or pick in the DMX panel to choose another: {:?})",
                        found.len(),
                        chosen,
                        found
                    );
                } else {
                    log::info!("[DMX] auto-detected USB serial cable at {}", chosen);
                }
                Some(chosen.clone())
            }
        }
    }
}

/// Map REACT's channel data (`data[0]` = DMX channel 1, no start code) into the fixed
/// `[u8; 512]` slot buffer `open_dmx` wants. Extra bytes truncate; a short frame leaves
/// the remaining slots at 0.
fn to_dmx_frame(data: &[u8]) -> [u8; DMX_SLOTS] {
    let mut frame = [0u8; DMX_SLOTS];
    let n = data.len().min(DMX_SLOTS);
    frame[..n].copy_from_slice(&data[..n]);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmx_frame_maps_channels_one_indexed() {
        let mut data = vec![0u8; DMX_SLOTS];
        data[0] = 255; // DMX channel 1
        data[511] = 7; // DMX channel 512
        let frame = to_dmx_frame(&data);
        assert_eq!(frame[0], 255, "channel 1 lands in slot 0");
        assert_eq!(frame[511], 7, "channel 512 lands in slot 511");
    }

    #[test]
    fn dmx_frame_truncates_oversized_input() {
        let data = vec![9u8; DMX_SLOTS + 16];
        let frame = to_dmx_frame(&data);
        assert_eq!(frame.len(), DMX_SLOTS);
        assert_eq!(frame[DMX_SLOTS - 1], 9);
    }

    #[test]
    fn dmx_frame_zero_pads_short_input() {
        let data = vec![5u8; 4];
        let frame = to_dmx_frame(&data);
        assert_eq!(&frame[..4], &[5, 5, 5, 5]);
        assert_eq!(frame[4], 0, "unspecified slots default to 0");
    }
}
