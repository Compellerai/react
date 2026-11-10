//! MaestroDMX REST API Client
//!
//! Fetches patterns, palettes, and shows from MaestroDMX's REST API
//! to dynamically populate UI selectors.

use serde::Deserialize;
use std::time::Duration;

// ============================================================================
// API RESPONSE TYPES
// ============================================================================

/// Raw API response with "ids" array (MaestroDMX format)
#[derive(Debug, Clone, Deserialize)]
struct IdsResponse {
    ids: Vec<String>,
}

/// Pattern from MaestroDMX API
#[derive(Debug, Clone)]
pub struct MaestroPattern {
    pub index: i32,
    pub name: String,
}

/// Palette from MaestroDMX API (used for both color and FX palettes)
#[derive(Debug, Clone)]
pub struct MaestroPalette {
    pub index: i32,
    pub name: String,
}

/// Show from MaestroDMX API
#[derive(Debug, Clone, Deserialize)]
pub struct MaestroShow {
    pub uuid: String,
    pub name: String,
}

/// Convert IdsResponse to Vec<MaestroPattern>
fn ids_to_patterns(response: IdsResponse) -> Vec<MaestroPattern> {
    response.ids.into_iter().enumerate().map(|(i, name)| {
        MaestroPattern {
            index: i as i32,
            name,
        }
    }).collect()
}

/// Convert IdsResponse to Vec<MaestroPalette>
fn ids_to_palettes(response: IdsResponse) -> Vec<MaestroPalette> {
    response.ids.into_iter().enumerate().map(|(i, name)| {
        MaestroPalette {
            index: i as i32,
            name,
        }
    }).collect()
}

// ============================================================================
// MAESTRO DATA STATE
// ============================================================================

/// Cached data from MaestroDMX API
#[derive(Debug, Clone, Default)]
pub struct MaestroData {
    /// Available patterns
    pub patterns: Vec<MaestroPattern>,
    /// Available color palettes
    pub color_palettes: Vec<MaestroPalette>,
    /// Available FX palettes
    pub fx_palettes: Vec<MaestroPalette>,
    /// Available shows
    pub shows: Vec<MaestroShow>,
    /// Whether we're currently loading data
    pub loading: bool,
    /// Error message if fetch failed
    pub error: Option<String>,
    /// Last IP we fetched from (to detect changes)
    pub last_fetch_ip: Option<String>,
}

impl MaestroData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if we have valid data
    pub fn has_data(&self) -> bool {
        !self.patterns.is_empty() || !self.color_palettes.is_empty()
    }

    /// Check if we need to fetch data for the given IP
    /// Returns true if we don't have data, are not loading, and haven't failed for this IP
    pub fn needs_fetch(&self, ip: &str) -> bool {
        if self.loading {
            return false;
        }
        // If we have data from this IP, no need to refetch
        if self.has_data() {
            if let Some(ref last_ip) = self.last_fetch_ip {
                if last_ip == ip {
                    return false;
                }
            }
        }
        // If we already tried and failed for this IP, don't retry automatically
        if self.error.is_some() {
            if let Some(ref last_ip) = self.last_fetch_ip {
                if last_ip == ip {
                    return false;
                }
            }
        }
        true
    }

    /// Clear all data
    pub fn clear(&mut self) {
        self.patterns.clear();
        self.color_palettes.clear();
        self.fx_palettes.clear();
        self.shows.clear();
        self.error = None;
        self.last_fetch_ip = None;
    }
}

// ============================================================================
// API CLIENT
// ============================================================================

