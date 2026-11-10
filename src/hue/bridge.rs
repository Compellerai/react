//! Philips Hue bridge discovery, push-link authentication, and entertainment config
//!
//! Uses the Hue v1 REST API over HTTP and v2 CLIP API where needed.
//!
//! ## Auth flow
//! 1. POST `http://{ip}/api` with `{"devicetype":"compeller-react#desktop","generateclientkey":true}`
//! 2. User presses bridge button within ~30 seconds
//! 3. Bridge returns `username` + `clientkey` (PSK identity for DTLS)
//!
//! ## Entertainment config
//! - GET `/api/{username}/groups` lists groups; filter by `type == "Entertainment"`
//! - PUT `/api/{username}/groups/{id}` with `{"stream":{"active":true}}` to activate

use std::time::Duration;

/// Push-link pair with bridge. Polls for up to 30 seconds waiting for button press.
/// Returns (username, clientkey) on success.
pub fn push_link_pair(bridge_ip: &str) -> Result<(String, String), String> {
    let url = format!("http://{}/api", bridge_ip);
    let body = serde_json::json!({
        "devicetype": "compeller-react#desktop",
        "generateclientkey": true
    });

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .build();

    // Poll for up to 30 seconds (the user needs to press the bridge button)
    let max_attempts = 30;
    for attempt in 0..max_attempts {
        log::info!("[Hue] Pairing attempt {}/{}...", attempt + 1, max_attempts);

        match agent.post(&url).send_json(&body) {
            Ok(response) => {
                let response_text = response.into_string()
                    .map_err(|e| format!("Failed to read response: {}", e))?;

                // Parse response — it's always a JSON array
                let parsed: serde_json::Value = serde_json::from_str(&response_text)
                    .map_err(|e| format!("Invalid JSON: {}", e))?;

                if let Some(arr) = parsed.as_array() {
                    if let Some(first) = arr.first() {
                        // Check for success
                        if let Some(success) = first.get("success") {
                            let username = success.get("username")
                                .and_then(|v| v.as_str())
                                .ok_or("Missing username in response")?
                                .to_string();
                            let clientkey = success.get("clientkey")
                                .and_then(|v| v.as_str())
                                .ok_or("Missing clientkey in response")?
                                .to_string();
                            return Ok((username, clientkey));
                        }

                        // Check for error (101 = link button not pressed)
                        if let Some(error) = first.get("error") {
                            let error_type = error.get("type")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0);
                            if error_type == 101 {
                                // Button not pressed yet, keep polling
                                std::thread::sleep(Duration::from_secs(1));
                                continue;
                            }
                            let desc = error.get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown error");
                            return Err(format!("Bridge error: {}", desc));
                        }
                    }
                }

                return Err(format!("Unexpected response: {}", response_text));
            }
            Err(e) => {
                // Network error — might be transient
                if attempt < max_attempts - 1 {
                    log::warn!("[Hue] Request failed, retrying: {}", e);
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                return Err(format!("Connection failed: {}", e));
            }
        }
    }

    Err("Timed out waiting for bridge button press".to_string())
}

/// Fetch available entertainment groups from the bridge (v1 API)
/// Returns Vec of (group_id, group_name)
pub fn fetch_entertainment_areas(bridge_ip: &str, username: &str) -> Result<Vec<(String, String)>, String> {
    let url = format!("http://{}/api/{}/groups", bridge_ip, username);

    let response = http_get(&url)?;
    let parsed: serde_json::Value = serde_json::from_str(&response)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let mut areas = Vec::new();
    if let Some(obj) = parsed.as_object() {
        for (id, group) in obj {
            let group_type = group.get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if group_type == "Entertainment" {
                let name = group.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unnamed Area")
                    .to_string();
                areas.push((id.clone(), name));
            }
        }
    }

    Ok(areas)
}

/// Light info fetched from the Hue bridge API
pub struct HueLightInfo {
    /// v1 API resource ID (used in DTLS streaming packets)
    pub id: u16,
    /// Human-readable light name
    pub name: String,
    /// Archetype string from bridge (e.g. "sultanbulb", "huelightstrip", "spotbulb")
    pub archetype: String,
}

