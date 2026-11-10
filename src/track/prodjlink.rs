//! Pro DJ Link Network Source adapter
//!
//! REACT integration wrapper around the standalone prodjlink-rs crate.

use super::{CdjDeviceInfo, TrackInfo, TrackSource};

/// Pro DJ Link network source - discovers and monitors Pioneer/AlphaTheta CDJs.
pub struct ProDjLinkSource {
    client: prodjlink_rs::ProDjLinkClient,
}

impl ProDjLinkSource {
    /// Create a new Pro DJ Link source and start listening for CDJs.
    pub fn new() -> Self {
        Self {
            client: prodjlink_rs::ProDjLinkClient::new(),
        }
    }

    /// Get a status message for UI display.
    pub fn get_status_message(&self) -> String {
        self.client.get_status_message()
    }

    /// Get list of discovered devices.
    pub fn get_devices(&self) -> Vec<(u8, String, bool)> {
        self.client.get_devices()
    }

    /// Check if any devices are connected.
    pub fn has_devices(&self) -> bool {
        self.client.has_devices()
    }
}

impl Default for ProDjLinkSource {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackSource for ProDjLinkSource {
    fn name(&self) -> &str {
        self.client.name()
    }

    fn get_current_track(&mut self) -> Option<TrackInfo> {
        self.client.current_track().map(Into::into)
    }

    fn is_available(&self) -> bool {
        self.client.is_available()
    }

    fn status_message(&self) -> Option<String> {
        Some(self.get_status_message())
    }

    fn get_cdj_devices(&self) -> Vec<CdjDeviceInfo> {
        self.client
            .cdj_devices()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    fn master_beat(&self) -> Option<prodjlink_rs::MasterBeat> {
        self.client.master_beat()
    }
}

impl From<prodjlink_rs::CdjDeviceInfo> for CdjDeviceInfo {
    fn from(device: prodjlink_rs::CdjDeviceInfo) -> Self {
        Self {
            device_id: device.device_id,
            name: device.name,
            ip: device.ip,
            track_loaded: device.track_loaded,
            is_playing: device.is_playing,
            is_looping: device.is_looping,
            is_master: device.is_master,
            is_on_air: device.is_on_air,
            bpm: device.bpm,
            track_title: device.track_title,
            track_artist: device.track_artist,
        }
    }
}

impl From<prodjlink_rs::TrackInfo> for TrackInfo {
    fn from(track: prodjlink_rs::TrackInfo) -> Self {
        Self {
            title: track.title,
            artist: track.artist,
            album: track.album,
            duration: track.duration,
            position: track.position,
            is_playing: track.is_playing,
            bpm: track.bpm,
            source: track.source,
            deck: track.deck,
            artwork: track.artwork,
        }
    }
}