/// Fetch all MaestroDMX data from the REST API
///
/// This is a blocking function intended to be called from a background thread.
/// Returns the populated MaestroData or an error.
pub fn fetch_maestro_data(ip: &str) -> Result<MaestroData, String> {
    // MaestroDMX REST API runs on port 80 (default HTTP)
    let base_url = format!("http://{}/api/v1", ip);
    log::info!("[MaestroAPI] Connecting to {}", base_url);

    let client = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .build();

    let mut data = MaestroData::new();
    data.last_fetch_ip = Some(ip.to_string());

    // Fetch patterns - API returns {"ids": ["Still", "Beam", ...]}
    let patterns_url = format!("{}/patterns/available", base_url);
    log::debug!("[MaestroAPI] GET {}", patterns_url);
    match fetch_json::<IdsResponse>(&client, &patterns_url) {
        Ok(response) => {
            let patterns = ids_to_patterns(response);
            log::info!("[MaestroAPI] Fetched {} patterns: {:?}", patterns.len(),
                patterns.iter().map(|p| &p.name).collect::<Vec<_>>());
            data.patterns = patterns;
        }
        Err(e) => {
            log::warn!("[MaestroAPI] Failed to fetch patterns from {}: {}", patterns_url, e);
        }
    }

    // Fetch color palettes - API returns {"ids": ["Cool White", "Cyan", ...]}
    let palettes_url = format!("{}/palettes/color", base_url);
    log::debug!("[MaestroAPI] GET {}", palettes_url);
    match fetch_json::<IdsResponse>(&client, &palettes_url) {
        Ok(response) => {
            let palettes = ids_to_palettes(response);
            log::info!("[MaestroAPI] Fetched {} color palettes: {:?}", palettes.len(),
                palettes.iter().map(|p| &p.name).collect::<Vec<_>>());
            data.color_palettes = palettes;
        }
        Err(e) => {
            log::warn!("[MaestroAPI] Failed to fetch color palettes from {}: {}", palettes_url, e);
        }
    }

    // Fetch FX palettes - API returns {"ids": ["Subtle", ...]}
    let fx_url = format!("{}/palettes/fx", base_url);
    log::debug!("[MaestroAPI] GET {}", fx_url);
    match fetch_json::<IdsResponse>(&client, &fx_url) {
        Ok(response) => {
            let palettes = ids_to_palettes(response);
            log::info!("[MaestroAPI] Fetched {} FX palettes: {:?}", palettes.len(),
                palettes.iter().map(|p| &p.name).collect::<Vec<_>>());
            data.fx_palettes = palettes;
        }
        Err(e) => {
            log::warn!("[MaestroAPI] Failed to fetch FX palettes from {}: {}", fx_url, e);
        }
    }

    // Fetch shows - API returns {"ids": [...]} or similar
    let shows_url = format!("{}/shows", base_url);
    log::debug!("[MaestroAPI] GET {}", shows_url);
    match fetch_json::<IdsResponse>(&client, &shows_url) {
        Ok(response) => {
            let shows: Vec<MaestroShow> = response.ids.into_iter().map(|name| {
                MaestroShow {
                    uuid: name.clone(),
                    name,
                }
            }).collect();
            log::info!("[MaestroAPI] Fetched {} shows: {:?}", shows.len(),
                shows.iter().map(|s| &s.name).collect::<Vec<_>>());
            data.shows = shows;
        }
        Err(e) => {
            log::warn!("[MaestroAPI] Failed to fetch shows from {}: {}", shows_url, e);
        }
    }

    // If we got nothing at all, return an error
    if data.patterns.is_empty() && data.color_palettes.is_empty() && data.fx_palettes.is_empty() && data.shows.is_empty() {
        let err = format!("Could not connect to MaestroDMX at {} - check if device is online", ip);
        log::error!("[MaestroAPI] {}", err);
        return Err(err);
    }

    log::info!("[MaestroAPI] Successfully fetched data from {}: {} patterns, {} color palettes, {} FX palettes, {} shows",
        ip, data.patterns.len(), data.color_palettes.len(), data.fx_palettes.len(), data.shows.len());
    Ok(data)
}

/// Fetch JSON from a URL
fn fetch_json<T: serde::de::DeserializeOwned>(client: &ureq::Agent, url: &str) -> Result<T, String> {
    let response = client
        .get(url)
        .call()
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let body = response
        .into_string()
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse JSON: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maestro_data_default() {
        let data = MaestroData::new();
        assert!(!data.has_data());
        assert!(data.patterns.is_empty());
        assert!(data.color_palettes.is_empty());
        assert!(data.fx_palettes.is_empty());
        assert!(data.shows.is_empty());
        assert!(!data.loading);
        assert!(data.error.is_none());
        assert!(data.last_fetch_ip.is_none());
    }

    #[test]
    fn test_maestro_data_has_data() {
        let mut data = MaestroData::new();
        assert!(!data.has_data());

        data.patterns.push(MaestroPattern {
            index: 0,
            name: "Test".to_string(),
        });
        assert!(data.has_data());
    }

    #[test]
    fn test_maestro_data_clear() {
        let mut data = MaestroData::new();
        data.patterns.push(MaestroPattern {
            index: 0,
            name: "Test".to_string(),
        });
        data.last_fetch_ip = Some("127.0.0.1".to_string());
        data.error = Some("Error".to_string());

        data.clear();
        assert!(!data.has_data());
        assert!(data.last_fetch_ip.is_none());
        assert!(data.error.is_none());
    }
}