/// Fetch lights in a specific entertainment group (v1 API)
/// Returns Vec of HueLightInfo with archetype for form factor detection
pub fn fetch_area_lights(bridge_ip: &str, username: &str, group_id: &str) -> Result<Vec<HueLightInfo>, String> {
    // First get the group to find light IDs
    let group_url = format!("http://{}/api/{}/groups/{}", bridge_ip, username, group_id);
    let group_response = http_get(&group_url)?;
    let group: serde_json::Value = serde_json::from_str(&group_response)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let light_ids: Vec<String> = group.get("lights")
        .and_then(|l| l.as_array())
        .map(|arr| arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect())
        .unwrap_or_default();

    if light_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Fetch all lights to get names and archetypes
    let lights_url = format!("http://{}/api/{}/lights", bridge_ip, username);
    let lights_response = http_get(&lights_url)?;
    let all_lights: serde_json::Value = serde_json::from_str(&lights_response)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let mut result = Vec::new();
    for light_id in &light_ids {
        let light = all_lights.get(light_id);
        let name = light
            .and_then(|l| l.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Light")
            .to_string();
        // Extract archetype: try config.archetype first, then top-level type, else "unknown"
        let archetype = light
            .and_then(|l| l.get("config"))
            .and_then(|c| c.get("archetype"))
            .and_then(|v| v.as_str())
            .or_else(|| light.and_then(|l| l.get("type")).and_then(|v| v.as_str()))
            .unwrap_or("unknown")
            .to_string();
        // Use the v1 API resource ID as light_id (matches streaming packet format)
        let id = light_id.parse::<u16>().unwrap_or(0);
        result.push(HueLightInfo { id, name, archetype });
    }

    Ok(result)
}

/// Activate entertainment streaming mode on a specific group (v1 API)
pub fn activate_entertainment(bridge_ip: &str, username: &str, group_id: &str) -> Result<(), String> {
    let url = format!("http://{}/api/{}/groups/{}", bridge_ip, username, group_id);
    let body = serde_json::json!({
        "stream": {
            "active": true
        }
    });

    log::info!("[Hue] PUT {} body={}", url, body);
    let agent = build_agent();
    match agent.put(&url).send_json(&body) {
        Ok(response) => {
            let status = response.status();
            let body_text = response.into_string().unwrap_or_default();
            log::info!("[Hue] Activate response (status {}): {}", status, body_text);
            if status == 200 {
                // Check for errors in response array
                let parsed: serde_json::Value = serde_json::from_str(&body_text)
                    .unwrap_or(serde_json::Value::Null);
                if let Some(arr) = parsed.as_array() {
                    for item in arr {
                        if item.get("error").is_some() {
                            let desc = item.get("error")
                                .and_then(|e| e.get("description"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown error");
                            return Err(format!("Bridge error: {}", desc));
                        }
                    }
                }
                log::info!("[Hue] Entertainment mode activated on group {}", group_id);
                Ok(())
            } else {
                Err(format!("Unexpected status {}: {}", status, body_text))
            }
        }
        Err(e) => {
            // Extract response body from ureq HTTP errors
            let detail = match e {
                ureq::Error::Status(code, response) => {
                    let body = response.into_string().unwrap_or_default();
                    format!("HTTP {} — {}", code, body)
                }
                other => format!("{}", other),
            };
            Err(format!("Failed to activate entertainment: {}", detail))
        }
    }
}

/// Deactivate entertainment streaming mode (v1 API)
pub fn deactivate_entertainment(bridge_ip: &str, username: &str, group_id: &str) -> Result<(), String> {
    let url = format!("http://{}/api/{}/groups/{}", bridge_ip, username, group_id);
    let body = serde_json::json!({
        "stream": {
            "active": false
        }
    });

    log::info!("[Hue] PUT (deactivate) {} body={}", url, body);
    let agent = build_agent();
    match agent.put(&url).send_json(&body) {
        Ok(response) => {
            let body_text = response.into_string().unwrap_or_default();
            log::info!("[Hue] Deactivate response: {}", body_text);
            Ok(())
        }
        Err(e) => {
            let detail = match e {
                ureq::Error::Status(code, response) => {
                    let body = response.into_string().unwrap_or_default();
                    format!("HTTP {} — {}", code, body)
                }
                other => format!("{}", other),
            };
            log::warn!("[Hue] Failed to deactivate entertainment: {}", detail);
            Err(format!("Failed to deactivate: {}", detail))
        }
    }
}

// ============================================================================
// v2 CLIP API (HTTPS with self-signed cert)
// ============================================================================

/// Fetch the hue-application-id from the bridge (v2 auth endpoint).
///
/// The v2 Entertainment API uses this as the DTLS PSK identity instead of the username.
/// Endpoint: GET https://{bridge_ip}/auth/v1 with hue-application-key header.
/// The application ID is returned as a **response header**, not in the body.
pub fn fetch_application_id(bridge_ip: &str, username: &str) -> Result<String, String> {
    let url = format!("https://{}/auth/v1", bridge_ip);
    let (host, path) = parse_https_url(&url)?;
    let connector = build_tls_connector()?;

    let tcp = std::net::TcpStream::connect(format!("{}:443", host))
        .map_err(|e| format!("TCP connect to {}: {}", host, e))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let mut tls = connector.connect(&host, tcp)
        .map_err(|e| format!("TLS handshake with {}: {}", host, e))?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nhue-application-key: {}\r\nConnection: close\r\n\r\n",
        path, host, username
    );
    tls.write_all(request.as_bytes())
        .map_err(|e| format!("HTTPS write: {}", e))?;

    let mut response_bytes = Vec::new();
    tls.read_to_end(&mut response_bytes)
        .map_err(|e| format!("HTTPS read: {}", e))?;

    let raw_str = String::from_utf8_lossy(&response_bytes);

    // Parse the hue-application-id from response headers
    // Headers are between the status line and the \r\n\r\n separator
    let header_end = raw_str.find("\r\n\r\n").unwrap_or(raw_str.len());
    let headers_section = &raw_str[..header_end];

    for line in headers_section.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("hue-application-id:") {
            let value = line.splitn(2, ':').nth(1)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if !value.is_empty() {
                log::info!("[Hue/v2] Got application_id: {}", value);
                return Ok(value);
            }
        }
    }

    Err("Missing hue-application-id header in /auth/v1 response".to_string())
}

/// 3D position of a light channel in the entertainment area
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ChannelPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Light channel info from v2 entertainment configuration
pub struct HueChannelInfo {
    /// v2 channel ID (u8 index into channels array)
    pub channel_id: u8,
    /// Human-readable light name (from service metadata)
    pub name: String,
    /// Archetype string from bridge
    pub archetype: String,
    /// 3D position in the entertainment area
    pub position: ChannelPosition,
}

/// Fetch available entertainment configurations from the bridge (v2 CLIP API)
/// Returns Vec of (config_id/UUID, config_name)
pub fn fetch_entertainment_areas_v2(bridge_ip: &str, username: &str) -> Result<Vec<(String, String)>, String> {
    let url = format!("https://{}/clip/v2/resource/entertainment_configuration", bridge_ip);
    let response_text = https_get(&url, username)?;
    let parsed: serde_json::Value = serde_json::from_str(&response_text)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let mut areas = Vec::new();
    if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
        for config in data {
            let id = config.get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = config.get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unnamed Area")
                .to_string();
            if !id.is_empty() {
                areas.push((id, name));
            }
        }
    }

    log::info!("[Hue/v2] Found {} entertainment configurations", areas.len());
    Ok(areas)
}

