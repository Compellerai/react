use ola_rs::{OlaClient, OlaConfig};
use std::io;

/// OLA backend using Compeller's public `ola-rs` crate.
///
/// This path talks to `olad` over TCP, then lets OLA handle the configured
/// output plugin: Art-Net, USB DMX, sACN, or whatever the venue operator has
/// patched in OLA. REACT still owns the fixture patch and blackout semantics.
pub struct OlaDmxSender {
    client: OlaClient,
    host: String,
    port: u16,
}

impl OlaDmxSender {
    pub fn connect(host: String, port: u16) -> io::Result<Self> {
        let config = OlaConfig {
            host: host.clone(),
            port,
            ..OlaConfig::default()
        };
        let client = OlaClient::connect(config).map_err(to_io_error)?;
        Ok(Self { client, host, port })
    }

    /// The configured `host:port` olad endpoint (for the operator status line).
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn send_frame(&mut self, universe: u16, data: &[u8]) -> io::Result<usize> {
        self.client
            .stream_dmx(i32::from(universe), data, None)
            .map_err(to_io_error)?;
        Ok(data.len())
    }
}

fn to_io_error(err: ola_rs::OlaError) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err.to_string())
}
