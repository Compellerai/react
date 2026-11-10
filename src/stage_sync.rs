//! HTTP client for the SaaS `/api/v1/stages` surface.
//!
//! Pure transport — no queueing, no retry, no debounce. The sync worker
//! ([`crate::stage_sync_worker`]) calls these for the operator-driven side
//! effects (select a stage, drag a fixture, +Fixture, remove).
//!
//! Environment split: [`api_base`] returns the `REACT_DEV_API_HOST` build-time
//! host under `--features dev` (start-react.sh) and `compeller.ai` under
//! release builds (build.sh).

use std::collections::HashMap;
use std::env;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::emerge::{
    ChannelMap, DmxPatch, FixturePatch, FixtureRole, StageVec3, StageVisualKind, Truss, VenueKind,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct StageSyncConfig {
    pub base_url: String,
    pub token: String,
}

/// SaaS API base — `REACT_API_BASE` override, else the build's configured host
/// (`REACT_DEV_API_HOST` under `--features dev`, compeller.ai otherwise).
pub fn api_base() -> String {
    env::var("REACT_API_BASE")
        .or_else(|_| env::var("COMPELLER_API_BASE"))
        .unwrap_or_else(|_| crate::config::API_HOST.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// One row in the Stage: dropdown.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StageSummary {
    pub id: i64,
    pub name: String,
    pub slug: Option<String>,
    pub active: bool,
    /// The venue this stage authored (`venue_kind` from SaaS). `Some(WeddingRoom)`
    /// lets the picker route the pick to the Wedding room and label it; `None` /
    /// Concert behaves exactly as before. `#[serde(default)]` keeps any older
    /// cached list deserializable.
    #[serde(default)]
    pub venue: Option<VenueKind>,
    /// True for the shared, admin-maintained Concert/Wedding templates (SaaS sets a
    /// `slug`). The dropdown pins these in a "Shared (Admin)" section above the
    /// account's own "My" stages. Older SaaS builds omit the field → `false`
    /// (treated as a personal stage, the pre-split behavior).
    #[serde(default)]
    pub shared: bool,
    /// Whether the current login may EDIT this stage. Shared templates are editable
    /// only by a Compeller admin; personal stages are always editable. Older SaaS
    /// builds omit the field → `true`, preserving the pre-split edit-everything
    /// behavior. Gates the auto-flush + the in-stage edit affordances.
    #[serde(default = "default_true")]
    pub editable: bool,
}

/// serde default for `StageSummary::editable` — a stage with no `editable` field
/// (older SaaS) is treated as editable so existing rigs keep flushing edits.
fn default_true() -> bool {
    true
}

/// List the operator's stages. Empty list is a legitimate result (account has
/// no stages yet — the dropdown shows just the "Select stage" placeholder).
pub fn list_stages(config: &StageSyncConfig) -> Result<Vec<StageSummary>, String> {
    let url = format!("{}/api/v1/stages?limit=100", config.base_url);
    let response: StageListResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .timeout(REQUEST_TIMEOUT)
        .call()
        .map_err(|err| format!("stage list failed: {}", err))?
        .into_json()
        .map_err(|err| format!("stage list parse failed: {}", err))?;
    Ok(response
        .stages
        .into_iter()
        .map(|s| StageSummary {
            id: s.id,
            name: s.name.unwrap_or_else(|| format!("Stage {}", s.id)),
            slug: s.slug,
            active: s.active.unwrap_or(true),
            venue: s.venue_kind.as_deref().and_then(VenueKind::from_key),
            shared: s.shared,
            editable: s.editable,
        })
        .collect())
}

/// Clone a shared stage into the account's own editable "My <venue>" copy and
/// return the new (or pre-existing) stage's SaaS id, so the caller can select it.
/// POST `/api/v1/stages/{id}/clone`. The server dedups one personal stage per
/// venue, so calling this twice just re-selects the existing copy.
pub fn clone_stage(config: &StageSyncConfig, source_id: i64) -> Result<i64, String> {
    let url = format!("{}/api/v1/stages/{}/clone", config.base_url, source_id);
    let response: StageResponse = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .timeout(REQUEST_TIMEOUT)
        .call()
        .map_err(|err| format!("stage clone failed: {}", err))?
        .into_json()
        .map_err(|err| format!("stage clone parse failed: {}", err))?;
    Ok(response.stage.id)
}

/// Pull one stage and return its renderer patch plus a `fixture_key → SaaS id`
/// map. The worker stores that map so subsequent PATCH/DELETE can address
/// rows by SaaS id even though the renderer keys off the local fixture_key.
pub fn fetch_stage_patch(
    config: &StageSyncConfig,
    stage_id: i64,
) -> Result<(DmxPatch, HashMap<String, i64>), String> {
    let stage = fetch_stage(config, stage_id)?;
    let mut id_by_key: HashMap<String, i64> = HashMap::new();
    let fixtures: Vec<FixturePatch> = stage
        .fixtures
        .into_iter()
        .map(|dto| {
            let patch = FixturePatch::from(dto.clone());
            id_by_key.insert(patch.id.clone(), dto.id);
            patch
        })
        .collect();
    // Seed the corrected default rig when the stage has no saved layout (new
    // stage, or an older SaaS that doesn't return the field) so the venue
    // always reads as a rig; a saved layout takes over verbatim.
    let trusses = match stage.truss_layout {
        Some(t) if !t.is_empty() => t,
        _ => DmxPatch::default_trusses(),
    };
    // `None` when the stage didn't author a venue (absent/unknown key) — that
    // hands control to the operator's local Venue toggle; `Some` makes the stage
    // authoritative for the rendered venue.
    let venue = stage.venue_kind.as_deref().and_then(VenueKind::from_key);
    Ok((
        DmxPatch {
            universe: stage.universe.unwrap_or(0),
            fixtures,
            trusses,
            venue,
        },
        id_by_key,
    ))
}

/// Create one fixture on the SaaS stage. Returns the assigned SaaS id so the
/// worker can map subsequent updates / deletes back to this row.
pub fn create_fixture(
    config: &StageSyncConfig,
    stage_id: i64,
    fixture: &FixturePatch,
) -> Result<i64, String> {
    let url = format!("{}/api/v1/stages/{}/fixtures", config.base_url, stage_id);
    let body = fixture_payload(fixture);
    let response: FixtureCreateResponse = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_json(serde_json::to_value(body).map_err(|err| err.to_string())?)
        .map_err(|err| format!("fixture create failed: {}", err))?
        .into_json()
        .map_err(|err| format!("fixture create parse failed: {}", err))?;
    Ok(response.fixture.id)
}

/// Patch an existing fixture by its SaaS id. `Ok(true)` = updated; `Ok(false)` =
/// the row is gone on SaaS (404) — the caller re-creates it rather than retrying
/// a doomed PATCH forever.
pub fn update_fixture(
    config: &StageSyncConfig,
    stage_id: i64,
    fixture_id: i64,
    fixture: &FixturePatch,
) -> Result<bool, String> {
    let url = format!(
        "{}/api/v1/stages/{}/fixtures/{}",
        config.base_url, stage_id, fixture_id
    );
    let body = fixture_payload(fixture);
    let payload = serde_json::to_value(body).map_err(|err| err.to_string())?;
    match ureq::patch(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_json(payload)
    {
        Ok(_) => Ok(true),
        Err(ureq::Error::Status(404, _)) => Ok(false),
        Err(err) => Err(format!("fixture update failed: {}", err)),
    }
}

/// Delete a fixture by SaaS id.
pub fn delete_fixture(
    config: &StageSyncConfig,
    stage_id: i64,
    fixture_id: i64,
) -> Result<(), String> {
    let url = format!(
        "{}/api/v1/stages/{}/fixtures/{}",
        config.base_url, stage_id, fixture_id
    );
    match ureq::delete(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .timeout(REQUEST_TIMEOUT)
        .call()
    {
        Ok(_) => Ok(()),
        // A 404 means the row is already gone — for a delete that is success
        // (idempotent). It also stops a stale cached fixture id (the row was
        // deleted on SaaS while we were offline) from becoming a poison-pill op
        // that fails forever and stalls the whole sync queue.
        Err(ureq::Error::Status(404, _)) => Ok(()),
        Err(err) => Err(format!("fixture delete failed: {}", err)),
    }
}

/// Replace the stage's entire truss layout. Trusses are edited as a small set
/// (a handful per stage) rather than individually addressed like fixtures, so
/// they round-trip as one PATCH on the stage instead of per-row CRUD.
pub fn update_truss_layout(
    config: &StageSyncConfig,
    stage_id: i64,
    trusses: &[Truss],
) -> Result<(), String> {
    let url = format!("{}/api/v1/stages/{}", config.base_url, stage_id);
    let body = TrussLayoutUpdate { truss_layout: trusses };
    ureq::patch(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_json(serde_json::to_value(body).map_err(|err| err.to_string())?)
        .map_err(|err| format!("truss layout update failed: {}", err))?;
    Ok(())
}

fn fetch_stage(config: &StageSyncConfig, stage_id: i64) -> Result<StageDto, String> {
    let url = format!("{}/api/v1/stages/{}", config.base_url, stage_id);
    let response: StageResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .timeout(REQUEST_TIMEOUT)
        .call()
        .map_err(|err| format!("stage fetch failed: {}", err))?
        .into_json()
        .map_err(|err| format!("stage response parse failed: {}", err))?;
    Ok(response.stage)
}

fn fixture_payload(fixture: &FixturePatch) -> FixtureUpdate<'_> {
    // Screens round-trip as kind='Screen' + behaviorJson; lights emit neither, so
    // SaaS leaves those columns alone on a normal fixture PATCH.
    let (kind, behavior) = match &fixture.screen {
        Some(cfg) => (Some("Screen"), Some(cfg.to_behavior())),
        None => (None, None),
    };
    FixtureUpdate {
        fixture_key: &fixture.id,
        name: &fixture.name,
        profile_id: &fixture.profile_id,
        mode: &fixture.mode,
        role: fixture.role,
        visual_kind: fixture.visual_kind,
        universe: fixture.universe,
        start_address: fixture.start_address,
        position: fixture.position,
        rotation: fixture.rotation,
        scale: fixture.scale,
        tags: &fixture.tags,
        channels: fixture.channels,
        kind,
        behavior,
    }
}

#[derive(Debug, Deserialize)]
struct StageListResponse {
    stages: Vec<StageSummaryDto>,
}

#[derive(Debug, Deserialize)]
struct StageSummaryDto {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    active: Option<bool>,
    // Which venue this stage renders as. Absent on SaaS builds that predate the
    // wedding-room work; unknown/absent -> None (the picker treats it as Concert).
    #[serde(default)]
    venue_kind: Option<String>,
    // Shared admin template vs the account's own stage; whether this login may edit
    // it. Absent on older SaaS -> shared=false, editable=true (pre-split behavior).
    #[serde(default)]
    shared: bool,
    #[serde(default = "default_true")]
    editable: bool,
}

#[derive(Debug, Deserialize)]
struct StageResponse {
    stage: StageDto,
}

#[derive(Debug, Deserialize)]
struct StageDto {
    #[allow(dead_code)]
    id: i64,
    universe: Option<u16>,
    fixtures: Vec<StageFixtureDto>,
    // Absent on older SaaS builds (pre truss_layout); `[]` on a stage the
    // operator has never configured. Both fall through to seeded defaults.
    #[serde(default)]
    truss_layout: Option<Vec<Truss>>,
    // Which venue/environment this stage renders as. Absent on SaaS builds that
    // predate the wedding-room work; unknown/absent -> Concert Stage.
    #[serde(default)]
    venue_kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct StageFixtureDto {
    id: i64,
    fixture_key: Option<String>,
    name: String,
    profile_id: Option<String>,
    mode: Option<String>,
    role: Option<FixtureRole>,
    visual_kind: Option<StageVisualKind>,
    universe: Option<u16>,
    start_address: u16,
    position: Option<StageVec3>,
    rotation: Option<StageVec3>,
    scale: Option<StageVec3>,
    tags: Option<Vec<String>>,
    channels: Option<ChannelMap>,
    // Free-string fixture type. `"Screen"` marks a placeable video surface; any
    // other value (or absent) is a normal light. Absent on pre-wedding SaaS.
    #[serde(default)]
    kind: Option<String>,
    // Opaque per-fixture behavior JSON. For a Screen it carries the wire keys
    // `screen_fit` / `screen_media_id` / `screen_video_url`. Absent otherwise.
    #[serde(default)]
    behavior: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct FixtureCreateResponse {
    fixture: FixtureCreatedDto,
}

#[derive(Debug, Deserialize)]
struct FixtureCreatedDto {
    id: i64,
}

#[derive(Debug, Serialize)]
struct TrussLayoutUpdate<'a> {
    truss_layout: &'a [Truss],
}

#[derive(Debug, Serialize)]
struct FixtureUpdate<'a> {
    fixture_key: &'a str,
    name: &'a str,
    profile_id: &'a str,
    mode: &'a str,
    role: FixtureRole,
    visual_kind: StageVisualKind,
    universe: u16,
    start_address: u16,
    position: StageVec3,
    rotation: StageVec3,
    scale: StageVec3,
    tags: &'a [String],
    channels: ChannelMap,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    behavior: Option<serde_json::Value>,
}

impl From<StageFixtureDto> for FixturePatch {
    fn from(dto: StageFixtureDto) -> Self {
        let start_address = dto.start_address;
        // A fixture is a screen iff SaaS tagged it `kind='Screen'`; its config
        // comes from behaviorJson (empty behavior still yields a valid screen).
        let screen = if dto.kind.as_deref() == Some("Screen") {
            Some(crate::emerge::ScreenConfig::from_behavior(dto.behavior.as_ref()))
        } else {
            None
        };
        Self {
            id: dto
                .fixture_key
                .unwrap_or_else(|| format!("fixture-{}", dto.id)),
            name: dto.name,
            universe: dto.universe.unwrap_or(0),
            start_address,
            profile_id: dto
                .profile_id
                .unwrap_or_else(|| "saas-stage-fixture".to_string()),
            mode: dto.mode.unwrap_or_else(|| "default".to_string()),
            position: dto.position.unwrap_or_else(StageVec3::zero),
            rotation: dto.rotation.unwrap_or_else(StageVec3::zero),
            scale: dto.scale.unwrap_or_else(StageVec3::one),
            role: dto.role.unwrap_or(FixtureRole::Accent),
            visual_kind: dto.visual_kind.unwrap_or(StageVisualKind::Beam),
            tags: dto.tags.unwrap_or_default(),
            channels: dto
                .channels
                .unwrap_or_else(|| ChannelMap::rgbw_dimmer(start_address)),
            screen,
            // SaaS doesn't carry a footprint; 0 = unknown (allocator falls
            // back to the ChannelMap span). apply_patch self-heals it from
            // the resolved profile at the normalization chokepoint.
            footprint: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_dto_maps_to_patch_with_defaults() {
        let fixture = FixturePatch::from(StageFixtureDto {
            id: 10,
            fixture_key: Some("wash-1".to_string()),
            name: "Wash 1".to_string(),
            profile_id: None,
            mode: None,
            role: Some(FixtureRole::Wash),
            visual_kind: Some(StageVisualKind::Wash),
            universe: Some(2),
            start_address: 17,
            position: Some(StageVec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }),
            rotation: None,
            scale: None,
            tags: Some(vec!["front".to_string()]),
            channels: None,
            kind: None,
            behavior: None,
        });

        assert_eq!(fixture.id, "wash-1");
        assert!(fixture.screen.is_none());
        assert_eq!(fixture.universe, 2);
        assert_eq!(fixture.start_address, 17);
        assert_eq!(fixture.role, FixtureRole::Wash);
        assert_eq!(fixture.channels.dimmer, Some(17));
    }

    #[test]
    fn screen_fixture_maps_kind_and_behavior() {
        let dto = StageFixtureDto {
            id: 7,
            fixture_key: Some("screen-upstage".to_string()),
            name: "Upstage Screen".to_string(),
            profile_id: None,
            mode: None,
            role: Some(FixtureRole::Wash),
            visual_kind: Some(StageVisualKind::LedPanel),
            universe: Some(1),
            start_address: 1,
            position: None,
            rotation: None,
            scale: None,
            tags: None,
            channels: None,
            kind: Some("Screen".to_string()),
            behavior: Some(serde_json::json!({
                "screen_fit": "cover",
                "screen_media_id": 42,
                "screen_video_url": serde_json::Value::Null,
            })),
        };
        let patch = FixturePatch::from(dto);
        let screen = patch.screen.expect("kind='Screen' must yield a ScreenConfig");
        assert_eq!(screen.fit, crate::emerge::ScreenFit::Cover);
        assert_eq!(screen.media_id, Some(42));
        assert_eq!(screen.video_url, None);
    }

    #[test]
    fn screen_fixture_payload_carries_kind_and_behavior() {
        // A screen round-trips as kind='Screen' + behaviorJson so SaaS stores it
        // as a screen (and a REACT-authored screen survives the POST).
        let screen = FixturePatch::demo_screen("screen-1", 0, 1);
        let v = serde_json::to_value(fixture_payload(&screen)).unwrap();
        assert_eq!(v["kind"], "Screen");
        assert_eq!(v["behavior"]["screen_fit"], "contain");
        // A normal light omits both (skip_serializing_if) so its PATCH is clean.
        let wash = FixturePatch::demo_rgbw_wash("wash-1", 0, 1);
        let lv = serde_json::to_value(fixture_payload(&wash)).unwrap();
        assert!(lv.get("kind").is_none());
        assert!(lv.get("behavior").is_none());
    }
}