/// Fetch channels (lights with positions) for a v2 entertainment configuration
/// Returns Vec of HueChannelInfo with channel_id, name, archetype, position
pub fn fetch_area_lights_v2(bridge_ip: &str, username: &str, config_id: &str) -> Result<Vec<HueChannelInfo>, String> {
    let url = format!("https://{}/clip/v2/resource/entertainment_configuration/{}", bridge_ip, config_id);
    let response_text = https_get(&url, username)?;
    let parsed: serde_json::Value = serde_json::from_str(&response_text)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let mut result = Vec::new();

    // v2 response: { "data": [ { "channels": [...], ... } ] }
    let config = parsed.get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .ok_or("No entertainment configuration data in response")?;

    let channels = config.get("channels")
        .and_then(|c| c.as_array())
        .ok_or("No channels in entertainment configuration")?;

    for channel in channels {
        let channel_id = channel.get("channel_id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u8;

        // Position: { x: f32, y: f32, z: f32 }
        let position = channel.get("position")
            .map(|p| ChannelPosition {
                x: p.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                y: p.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                z: p.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            })
            .unwrap_or_default();

        // Channel contains a "members" array with light references
        let name = channel.get("members")
            .and_then(|m| m.as_array())
            .and_then(|arr| arr.first())
            .and_then(|member| member.get("service"))
            .and_then(|s| s.get("rtype"))
            .and_then(|v| v.as_str())
            .unwrap_or("Light")
            .to_string();

        // Archetype from member's service metadata (best effort)
        let archetype = "unknown".to_string();

        result.push(HueChannelInfo {
            channel_id,
            name: format!("Channel {}", channel_id),
            archetype,
            position,
        });
    }

    // Try to resolve light names from the light resources
    // Fetch all lights to get human-readable names
    let lights_url = format!("https://{}/clip/v2/resource/light", bridge_ip);
    if let Ok(lights_text) = https_get(&lights_url, username) {
        if let Ok(lights_parsed) = serde_json::from_str::<serde_json::Value>(&lights_text) {
            if let Some(light_data) = lights_parsed.get("data").and_then(|d| d.as_array()) {
                // Build a map of light rid -> (name, archetype)
                let light_map: std::collections::HashMap<String, (String, String)> = light_data.iter()
                    .filter_map(|light| {
                        let rid = light.get("id")?.as_str()?.to_string();
                        let name = light.get("metadata")
                            .and_then(|m| m.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown")
                            .to_string();
                        let archetype = light.get("metadata")
                            .and_then(|m| m.get("archetype"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        Some((rid, (name, archetype)))
                    })
                    .collect();

                // Match channel members to light names
                if let Some(channels_arr) = config.get("channels").and_then(|c| c.as_array()) {
                    for (i, channel) in channels_arr.iter().enumerate() {
                        if i >= result.len() { break; }
                        if let Some(members) = channel.get("members").and_then(|m| m.as_array()) {
                            for member in members {
                                if let Some(rid) = member.get("service")
                                    .and_then(|s| s.get("rid"))
                                    .and_then(|v| v.as_str())
                                {
                                    // Try to find this rid in the entertainment resource's light services
                                    // The rid in channels.members.service is an entertainment service rid,
                                    // not a light rid. We need to look up via the owner.
                                    if let Some((name, arch)) = light_map.get(rid) {
                                        result[i].name = name.clone();
                                        result[i].archetype = arch.clone();
                                    }
                                }
                            }
                        }
                    }
                }

                // If names are still "Channel N", try matching via entertainment services
                let ent_url = format!("https://{}/clip/v2/resource/entertainment", bridge_ip);
                if let Ok(ent_text) = https_get(&ent_url, username) {
                    if let Ok(ent_parsed) = serde_json::from_str::<serde_json::Value>(&ent_text) {
                        if let Some(ent_data) = ent_parsed.get("data").and_then(|d| d.as_array()) {
                            // Build map: entertainment service rid -> owner light rid
                            let ent_to_light: std::collections::HashMap<String, String> = ent_data.iter()
                                .filter_map(|ent| {
                                    let ent_rid = ent.get("id")?.as_str()?.to_string();
                                    let light_rid = ent.get("renderer_reference")
                                        .or_else(|| ent.get("proxy"))
                                        .and_then(|r| r.get("rid"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string())?;
                                    Some((ent_rid, light_rid))
                                })
                                .collect();

                            // Re-match channels
                            if let Some(channels_arr) = config.get("channels").and_then(|c| c.as_array()) {
                                for (i, channel) in channels_arr.iter().enumerate() {
                                    if i >= result.len() { break; }
                                    if result[i].name.starts_with("Channel ") {
                                        if let Some(members) = channel.get("members").and_then(|m| m.as_array()) {
                                            for member in members {
                                                if let Some(ent_rid) = member.get("service")
                                                    .and_then(|s| s.get("rid"))
                                                    .and_then(|v| v.as_str())
                                                {
                                                    if let Some(light_rid) = ent_to_light.get(ent_rid) {
                                                        if let Some((name, arch)) = light_map.get(light_rid) {
                                                            result[i].name = name.clone();
                                                            result[i].archetype = arch.clone();
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    log::info!("[Hue/v2] Found {} channels in entertainment config", result.len());
    for ch in &result {
        log::info!("[Hue/v2]   channel {} '{}' archetype='{}' pos=({:.2},{:.2},{:.2})",
            ch.channel_id, ch.name, ch.archetype, ch.position.x, ch.position.y, ch.position.z);
    }

    Ok(result)
}

/// Activate entertainment streaming mode (v2 CLIP API)
pub fn activate_entertainment_v2(bridge_ip: &str, username: &str, config_id: &str) -> Result<(), String> {
    let url = format!("https://{}/clip/v2/resource/entertainment_configuration/{}", bridge_ip, config_id);
    let body = serde_json::json!({
        "action": "start"
    });

    log::info!("[Hue/v2] PUT {} body={}", url, body);
    let response_text = https_put(&url, username, &body)?;
    log::info!("[Hue/v2] Activate response: {}", response_text);

    // Check for errors in response
    let parsed: serde_json::Value = serde_json::from_str(&response_text).unwrap_or(serde_json::Value::Null);
    if let Some(errors) = parsed.get("errors").and_then(|e| e.as_array()) {
        if !errors.is_empty() {
            let desc = errors.first()
                .and_then(|e| e.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            return Err(format!("v2 API error: {}", desc));
        }
    }

    log::info!("[Hue/v2] Entertainment mode activated on config {}", config_id);
    Ok(())
}

/// Deactivate entertainment streaming mode (v2 CLIP API)
pub fn deactivate_entertainment_v2(bridge_ip: &str, username: &str, config_id: &str) -> Result<(), String> {
    let url = format!("https://{}/clip/v2/resource/entertainment_configuration/{}", bridge_ip, config_id);
    let body = serde_json::json!({
        "action": "stop"
    });

    log::info!("[Hue/v2] PUT (deactivate) {} body={}", url, body);
    match https_put(&url, username, &body) {
        Ok(response_text) => {
            log::info!("[Hue/v2] Deactivate response: {}", response_text);
            Ok(())
        }
        Err(e) => {
            log::warn!("[Hue/v2] Failed to deactivate entertainment: {}", e);
            Err(format!("Failed to deactivate: {}", e))
        }
    }
}

// ============================================================================
// HTTPS helpers (v2 CLIP API — self-signed bridge cert via native-tls)
// ============================================================================

use std::io::{Read, Write};

/// Parse host and path from an https:// URL
fn parse_https_url(url: &str) -> Result<(String, String), String> {
    let stripped = url.strip_prefix("https://")
        .ok_or_else(|| format!("Not an HTTPS URL: {}", url))?;
    let (host, path) = match stripped.find('/') {
        Some(idx) => (stripped[..idx].to_string(), stripped[idx..].to_string()),
        None => (stripped.to_string(), "/".to_string()),
    };
    Ok((host, path))
}

/// Build a native-tls connector that accepts self-signed certs (Hue bridges use self-signed)
fn build_tls_connector() -> Result<native_tls::TlsConnector, String> {
    native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| format!("TLS connector: {}", e))
}

/// Parse raw HTTP response bytes: extract status code, check for chunked encoding, return body.
fn parse_http_response(raw: &[u8]) -> (u16, String) {
    // Convert to lossy string for header parsing
    let raw_str = String::from_utf8_lossy(raw);

    // Find the status line
    let status = if let Some(first_line) = raw_str.lines().next() {
        first_line.split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0)
    } else {
        return (0, String::new());
    };

    // Split headers from body at \r\n\r\n
    let separator = b"\r\n\r\n";
    let header_end = raw.windows(4)
        .position(|w| w == separator)
        .unwrap_or(raw.len());
    let headers_str = String::from_utf8_lossy(&raw[..header_end]).to_lowercase();
    let body_start = (header_end + 4).min(raw.len());
    let body_bytes = &raw[body_start..];

    // Check for chunked transfer encoding
    let is_chunked = headers_str.contains("transfer-encoding: chunked");

    let body = if is_chunked {
        decode_chunked(body_bytes)
    } else {
        String::from_utf8_lossy(body_bytes).to_string()
    };

    log::debug!("[Hue/HTTPS] Response status={} chunked={} body_len={}", status, is_chunked, body.len());
    (status, body)
}

/// Decode HTTP chunked transfer encoding from raw bytes
fn decode_chunked(data: &[u8]) -> String {
    let mut result = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // Find end of chunk size line (\r\n)
        let line_end = data[pos..].windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| pos + p);

        let line_end = match line_end {
            Some(e) => e,
            None => break,
        };

        // Parse chunk size (hex)
        let size_str = String::from_utf8_lossy(&data[pos..line_end]);
        let size_str = size_str.trim();
        // Chunk size may have extensions after a semicolon
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let chunk_size = match usize::from_str_radix(size_hex, 16) {
            Ok(s) => s,
            Err(_) => break,
        };

        if chunk_size == 0 {
            break; // Final chunk
        }

        let chunk_start = line_end + 2; // skip \r\n after size
        let chunk_end = (chunk_start + chunk_size).min(data.len());
        result.extend_from_slice(&data[chunk_start..chunk_end]);

        // Skip chunk data + trailing \r\n
        pos = chunk_end + 2;
    }

    String::from_utf8_lossy(&result).to_string()
}

fn https_get(url: &str, app_key: &str) -> Result<String, String> {
    let (host, path) = parse_https_url(url)?;
    let connector = build_tls_connector()?;

    let tcp = std::net::TcpStream::connect(format!("{}:443", host))
        .map_err(|e| format!("TCP connect to {}: {}", host, e))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let mut tls = connector.connect(&host, tcp)
        .map_err(|e| format!("TLS handshake with {}: {}", host, e))?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nhue-application-key: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        path, host, app_key
    );
    tls.write_all(request.as_bytes())
        .map_err(|e| format!("HTTPS write: {}", e))?;

    let mut response_bytes = Vec::new();
    tls.read_to_end(&mut response_bytes)
        .map_err(|e| format!("HTTPS read: {}", e))?;

    let (status, body) = parse_http_response(&response_bytes);
    if status >= 200 && status < 300 {
        Ok(body)
    } else {
        Err(format!("HTTPS GET {} → {} — {}", url, status, body))
    }
}

fn https_put(url: &str, app_key: &str, body: &serde_json::Value) -> Result<String, String> {
    let (host, path) = parse_https_url(url)?;
    let connector = build_tls_connector()?;

    let tcp = std::net::TcpStream::connect(format!("{}:443", host))
        .map_err(|e| format!("TCP connect to {}: {}", host, e))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let mut tls = connector.connect(&host, tcp)
        .map_err(|e| format!("TLS handshake with {}: {}", host, e))?;

    let body_str = serde_json::to_string(body)
        .map_err(|e| format!("JSON serialize: {}", e))?;

    let request = format!(
        "PUT {} HTTP/1.1\r\nHost: {}\r\nhue-application-key: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n{}",
        path, host, app_key, body_str.len(), body_str
    );
    tls.write_all(request.as_bytes())
        .map_err(|e| format!("HTTPS write: {}", e))?;

    let mut response_bytes = Vec::new();
    tls.read_to_end(&mut response_bytes)
        .map_err(|e| format!("HTTPS read: {}", e))?;

    let (status, response_body) = parse_http_response(&response_bytes);
    if status >= 200 && status < 300 {
        Ok(response_body)
    } else {
        Err(format!("HTTPS PUT {} → {} — {}", url, status, response_body))
    }
}

// ============================================================================
// HTTP helpers (v1 API)
// ============================================================================

fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .build()
}

fn http_get(url: &str) -> Result<String, String> {
    let agent = build_agent();
    match agent.get(url).call() {
        Ok(response) => {
            response.into_string()
                .map_err(|e| format!("Failed to read response: {}", e))
        }
        Err(e) => Err(format!("HTTP request failed: {}", e)),
    }
}
