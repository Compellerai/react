//! Fixture profile ingestion for REACT Virtual Stage.
//!
//! COM-406 starts with a small Open Fixture Library inspired JSON subset rather
//! than full GDTF. The goal is practical MVP fixture profiles that normalize
//! into renderer-free `FixtureProfile` contracts.

use crate::emerge::contracts::{
    ChannelDefinition, ChannelDetail, ChannelSlot, DmxAttribute, FixtureCapability, FixtureHousing,
    FixtureKind, FixtureProfile, ValueRange,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub struct FixtureProfileSource {
    pub profile: FixtureProfile,
    pub commerce: Option<FixtureCommerceMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureCommerceMetadata {
    pub price_tier: Option<String>,
    pub street_price_range: Option<String>,
    pub recommendation_notes: Option<String>,
}

#[derive(Debug)]
pub enum FixtureProfileError {
    Json(serde_json::Error),
    MissingModes,
    MissingChannels,
}

impl std::fmt::Display for FixtureProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(err) => write!(f, "invalid fixture profile JSON: {}", err),
            Self::MissingModes => f.write_str("fixture profile has no modes"),
            Self::MissingChannels => f.write_str("fixture mode has no channels and no fallback is available"),
        }
    }
}

impl std::error::Error for FixtureProfileError {}

impl From<serde_json::Error> for FixtureProfileError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub type Result<T> = std::result::Result<T, FixtureProfileError>;

pub fn load_ofl_fixture_profile(json: &str, mode_name: Option<&str>) -> Result<FixtureProfileSource> {
    let raw: OflFixture = serde_json::from_str(json)?;
    normalize_ofl_fixture(raw, mode_name)
}

/// Load a fixture profile in the ATLABASE `FIXTURE_PROFILE` JSON format - the
/// hardware library collected on the NAS. ATLABASE is just another input
/// adapter: it normalizes into the very same `FixtureProfile` contract as the
/// OFL loader, so its shape never reaches a renderer.
pub fn load_atla_fixture_profile(json: &str, mode_name: Option<&str>) -> Result<FixtureProfileSource> {
    let raw: AtlaProfileFile = serde_json::from_str(json)?;
    normalize_atla_fixture(raw.fixture_profile, mode_name)
}

/// In-memory index of the NAS hardware-profile directory - one ATLABASE
/// `FIXTURE_PROFILE` JSON per fixture, the shared library for both the SaaS
/// admin picker and REACT. Keyed by the ATLABASE `fixtureProfile.id`, which is
/// also the value stored in `StageFixture.profile_id`.
#[derive(Debug, Default)]
pub struct FixtureProfileLibrary {
    /// fixtureProfile.id -> raw JSON. Resolved (parsed per mode) on demand.
    by_id: std::collections::HashMap<String, String>,
    /// Parsed default-mode snapshot of every profile, sorted by manufacturer
    /// + model + mode — built once per scan via [`Self::rebuild_resolved`].
    /// [`Self::all_profiles`] is on the per-frame egui path while the stage
    /// editor is open, so it must serve this cache instead of re-parsing the
    /// raw JSON (the bundled library alone is ~121KB) every frame. The
    /// library is immutable after scan, so the cache never goes stale.
    resolved: Vec<FixtureProfile>,
}

impl FixtureProfileLibrary {
    /// Scan the configured hardware-profile directory. Honors
    /// `REACT_FIXTURE_PROFILE_DIR`, else the standard NAS mount paths. A
    /// missing directory yields an empty library, never an error.
    pub fn scan_default() -> Self {
        // The bundled set is the offline FLOOR (embedded at build time) so the
        // known rig always resolves with no NAS mount — without this, an offline /
        // venue laptop gets an empty library and every fixture falls back to a
        // default ChannelMap (wrong channels -> magenta / rogue strobe / garbage).
        // The live NAS (or REACT_FIXTURE_PROFILE_DIR) overlays on top when present
        // so profile edits land without a rebuild and NAS-only fixtures are added.
        let mut lib = Self::bundled();
        let nas = Self::scan_nas();
        let nas_count = nas.by_id.len();
        for (id, json) in nas.by_id {
            lib.by_id.insert(id, json);
        }
        // The overlay changed `by_id`, so the parsed cache must follow.
        lib.rebuild_resolved();
        log::info!(
            "[profiles] fixture library ready: {} profile(s) ({} from live NAS/env overlay, bundled fallback otherwise)",
            lib.by_id.len(),
            nas_count,
        );
        lib
    }

    /// Scan the live NAS mount or `REACT_FIXTURE_PROFILE_DIR` override. Empty when
    /// neither is present (offline / venue laptop) — the bundled floor covers it.
    fn scan_nas() -> Self {
        if let Ok(dir) = std::env::var("REACT_FIXTURE_PROFILE_DIR") {
            return Self::scan_dir(std::path::Path::new(&dir));
        }
        for candidate in [
            "/Volumes/Compeller/react/hardware/profile",
            "/mnt/compeller/react/hardware/profile",
        ] {
            let path = std::path::Path::new(candidate);
            if path.is_dir() {
                return Self::scan_dir(path);
            }
        }
        Self::default()
    }

    /// The fixtures embedded in the binary at build time (a snapshot of the NAS
    /// hardware-profile library) so the known rig drives correctly OFFLINE — no NAS
    /// mount required. Keyed by ATLABASE `fixtureProfile.id`, same as the NAS scan,
    /// so `scan_default`'s overlay replaces these by id when the NAS is mounted.
    ///
    /// INVARIANTS: (1) this set IS the offline guarantee — any NEW rig fixture must
    /// be appended here or it regresses to a default ChannelMap (garbage) offline;
    /// `bundled_library_resolves_the_rig_offline` cross-checks the count against
    /// `assets/nas-fixture-profiles/` so a missed bundle fails a test. (2) The NAS
    /// overlay can add or edit but never RETIRE a bundled id — a profile deleted on
    /// the NAS keeps resolving from this snapshot until a rebuild; true retirement
    /// needs a NAS-side tombstone, not file absence.
    fn bundled() -> Self {
        const BUNDLED: &[&str] = &[
            include_str!("../../assets/nas-fixture-profiles/chauvet-dj-obsession.json"),
            include_str!("../../assets/nas-fixture-profiles/chauvet-dj-scorpion-dual-rgb.json"),
            include_str!("../../assets/nas-fixture-profiles/shehds-200w-rgb-strobe-light-8-segment.json"),
            include_str!("../../assets/nas-fixture-profiles/shehds-3w-scan-laser.json"),
            include_str!("../../assets/nas-fixture-profiles/shehds-7r-beam-move-head-light.json"),
            include_str!("../../assets/nas-fixture-profiles/shehds-led-beam-8x12w-rgbw.json"),
            include_str!("../../assets/nas-fixture-profiles/shehds-led-flat-par-18x18w.json"),
        ];
        let mut by_id = std::collections::HashMap::new();
        for json in BUNDLED {
            match serde_json::from_str::<AtlaProfileFile>(json) {
                Ok(file) => match file.fixture_profile.id.filter(|s| !s.is_empty()) {
                    Some(id) => {
                        by_id.insert(id, json.to_string());
                    }
                    None => log::warn!("[profiles] bundled profile missing fixtureProfile.id"),
                },
                Err(err) => log::warn!("[profiles] bundled profile failed to parse: {}", err),
            }
        }
        let mut lib = Self { by_id, resolved: Vec::new() };
        lib.rebuild_resolved();
        lib
    }

    /// Index one directory of ATLABASE profile JSON files. Unreadable or
    /// non-ATLABASE files are logged and skipped, never fatal.
    pub fn scan_dir(dir: &std::path::Path) -> Self {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                log::warn!("[profiles] cannot read profile dir {}: {}", dir.display(), err);
                return Self::default();
            }
        };
        let mut by_id = std::collections::HashMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let json = match std::fs::read_to_string(&path) {
                Ok(json) => json,
                Err(err) => {
                    log::warn!("[profiles] cannot read {}: {}", path.display(), err);
                    continue;
                }
            };
            let file: AtlaProfileFile = match serde_json::from_str(&json) {
                Ok(file) => file,
                Err(err) => {
                    log::warn!("[profiles] skipped non-ATLABASE profile {}: {}", path.display(), err);
                    continue;
                }
            };
            match file.fixture_profile.id.filter(|s| !s.is_empty()) {
                Some(id) => {
                    by_id.insert(id, json);
                }
                None => log::warn!("[profiles] {} has no fixtureProfile.id; skipped", path.display()),
            }
        }
        log::info!("[profiles] indexed {} fixture profile(s) from {}", by_id.len(), dir.display());
        let mut lib = Self { by_id, resolved: Vec::new() };
        lib.rebuild_resolved();
        lib
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Resolve a SaaS `profile_id` (the ATLABASE `fixtureProfile.id`) and mode
    /// into a renderer-ready `FixtureProfile`. An empty mode picks the first.
    pub fn resolve(&self, profile_id: &str, mode: &str) -> Option<FixtureProfile> {
        let raw = self.by_id.get(profile_id)?;
        let mode = (!mode.is_empty()).then_some(mode);
        match load_atla_fixture_profile(raw, mode) {
            Ok(source) => Some(source.profile),
            Err(err) => {
                log::warn!("[profiles] profile {} failed to resolve: {}", profile_id, err);
                None
            }
        }
    }

    /// All resolved profiles, sorted by manufacturer + model + mode. Drives
    /// the +Fixture palette: the list of fixture types the operator can drop
    /// onto the current stage. Served from the scan-time cache — this runs
    /// every egui frame while the stage editor is open, so it must never
    /// re-parse the raw JSON library per call.
    pub fn all_profiles(&self) -> Vec<FixtureProfile> {
        self.resolved.clone()
    }

    /// (Re)build the parsed-profile cache from the raw JSON index — each
    /// profile's default (first) mode, sorted by manufacturer + model + mode.
    /// Called once at every construction/overlay point (`bundled`, `scan_dir`,
    /// `scan_default`); the library never mutates afterwards.
    fn rebuild_resolved(&mut self) {
        let mut out: Vec<FixtureProfile> = self
            .by_id
            .values()
            .filter_map(|raw| load_atla_fixture_profile(raw, None).ok().map(|s| s.profile))
            .collect();
        out.sort_by(|a, b| {
            a.manufacturer
                .cmp(&b.manufacturer)
                .then(a.model.cmp(&b.model))
                .then(a.mode.cmp(&b.mode))
        });
        self.resolved = out;
    }
}

fn normalize_ofl_fixture(raw: OflFixture, mode_name: Option<&str>) -> Result<FixtureProfileSource> {
    let mode = select_mode(&raw, mode_name)?;
    let kind = fixture_kind(raw.fixture_kind.as_deref().or(raw.category.as_deref()).unwrap_or("unknown"));
    let mut channels = normalize_channels(mode.channels.as_deref().unwrap_or_default());
    if channels.is_empty() {
        channels = fallback_channels(kind);
    }
    if channels.is_empty() {
        return Err(FixtureProfileError::MissingChannels);
    }

    let capabilities = capabilities_for_channels(&channels);
    let mode_label = mode.name.clone().or_else(|| mode.short_name.clone()).unwrap_or_else(|| "Default".to_string());
    let id = make_profile_id(&raw.manufacturer, &raw.model, &mode_label);
    let housing = housing_from_kind(kind);
    let profile = FixtureProfile {
        id,
        manufacturer: raw.manufacturer,
        model: raw.model,
        mode: mode_label,
        kind,
        housing,
        channel_count: channels.len() as u16,
        channels,
        capabilities,
    };

    Ok(FixtureProfileSource {
        profile,
        commerce: raw.commerce.map(|c| FixtureCommerceMetadata {
            price_tier: c.price_tier,
            street_price_range: c.street_price_range,
            recommendation_notes: c.recommendation_notes,
        }),
    })
}

fn select_mode<'a>(raw: &'a OflFixture, mode_name: Option<&str>) -> Result<&'a OflMode> {
    if raw.modes.is_empty() {
        return Err(FixtureProfileError::MissingModes);
    }
    if let Some(mode_name) = mode_name {
        if let Some(mode) = raw.modes.iter().find(|m| {
            m.name.as_deref() == Some(mode_name) || m.short_name.as_deref() == Some(mode_name)
        }) {
            return Ok(mode);
        }
        // Degraded fallback, never silent: a stale stored mode name driving
        // the FIRST mode's layout is exactly the rogue-strobe / garbage-
        // channel failure the bundled floor exists to prevent — a 9ch
        // personality on a 24ch-addressed fixture. Make it loud in the log.
        let ofl_mode_label =
            |m: &OflMode| m.name.clone().or_else(|| m.short_name.clone()).unwrap_or_else(|| "<unnamed>".to_string());
        log::warn!(
            "[profiles] OFL profile {} {} has no mode '{}' (available: [{}]); falling back to first mode '{}' — channel layout may not match the patched address span",
            raw.manufacturer,
            raw.model,
            mode_name,
            raw.modes.iter().map(ofl_mode_label).collect::<Vec<_>>().join(", "),
            ofl_mode_label(&raw.modes[0]),
        );
    }
    Ok(&raw.modes[0])
}

fn normalize_channels(raw_channels: &[OflChannel]) -> Vec<ChannelDefinition> {
    raw_channels.iter().enumerate().map(|(idx, raw)| {
        let attribute = raw.attribute
            .as_deref()
            .map(attribute_from_label)
            .unwrap_or_else(|| attribute_from_label(raw.name.as_deref().unwrap_or("custom")));
        ChannelDefinition {
            attribute,
            channel_offset: (idx + 1) as u16,
            default_value: raw.default_value.unwrap_or(0),
            blackout_value: raw.blackout_value.unwrap_or(0),
            // OFL channels carry no rich ATLABASE-style detail.
            detail: None,
            source_attribute: None,
        }
    }).collect()
}

fn fallback_channels(kind: FixtureKind) -> Vec<ChannelDefinition> {
    let attrs = match kind {
        FixtureKind::Wash | FixtureKind::Unknown => vec![DmxAttribute::Dimmer, DmxAttribute::Red, DmxAttribute::Green, DmxAttribute::Blue, DmxAttribute::White],
        FixtureKind::Spot | FixtureKind::Beam => vec![DmxAttribute::Pan, DmxAttribute::Tilt, DmxAttribute::Dimmer, DmxAttribute::Strobe, DmxAttribute::ColorWheel, DmxAttribute::GoboSlot],
        FixtureKind::Strobe | FixtureKind::Blinder => vec![DmxAttribute::Dimmer, DmxAttribute::Strobe],
        FixtureKind::Laser => vec![DmxAttribute::Dimmer, DmxAttribute::LaserMacro, DmxAttribute::Strobe],
        FixtureKind::PixelBar => vec![DmxAttribute::Dimmer, DmxAttribute::Red, DmxAttribute::Green, DmxAttribute::Blue],
        FixtureKind::VideoPanel => vec![DmxAttribute::Dimmer],
    };
    attrs.into_iter().enumerate().map(|(idx, attribute)| ChannelDefinition {
        attribute,
        channel_offset: (idx + 1) as u16,
        default_value: 0,
        blackout_value: 0,
        detail: None,
        source_attribute: None,
    }).collect()
}

fn fixture_kind(label: &str) -> FixtureKind {
    let value = label.to_ascii_lowercase();
    if value.contains("wash") || value.contains("par") { FixtureKind::Wash }
    else if value.contains("spot") { FixtureKind::Spot }
    else if value.contains("beam") { FixtureKind::Beam }
    // "strob" catches both "Strobe" and "Stroboscope(s)" — the ATLABASE
    // category is the latter and is missing an `e` after the `b`.
    else if value.contains("strob") { FixtureKind::Strobe }
    else if value.contains("blinder") { FixtureKind::Blinder }
    else if value.contains("laser") { FixtureKind::Laser }
    else if value.contains("pixel") || value.contains("bar") { FixtureKind::PixelBar }
    else if value.contains("video") || value.contains("panel") { FixtureKind::VideoPanel }
    else { FixtureKind::Unknown }
}

/// Word-boundary containment: does `value` contain `word` as a whole word
/// (words split on any non-alphanumeric run)? Plain substring matching let
/// "Panel Mode" register as Pan — this is the guard against that class of
/// false positive on short attribute stems.
fn has_word(value: &str, word: &str) -> bool {
    value
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| part == word)
}

fn attribute_from_label(label: &str) -> DmxAttribute {
    let value = label.to_ascii_lowercase().replace(['-', '_'], " ");
    // 16-bit fine channels ("Pan fine", "Tilt fine", "Dimmer fine") must NOT
    // claim a coarse slot: ChannelMap::from_profile is last-wins, so a fine
    // channel listed after its coarse partner would silently hijack the slot
    // — the head pins to one side at 1/256 of the intended travel. Classified
    // Custom so the coarse channel keeps the slot; the fine data stays in the
    // profile for a future 16-bit-aware executor.
    if value.contains("fine") {
        return DmxAttribute::Custom(label.to_string());
    }
    if value.contains("dimmer") || value == "intensity" { DmxAttribute::Dimmer }
    else if value == "red" || value.ends_with(" red") { DmxAttribute::Red }
    else if value == "green" || value.ends_with(" green") { DmxAttribute::Green }
    else if value == "blue" || value.ends_with(" blue") { DmxAttribute::Blue }
    else if value == "white" || value.ends_with(" white") { DmxAttribute::White }
    // UV is word-boundary matched like pan/tilt: bare substring "uv" would
    // mis-slot labels like "Fluvial". "UV fine" is already caught Custom by
    // the fine guard above.
    else if has_word(&value, "uv") { DmxAttribute::Uv }
    else if value.contains("color wheel") || value == "colour" || value == "color" { DmxAttribute::ColorWheel }
    // Pan/tilt are word-boundary matched: "pan" as a bare substring also
    // matched "Panel Mode" / "Expand" style labels and mis-slotted them.
    else if has_word(&value, "pan") { DmxAttribute::Pan }
    else if has_word(&value, "tilt") { DmxAttribute::Tilt }
    else if value.contains("zoom") { DmxAttribute::Zoom }
    else if value.contains("focus") { DmxAttribute::Focus }
    else if value.contains("strobe") || value.contains("shutter") { DmxAttribute::Strobe }
    else if value.contains("gobo rotation") || value.contains("gobo rotate") { DmxAttribute::GoboRotation }
    else if value.contains("gobo") { DmxAttribute::GoboSlot }
    else if value.contains("prism") { DmxAttribute::Prism }
    else if value.contains("laser") || value.contains("macro") { DmxAttribute::LaserMacro }
    else { DmxAttribute::Custom(label.to_string()) }
}

fn capabilities_for_channels(channels: &[ChannelDefinition]) -> Vec<FixtureCapability> {
    let mut caps = Vec::new();
    let has = |attr: DmxAttribute| channels.iter().any(|c| c.attribute == attr);
    if has(DmxAttribute::Dimmer) { caps.push(FixtureCapability::Dimmer); }
    if has(DmxAttribute::Red) || has(DmxAttribute::Green) || has(DmxAttribute::Blue) { caps.push(FixtureCapability::Rgb); }
    if has(DmxAttribute::White) { caps.push(FixtureCapability::White); }
    if has(DmxAttribute::ColorWheel) { caps.push(FixtureCapability::ColorWheel); }
    if has(DmxAttribute::Pan) || has(DmxAttribute::Tilt) { caps.push(FixtureCapability::PanTilt); }
    if has(DmxAttribute::Zoom) { caps.push(FixtureCapability::Zoom); }
    if has(DmxAttribute::Focus) { caps.push(FixtureCapability::Focus); }
    if has(DmxAttribute::Strobe) { caps.push(FixtureCapability::Strobe); }
    if has(DmxAttribute::GoboSlot) || has(DmxAttribute::GoboRotation) { caps.push(FixtureCapability::Gobo); }
    if has(DmxAttribute::Prism) { caps.push(FixtureCapability::Prism); }
    if has(DmxAttribute::LaserMacro) { caps.push(FixtureCapability::LaserMacro); }
    caps
}

fn make_profile_id(manufacturer: &str, model: &str, mode: &str) -> String {
    format!("{}.{}.{}", slug(manufacturer), slug(model), slug(mode))
}

fn slug(value: &str) -> String {
    value.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_atla_fixture(
    raw: AtlaFixtureProfile,
    mode_name: Option<&str>,
) -> Result<FixtureProfileSource> {
    if raw.mode.is_empty() {
        return Err(FixtureProfileError::MissingModes);
    }
    // ATLABASE lists attributes in DMX channel order, so the selected mode's
    // attribute slice maps 1:1 onto channel offsets.
    let mode = match mode_name {
        Some(wanted) => raw
            .mode
            .iter()
            .find(|m| m.name.as_deref() == Some(wanted))
            .unwrap_or_else(|| {
                // Degraded fallback, never silent: SaaS stored a mode name
                // the profile no longer carries (edited/renamed on the NAS),
                // so the fixture is driven with the FIRST mode's ChannelMap —
                // potentially a 9ch personality on a 24ch-addressed strobe.
                // Keep the fallback (a wrong-but-lit rig beats a dark one)
                // but surface it in the log for the operator.
                let atla_mode_label = |m: &AtlaMode| m.name.as_deref().unwrap_or("<unnamed>").to_string();
                log::warn!(
                    "[profiles] profile {} has no mode '{}' (available: [{}]); falling back to first mode '{}' — channel layout may not match the patched address span",
                    raw.id.as_deref().unwrap_or("<no id>"),
                    wanted,
                    raw.mode.iter().map(atla_mode_label).collect::<Vec<_>>().join(", "),
                    atla_mode_label(&raw.mode[0]),
                );
                &raw.mode[0]
            }),
        None => &raw.mode[0],
    };

    let channels = normalize_atla_channels(&mode.attribute);
    if channels.is_empty() {
        return Err(FixtureProfileError::MissingChannels);
    }

    let manufacturer = raw.manufacturer.clone().unwrap_or_else(|| "Unknown".to_string());
    let model = raw.name.clone().unwrap_or_else(|| "Unknown".to_string());
    let mode_label = mode.name.clone().unwrap_or_else(|| "Default".to_string());
    let kind = atla_fixture_kind(&raw);
    // Housing is profile-level + mode-stable: scan across all modes once.
    let housing = infer_housing(&raw);
    let capabilities = capabilities_for_channels(&channels);

    Ok(FixtureProfileSource {
        profile: FixtureProfile {
            // Carry the raw ATLABASE `fixtureProfile.id` through as the profile
            // id. That id is the key `by_id` / `resolve()` use AND the value
            // SaaS stores in `StageFixture.profile_id`, so the +Fixture palette
            // and `count_profile` match synced fixtures instead of always
            // reading (0). Fall back to the slug only when a profile JSON omits
            // an id (older OFL-shaped files round-trip via `make_profile_id`).
            id: raw
                .id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| make_profile_id(&manufacturer, &model, &mode_label)),
            manufacturer,
            model,
            mode: mode_label,
            kind,
            housing,
            channel_count: channels.len() as u16,
            channels,
            capabilities,
        },
        commerce: None,
    })
}

fn normalize_atla_channels(attributes: &[AtlaAttribute]) -> Vec<ChannelDefinition> {
    attributes
        .iter()
        .enumerate()
        .map(|(idx, raw)| ChannelDefinition {
            attribute: attribute_from_atla(
                raw.attr_type.as_deref(),
                raw.name.as_deref().unwrap_or(""),
            ),
            channel_offset: (idx + 1) as u16,
            // A channel pinned to a fixed value (STATIC, or a typed channel
            // with a staticValue) carries it as the default so the DMX writer
            // reproduces the fixture's resting state.
            default_value: raw
                .static_value
                .as_ref()
                .and_then(|s| s.value)
                .map(|v| v.round().clamp(0.0, 255.0) as u8)
                .unwrap_or(0),
            blackout_value: 0,
            // Preserve the rich ATLABASE capability detail (color wheel slots,
            // gobo steps, motion ranges, dimmer curves, static values) so it
            // is never discarded on ingest - COM-494.
            detail: atla_channel_detail(raw),
            source_attribute: serde_json::to_value(raw).ok(),
        })
        .collect()
}

/// Extract the rich, renderer-agnostic capability detail from one ATLABASE
/// attribute. Returns `None` for plain color/value channels that carry no
/// extra structure. The priority order mirrors ATLABASE's own shape: a
/// channel is at most one of color-wheel / gobo / movement / static / curve.
fn atla_channel_detail(raw: &AtlaAttribute) -> Option<ChannelDetail> {
    // Discrete wheels first - these own the channel.
    if let Some(cw) = &raw.color_wheel_setting {
        return Some(ChannelDetail::ColorWheel {
            allow_split_colors: cw.allow_split_colors.unwrap_or(false),
            slots: cw.colors.iter().map(atla_color_slot).collect(),
        });
    }
    if let Some(g) = &raw.gobo_setting {
        let steps: Vec<ChannelSlot> = g.steps.iter().map(atla_step_slot).collect();
        // GOBO-typed channels are gobo wheels; the same `steps` shape under a
        // non-gobo type (PRISM, effect macros, ...) is preserved as a macro so
        // opaque built-in ranges survive ingest.
        return Some(if is_gobo_type(raw.attr_type.as_deref()) {
            ChannelDetail::Gobo { steps }
        } else {
            ChannelDetail::Macro { steps }
        });
    }
    if let Some(p) = &raw.prism_setting {
        return Some(ChannelDetail::Prism {
            steps: p.steps.iter().map(atla_step_slot).collect(),
        });
    }

    let range = raw.range.as_ref().map(|r| ValueRange {
        low: r.low_value.unwrap_or(0.0),
        high: r.high_value.unwrap_or(0.0),
    });

    // Pan/tilt carry physical width/offset alongside the usable range.
    if let Some(m) = raw.pan_setting.as_ref().or(raw.tilt_setting.as_ref()) {
        return Some(ChannelDetail::Movement {
            range,
            smoothing: raw.smoothing,
            invert: raw.invert.unwrap_or(false),
            width: m.width,
            offset: m.offset,
        });
    }

    // A continuous curve (zoom/focus/dimmer) - keep range + smoothing + invert.
    if range.is_some() || raw.smoothing.is_some() {
        return Some(ChannelDetail::Range {
            range,
            smoothing: raw.smoothing,
            invert: raw.invert.unwrap_or(false),
        });
    }

    // A channel pinned to a fixed value with no other structure (control-mode
    // selector, static shutter) - preserve the resting value explicitly.
    if let Some(v) = raw.static_value.as_ref().and_then(|s| s.value) {
        return Some(ChannelDetail::Static {
            value: v.round().clamp(0.0, 255.0) as u8,
        });
    }

    None
}

fn is_gobo_type(attr_type: Option<&str>) -> bool {
    matches!(attr_type.unwrap_or("").to_ascii_uppercase().as_str(), "GOBO")
}

fn atla_color_slot(c: &AtlaColor) -> ChannelSlot {
    let rgb = c.color.first().map(|p| [p.r, p.g, p.b]);
    ChannelSlot {
        name: c.name.clone().unwrap_or_default(),
        value: c.value.unwrap_or(0.0).round().clamp(0.0, 255.0) as u8,
        low_value: c.low_value,
        high_value: c.high_value,
        enabled: c.enabled.unwrap_or(true),
        rgb,
    }
}

fn atla_step_slot(s: &AtlaStep) -> ChannelSlot {
    ChannelSlot {
        name: s.name.clone().unwrap_or_default(),
        value: s.value.unwrap_or(0.0).round().clamp(0.0, 255.0) as u8,
        low_value: s.low_value,
        high_value: s.high_value,
        enabled: s.enabled.unwrap_or(true),
        rgb: None,
    }
}

fn atla_fixture_kind(raw: &AtlaFixtureProfile) -> FixtureKind {
    // Multi-cell structural override walks **all** modes, not just the
    // first — the same profile in 9-channel mode looks single-cell, but
    // the fixture is still operationally a pixel bar / panel. Stroboscope
    // category is the exception: a multi-cell strobe is still semantically
    // a strobe (cue engine routes it to the strobe family), it just
    // *renders* as a panel via FixtureHousing.
    let max_cells = raw
        .mode
        .iter()
        .map(|m| count_color_segments(&m.attribute))
        .max()
        .unwrap_or(0);
    if max_cells >= 2 {
        // "strob" matches both `Strobe` and `Stroboscope(s)`.
        let category_says_strobe = raw
            .category
            .as_deref()
            .map(|c| c.to_ascii_lowercase().contains("strob"))
            .unwrap_or(false);
        if !category_says_strobe {
            return FixtureKind::PixelBar;
        }
    }
    // Category first ("Stroboscopes", "PARs", ...), then the model name
    // ("7R Beam ...", "... SCAN LASER") as a fallback.
    for label in [raw.category.as_deref(), raw.name.as_deref()].into_iter().flatten() {
        let kind = fixture_kind(label);
        if kind != FixtureKind::Unknown {
            return kind;
        }
    }
    FixtureKind::Unknown
}

/// Derive the physical housing for a profile by scanning **all** modes.
/// Housing is the renderer's source of truth for "what does this fixture
/// look like on stage", independent of which channel mode SaaS picks at
/// patch time. See `FixtureHousing` in `contracts.rs` for the variant
/// taxonomy + extension guidance.
fn infer_housing(raw: &AtlaFixtureProfile) -> FixtureHousing {
    let category = raw.category.as_deref().unwrap_or("").to_ascii_lowercase();
    let name = raw.name.as_deref().unwrap_or("").to_ascii_lowercase();

    // Walk every mode once for cross-mode capability signals — the operator
    // might pick any mode, so we classify on the fixture's full envelope.
    let mut any_pan = false;
    let mut any_tilt = false;
    let mut max_cells = 0usize;
    for m in &raw.mode {
        for a in &m.attribute {
            match a
                .attr_type
                .as_deref()
                .map(|s| s.to_ascii_uppercase())
                .as_deref()
            {
                Some("PAN") => any_pan = true,
                Some("TILT") => any_tilt = true,
                _ => {}
            }
        }
        let cells = count_color_segments(&m.attribute);
        if cells > max_cells {
            max_cells = cells;
        }
    }
    let pan_and_tilt = any_pan && any_tilt;
    let tilt_only = any_tilt && !any_pan;
    let multi_cell = max_cells >= 2;

    // Lasers are name-keyed in the NAS library today (no consistent ATLA
    // category for them yet — "3W SCAN LASER" carries the name signal).
    if name.contains("laser") {
        return FixtureHousing::ScanLaser;
    }

    // Stroboscope category routes to Panel when any mode is multi-cell
    // (200W RGB in 24-ch mode = 8 segments) and to classic Strobe otherwise.
    if category.contains("stroboscope") {
        return if multi_cell {
            FixtureHousing::Panel
        } else {
            FixtureHousing::Strobe
        };
    }

    // Full moving head: pan + tilt with a single aperture (no internal cells).
    if pan_and_tilt && !multi_cell {
        return FixtureHousing::MovingHead;
    }

    // Pixel bar: multi-cell across the body, optionally tilt-motorized
    // (SHEHDS LED Beam 8x12W). Also catches category "Arrays" / "Bars" and
    // tilt-only single-aperture fixtures (a bar that just sweeps).
    if multi_cell
        || category.contains("array")
        || category.contains("bar")
        || tilt_only
    {
        return FixtureHousing::PixelBar;
    }

    // PARs / washes / blinders all share the single-aperture par housing.
    if category.contains("par") || category.contains("wash") || category.contains("blinder") {
        return FixtureHousing::Par;
    }
    if name.contains("blinder") {
        return FixtureHousing::Par;
    }

    // Profile labelled "Moving Heads" or "Effects" but no pan/tilt parsed
    // in any mode — still render as a moving head so the housing matches
    // the rig (some ATLA profiles for effect projectors / gobo movers ship
    // with incomplete channel lists; the Chauvet DJ Obsession is one).
    if category.contains("moving") || category.contains("head") || category.contains("effect") {
        return FixtureHousing::MovingHead;
    }

    FixtureHousing::Generic
}

/// Default housing for the OFL (Open Fixture Library) path, where we don't
/// have the per-mode attribute structure ATLA carries. Maps off the parsed
/// `FixtureKind` since that's all OFL imports gives us today.
fn housing_from_kind(kind: FixtureKind) -> FixtureHousing {
    match kind {
        FixtureKind::Wash | FixtureKind::Blinder => FixtureHousing::Par,
        FixtureKind::Spot | FixtureKind::Beam => FixtureHousing::MovingHead,
        FixtureKind::Strobe => FixtureHousing::Strobe,
        FixtureKind::Laser => FixtureHousing::ScanLaser,
        FixtureKind::PixelBar => FixtureHousing::PixelBar,
        FixtureKind::VideoPanel => FixtureHousing::Panel,
        FixtureKind::Unknown => FixtureHousing::Generic,
    }
}

/// Count repeated RGB color cells in an attribute list. Each time a RED is
/// seen after the current cell already has one, a new cell begins - so an
/// 8x RGBW bar reports 8.
fn count_color_segments(attributes: &[AtlaAttribute]) -> usize {
    let mut segments = 0usize;
    let mut cell_has_red = false;
    for attr in attributes {
        let a = attribute_from_atla(attr.attr_type.as_deref(), attr.name.as_deref().unwrap_or(""));
        if a == DmxAttribute::Red {
            if cell_has_red {
                segments += 1;
            }
            cell_has_red = true;
        }
    }
    if cell_has_red {
        segments += 1;
    }
    segments
}

/// Maps an ATLABASE attribute `type` onto a `DmxAttribute`. Types without a
/// first-class attribute yet (AMBER, PAN_FINE, ACTIVITY, STATIC, ...) keep
/// the channel name verbatim as `Custom`; they get promoted to real variants
/// once a renderer actually consumes them (UV graduated for UV mode — only
/// UV-capable emitters lit).
fn attribute_from_atla(attr_type: Option<&str>, name: &str) -> DmxAttribute {
    match attr_type.unwrap_or("").to_ascii_uppercase().as_str() {
        "RED" => DmxAttribute::Red,
        "GREEN" => DmxAttribute::Green,
        "BLUE" => DmxAttribute::Blue,
        "WHITE" | "WARM_WHITE" | "COLD_WHITE" => DmxAttribute::White,
        "UV" => DmxAttribute::Uv,
        "DIMMER" | "MASTER_DIMMER" | "INTENSITY" => DmxAttribute::Dimmer,
        // BLACKOUT acts as the fixture's master enable (e.g. the scan-laser's
        // LASER ON/OFF toggle). Functionally a binary dimmer — 0 = off, full
        // = on — so it maps to the Dimmer slot. Safe because no profile in
        // the library carries both a DIMMER and a BLACKOUT channel; if one
        // ever does, this collapses to "last wins" and the schema would need
        // a dedicated `blackout` ChannelMap slot.
        "BLACKOUT" => DmxAttribute::Dimmer,
        "COLOR_WHEEL" => DmxAttribute::ColorWheel,
        "PAN" => DmxAttribute::Pan,
        "TILT" => DmxAttribute::Tilt,
        "ZOOM" => DmxAttribute::Zoom,
        "FOCUS" => DmxAttribute::Focus,
        "SHUTTER" | "STROBE" => DmxAttribute::Strobe,
        "GOBO" => DmxAttribute::GoboSlot,
        "ROTATION" | "GOBO_ROTATION" => DmxAttribute::GoboRotation,
        "PRISM" => DmxAttribute::Prism,
        "LASER" | "LASER_MACRO" | "MACRO" => DmxAttribute::LaserMacro,
        other => {
            let label = if name.is_empty() { other } else { name };
            DmxAttribute::Custom(label.to_string())
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaProfileFile {
    #[serde(rename = "fixtureProfile")]
    fixture_profile: AtlaFixtureProfile,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaFixtureProfile {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    manufacturer: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    mode: Vec<AtlaMode>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaMode {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    attribute: Vec<AtlaAttribute>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaAttribute {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "type")]
    attr_type: Option<String>,
    #[serde(default, rename = "staticValue")]
    static_value: Option<AtlaStaticValue>,
    #[serde(default, rename = "colorWheelSetting")]
    color_wheel_setting: Option<AtlaColorWheelSetting>,
    #[serde(default, rename = "goboSetting")]
    gobo_setting: Option<AtlaGoboSetting>,
    #[serde(default, rename = "prismSetting")]
    prism_setting: Option<AtlaPrismSetting>,
    #[serde(default, rename = "panSetting")]
    pan_setting: Option<AtlaMovementSetting>,
    #[serde(default, rename = "tiltSetting")]
    tilt_setting: Option<AtlaMovementSetting>,
    #[serde(default, rename = "toggleSetting")]
    toggle_setting: Option<serde_json::Value>,
    #[serde(default)]
    range: Option<AtlaRange>,
    #[serde(default)]
    smoothing: Option<f64>,
    #[serde(default)]
    invert: Option<bool>,
    #[serde(default, flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaStaticValue {
    #[serde(default)]
    value: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaColorWheelSetting {
    #[serde(default, rename = "allowSplitColors")]
    allow_split_colors: Option<bool>,
    #[serde(default)]
    colors: Vec<AtlaColor>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaColor {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default, rename = "lowValue")]
    low_value: Option<f64>,
    #[serde(default, rename = "highValue")]
    high_value: Option<f64>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    color: Vec<AtlaRgb>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaRgb {
    #[serde(default)]
    r: f64,
    #[serde(default)]
    g: f64,
    #[serde(default)]
    b: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaGoboSetting {
    #[serde(default)]
    steps: Vec<AtlaStep>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaPrismSetting {
    #[serde(default)]
    steps: Vec<AtlaStep>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaStep {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default, rename = "lowValue")]
    low_value: Option<f64>,
    #[serde(default, rename = "highValue")]
    high_value: Option<f64>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaMovementSetting {
    #[serde(default)]
    width: Option<f64>,
    #[serde(default)]
    offset: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AtlaRange {
    #[serde(default, rename = "lowValue")]
    low_value: Option<f64>,
    #[serde(default, rename = "highValue")]
    high_value: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct OflFixture {
    manufacturer: String,
    model: String,
    #[serde(default, rename = "fixtureKind")]
    fixture_kind: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    modes: Vec<OflMode>,
    #[serde(default)]
    commerce: Option<OflCommerce>,
}

#[derive(Debug, Deserialize)]
struct OflMode {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "shortName")]
    short_name: Option<String>,
    #[serde(default)]
    channels: Option<Vec<OflChannel>>,
}

#[derive(Debug, Deserialize)]
struct OflChannel {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    attribute: Option<String>,
    #[serde(default, rename = "defaultValue")]
    default_value: Option<u8>,
    #[serde(default, rename = "blackoutValue")]
    blackout_value: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct OflCommerce {
    #[serde(default, rename = "priceTier")]
    price_tier: Option<String>,
    #[serde(default, rename = "streetPriceRange")]
    street_price_range: Option<String>,
    #[serde(default, rename = "recommendationNotes")]
    recommendation_notes: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGBW_PAR: &str = include_str!("../../assets/fixture-profiles/rgbw-par.json");
    const MOVING_HEAD: &str = include_str!("../../assets/fixture-profiles/moving-head-spot.json");
    const LASER_MACRO: &str = include_str!("../../assets/fixture-profiles/laser-macro.json");

    #[test]
    fn loads_three_sample_fixture_profiles() {
        let rgbw = load_ofl_fixture_profile(RGBW_PAR, None).unwrap();
        let moving = load_ofl_fixture_profile(MOVING_HEAD, None).unwrap();
        let laser = load_ofl_fixture_profile(LASER_MACRO, None).unwrap();
        assert_eq!(rgbw.profile.kind, FixtureKind::Wash);
        assert_eq!(moving.profile.kind, FixtureKind::Spot);
        assert_eq!(laser.profile.kind, FixtureKind::Laser);
    }

    #[test]
    fn rgbw_profile_normalizes_color_channels() {
        let loaded = load_ofl_fixture_profile(RGBW_PAR, None).unwrap();
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::Dimmer));
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::Rgb));
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::White));
        assert_eq!(loaded.profile.channel_count, 5);
    }

    #[test]
    fn moving_head_profile_normalizes_motion_and_gobo() {
        let loaded = load_ofl_fixture_profile(MOVING_HEAD, None).unwrap();
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::PanTilt));
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::Gobo));
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::Prism));
        assert!(loaded.profile.channels.iter().any(|c| c.attribute == DmxAttribute::Pan));
        assert!(loaded.profile.channels.iter().any(|c| c.attribute == DmxAttribute::Tilt));
    }

    #[test]
    fn laser_profile_normalizes_macro_metadata_and_commerce() {
        let loaded = load_ofl_fixture_profile(LASER_MACRO, None).unwrap();
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::LaserMacro));
        assert_eq!(loaded.commerce.unwrap().price_tier.as_deref(), Some("mid"));
    }

    #[test]
    fn incomplete_profile_gets_manual_fallback_map() {
        let json = r#"{
            "manufacturer": "Fallback",
            "model": "Unknown RGBW Bar",
            "fixtureKind": "wash",
            "modes": [{"name": "empty", "channels": []}]
        }"#;
        let loaded = load_ofl_fixture_profile(json, None).unwrap();
        assert_eq!(loaded.profile.channel_count, 5);
        assert!(loaded.profile.capabilities.contains(&FixtureCapability::Rgb));
    }

    // --- ATLABASE format -------------------------------------------------

    const ATLA_LED_BEAM: &str = include_str!("../../assets/fixture-profiles/atla-led-beam-rgbw.json");
    const ATLA_SCORPION: &str = include_str!("../../assets/fixture-profiles/atla-scorpion-laser.json");
    const ATLA_FLAT_PAR: &str = include_str!("../../assets/fixture-profiles/atla-flat-par.json");

    fn channel_attr(profile: &FixtureProfile, offset: u16) -> &DmxAttribute {
        &profile.channels.iter().find(|c| c.channel_offset == offset).unwrap().attribute
    }

    #[test]
    fn atla_led_beam_parses_default_38_channel_mode() {
        let loaded = load_atla_fixture_profile(ATLA_LED_BEAM, None).unwrap();
        assert_eq!(loaded.profile.mode, "38 Channel");
        assert_eq!(loaded.profile.channel_count, 38);
        // An 8-cell RGBW array is a pixel bar, even though the model name says
        // "Beam" - the repeated-RGB structural signal wins over the name.
        assert_eq!(loaded.profile.kind, FixtureKind::PixelBar);
        for cap in [FixtureCapability::Dimmer, FixtureCapability::Rgb, FixtureCapability::White, FixtureCapability::Strobe] {
            assert!(loaded.profile.capabilities.contains(&cap), "missing {:?}", cap);
        }
        // ATLABASE attribute order == DMX channel order.
        assert_eq!(channel_attr(&loaded.profile, 1), &DmxAttribute::Tilt);
        assert_eq!(channel_attr(&loaded.profile, 5), &DmxAttribute::Dimmer);
        assert_eq!(channel_attr(&loaded.profile, 6), &DmxAttribute::Strobe);
    }

    #[test]
    fn atla_static_channels_keep_name_and_resting_value() {
        let loaded = load_atla_fixture_profile(ATLA_LED_BEAM, None).unwrap();
        let chans = &loaded.profile.channels;
        // Channel 2 is the STATIC "Tilt Speed" - kept verbatim, not coerced to Tilt.
        assert_eq!(chans[1].attribute, DmxAttribute::Custom("Tilt Speed".to_string()));
        // Master Dimmer ships a staticValue of 255 -> carried as the channel default.
        assert_eq!(chans[4].attribute, DmxAttribute::Dimmer);
        assert_eq!(chans[4].default_value, 255);
    }

    #[test]
    fn atla_mode_selection_picks_the_named_mode() {
        let nine = load_atla_fixture_profile(ATLA_LED_BEAM, Some("9 Channel")).unwrap();
        assert_eq!(nine.profile.mode, "9 Channel");
        assert_eq!(nine.profile.channel_count, 9);
        // Kind is a property of the hardware (derived from the richest mode),
        // so it stays PixelBar even in the single-color 9-channel mode...
        assert_eq!(nine.profile.kind, FixtureKind::PixelBar);
        // ...but with one RGBW cell the segment layout is empty, so this mode
        // renders / drives as a single-color fixture, not a per-cell bar.
        assert!(crate::emerge::contracts::segments_for_profile(&nine.profile, 1).is_empty());
    }

    #[test]
    fn atla_led_beam_resolves_eight_rgbw_segments() {
        use crate::emerge::contracts::segments_for_profile;
        let loaded = load_atla_fixture_profile(ATLA_LED_BEAM, None).unwrap();
        // 38-ch order: Tilt,TiltSpd,Eff,EffSpd,Dimmer,Strobe, then 8x RGBW.
        // start_address 1 -> abs address == channel offset, so Red1 = 7.
        let segs = segments_for_profile(&loaded.profile, 1);
        assert_eq!(segs.len(), 8);
        assert_eq!(segs[0].red, Some(7));
        assert_eq!(segs[0].green, Some(8));
        assert_eq!(segs[0].blue, Some(9));
        assert_eq!(segs[0].white, Some(10));
        assert_eq!(segs[7].red, Some(35));
        assert_eq!(segs[7].white, Some(38));
        // Offsetting the patch start address shifts every segment in lockstep.
        let segs100 = segments_for_profile(&loaded.profile, 100);
        assert_eq!(segs100[0].red, Some(106));
    }

    #[test]
    fn atla_scorpion_normalizes_motion_gobo_and_zoom() {
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        assert_eq!(loaded.profile.channel_count, 10);
        for cap in [FixtureCapability::PanTilt, FixtureCapability::Gobo, FixtureCapability::ColorWheel, FixtureCapability::Zoom, FixtureCapability::Strobe] {
            assert!(loaded.profile.capabilities.contains(&cap), "missing {:?}", cap);
        }
    }

    #[test]
    fn atla_flat_par_keeps_amber_custom_and_maps_uv() {
        let loaded = load_atla_fixture_profile(ATLA_FLAT_PAR, None).unwrap();
        assert_eq!(loaded.profile.kind, FixtureKind::Wash);
        assert_eq!(loaded.profile.channel_count, 10);
        for cap in [FixtureCapability::Dimmer, FixtureCapability::Rgb, FixtureCapability::White, FixtureCapability::Strobe] {
            assert!(loaded.profile.capabilities.contains(&cap), "missing {:?}", cap);
        }
        // AMBER has no first-class DmxAttribute yet - kept verbatim.
        assert!(loaded.profile.channels.iter().any(|c| c.attribute == DmxAttribute::Custom("Amber".to_string())));
        // UV graduated to a first-class attribute (UV mode addresses it
        // rig-wide), so the par's UV channel must classify as Uv, not Custom.
        assert!(loaded.profile.channels.iter().any(|c| c.attribute == DmxAttribute::Uv));
        assert!(!loaded.profile.channels.iter().any(|c| c.attribute == DmxAttribute::Custom("UV".to_string())));
    }

    /// Parses every profile in the live NAS hardware library. Skips cleanly
    /// when the NAS is not mounted, so it is not flaky off-network.
    #[test]
    fn atla_nas_profile_library_all_parse_when_mounted() {
        let dir = ["/Volumes/Compeller/react/hardware/profile", "/mnt/compeller/react/hardware/profile"]
            .into_iter()
            .map(std::path::PathBuf::from)
            .find(|p| p.is_dir());
        let Some(dir) = dir else { return };

        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let json = std::fs::read_to_string(&path).unwrap();
            let loaded = load_atla_fixture_profile(&json, None)
                .unwrap_or_else(|e| panic!("{} failed to parse: {}", path.display(), e));
            assert!(loaded.profile.channel_count > 0, "{} parsed with 0 channels", path.display());
            checked += 1;
        }
        assert!(checked > 0, "NAS profile dir {} held no .json files", dir.display());
    }

    #[test]
    fn atla_repo_local_nas_profiles_all_parse_and_have_snapshots() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest.join("assets/nas-fixture-profiles");
        let snapshot_dir = manifest.join("assets/fixture-capability-snapshots");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("repo-local NAS fixture profile dir exists") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let json = std::fs::read_to_string(&path).unwrap();
            let loaded = load_atla_fixture_profile(&json, None)
                .unwrap_or_else(|e| panic!("{} failed to parse: {}", path.display(), e));
            assert!(loaded.profile.channel_count > 0, "{} parsed with 0 channels", path.display());
            assert!(
                loaded.profile.channels.iter().any(|c| c.detail.is_some()),
                "{} has no preserved rich channel detail",
                path.display()
            );
            assert!(
                loaded.profile.channels.iter().all(|c| c.source_attribute.is_some()),
                "{} lost one or more source ATLABASE attributes",
                path.display()
            );

            let name = path.file_name().unwrap();
            let snapshot = snapshot_dir.join(name);
            let snapshot_json = std::fs::read_to_string(&snapshot)
                .unwrap_or_else(|e| panic!("missing snapshot {}: {}", snapshot.display(), e));
            let snapshot_value: serde_json::Value = serde_json::from_str(&snapshot_json)
                .unwrap_or_else(|e| panic!("invalid snapshot {}: {}", snapshot.display(), e));
            assert_eq!(
                snapshot_value.get("source_file").and_then(|v| v.as_str()),
                path.file_name().and_then(|s| s.to_str()),
                "snapshot source_file must match source profile"
            );
            checked += 1;
        }
        assert_eq!(checked, 7, "expected exactly the 7 NAS fixture profiles");
    }

    #[test]
    fn bundled_library_resolves_the_rig_offline() {
        // Offline FLOOR: the binary-embedded profiles resolve with NO NAS mount,
        // so a venue laptop gets correct ChannelMaps instead of magenta/garbage.
        let lib = FixtureProfileLibrary::bundled();
        // Cross-check: every profile in assets/nas-fixture-profiles/ must be embedded
        // by bundled(), so adding a NAS fixture without bundling it (which would make
        // it garbage offline) fails this test.
        let asset_count = std::fs::read_dir(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/nas-fixture-profiles"),
        )
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .count();
        assert_eq!(lib.len(), asset_count, "every assets/nas-fixture-profiles profile must be bundled()");
        assert_eq!(lib.len(), 7, "the rig currently has 7 fixtures");
        // The 200W RGB strobe resolves to the right per-mode channel counts so its
        // ChannelMap is correct offline in whichever mode the operator patched.
        let id = "0443a2c9-6ca7-486e-a573-3e7c8a3b4367";
        assert_eq!(lib.resolve(id, "3 Channel").unwrap().channel_count, 3);
        assert_eq!(lib.resolve(id, "9 Channel").unwrap().channel_count, 9);
        assert_eq!(lib.resolve(id, "24 Channel").unwrap().channel_count, 24);
    }

    #[test]
    fn nas_profiles_classify_to_expected_housing() {
        // Each NAS profile must classify to a non-Generic housing — Generic
        // is reserved for fixtures the parser genuinely can't shape. Plus
        // pin the specific housings we expect for the seven library
        // fixtures today so a regression in `infer_housing` fails loud.
        use std::collections::HashMap;
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest.join("assets/nas-fixture-profiles");
        let expected: HashMap<&str, FixtureHousing> = HashMap::from([
            ("shehds-200w-rgb-strobe-light-8-segment.json", FixtureHousing::Panel),
            ("shehds-led-flat-par-18x18w.json", FixtureHousing::Par),
            ("shehds-led-beam-8x12w-rgbw.json", FixtureHousing::PixelBar),
            ("shehds-7r-beam-move-head-light.json", FixtureHousing::MovingHead),
            ("shehds-3w-scan-laser.json", FixtureHousing::ScanLaser),
            ("chauvet-dj-obsession.json", FixtureHousing::MovingHead),
            // Chauvet Scorpion is a moving-head laser — the profile carries
            // Pan + Tilt but no "laser" keyword anywhere, so it classifies
            // on the yoke (MovingHead). TODO: surface laser optics via a
            // structural signal (X/Y/Z Axis Rolling channels) if we want
            // to render its actual laser fan visual.
            ("chauvet-dj-scorpion-dual-rgb.json", FixtureHousing::MovingHead),
        ]);
        for entry in std::fs::read_dir(&dir).expect("nas dir exists") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let json = std::fs::read_to_string(&path).unwrap();
            let loaded = load_atla_fixture_profile(&json, None)
                .unwrap_or_else(|e| panic!("{} failed to parse: {}", path.display(), e));
            assert_ne!(
                loaded.profile.housing,
                FixtureHousing::Generic,
                "{} classified as Generic — extend infer_housing()",
                path.display()
            );
            let name = path.file_name().and_then(|s| s.to_str()).unwrap();
            if let Some(want) = expected.get(name) {
                assert_eq!(
                    loaded.profile.housing, *want,
                    "{}: housing mismatch, expected {:?} got {:?}",
                    name, want, loaded.profile.housing
                );
            }
        }
    }

    #[test]
    fn shehds_200w_strobe_housing_is_mode_stable() {
        // The 200W RGB Strobe must register as Panel housing in every mode
        // its profile exposes (3-channel / 9-channel / 24-channel). The
        // renderer keys off `profile.housing` to keep the on-stage fixture
        // shape identical regardless of which mode SaaS picked at patch
        // time — only the channels available for cues change per mode.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/nas-fixture-profiles/shehds-200w-rgb-strobe-light-8-segment.json");
        let json = std::fs::read_to_string(&path).unwrap();
        for mode_name in ["3 Channel", "9 Channel", "24 Channel"] {
            let loaded = load_atla_fixture_profile(&json, Some(mode_name))
                .unwrap_or_else(|e| panic!("mode {} failed to parse: {}", mode_name, e));
            assert_eq!(
                loaded.profile.housing,
                FixtureHousing::Panel,
                "200W RGB Strobe must be Panel in {} mode, got {:?}",
                mode_name,
                loaded.profile.housing
            );
            // FixtureKind should also be mode-stable (Strobe family per
            // ATLA category, regardless of multi-cell structure).
            assert_eq!(
                loaded.profile.kind,
                FixtureKind::Strobe,
                "200W RGB Strobe family must be Strobe in {} mode, got {:?}",
                mode_name,
                loaded.profile.kind,
            );
        }
    }

    #[test]
    fn library_scans_atla_profiles_and_resolves_by_id() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fixture-profiles");
        let library = FixtureProfileLibrary::scan_dir(&dir);
        // The 3 bundled ATLABASE samples index; the OFL samples are skipped.
        assert!(library.len() >= 3, "expected >= 3 atla profiles, got {}", library.len());

        // Resolve the LED Beam by its ATLABASE fixtureProfile.id.
        let beam = library
            .resolve("36f69c64-f994-4a56-b5ee-2678540f62c4", "38 Channel")
            .expect("LED beam resolves by atla id");
        assert_eq!(beam.channel_count, 38);

        // An empty mode falls back to the profile's first mode.
        let beam_default = library
            .resolve("36f69c64-f994-4a56-b5ee-2678540f62c4", "")
            .unwrap();
        assert_eq!(beam_default.mode, "38 Channel");

        // An unknown id resolves to nothing.
        assert!(library.resolve("00000000-0000-0000-0000-000000000000", "").is_none());
    }

    // --- COM-494: rich capability detail is preserved, never discarded -----

    fn channel(profile: &FixtureProfile, offset: u16) -> &ChannelDefinition {
        profile.channels.iter().find(|c| c.channel_offset == offset).unwrap()
    }

    #[test]
    fn atla_color_wheel_slots_are_preserved() {
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        // Channel 3 is the COLOR_WHEEL "Color".
        let detail = channel(&loaded.profile, 3).detail.as_ref().expect("color wheel detail");
        match detail {
            ChannelDetail::ColorWheel { allow_split_colors, slots } => {
                assert!(*allow_split_colors, "split-color flag preserved");
                assert_eq!(slots.len(), 10, "all 10 color slots kept");
                // Slot 0 = Black @ DMX 0, rgb (0,0,0).
                assert_eq!(slots[0].name, "Black");
                assert_eq!(slots[0].value, 0);
                assert_eq!(slots[0].rgb, Some([0.0, 0.0, 0.0]));
                // Slot 1 = Red @ DMX 35, rgb (1,0,0).
                assert_eq!(slots[1].name, "Red");
                assert_eq!(slots[1].value, 35);
                assert_eq!(slots[1].rgb, Some([1.0, 0.0, 0.0]));
            }
            other => panic!("expected ColorWheel detail, got {:?}", other),
        }
    }

    #[test]
    fn atla_gobo_steps_are_preserved() {
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        // Channel 2 is the GOBO "Pattern" wheel - 32 pattern steps.
        let detail = channel(&loaded.profile, 2).detail.as_ref().expect("gobo detail");
        match detail {
            ChannelDetail::Gobo { steps } => {
                assert_eq!(steps.len(), 32, "all 32 gobo steps kept");
                assert_eq!(steps[0].name, "Pattern 1 ");
                assert_eq!(steps[0].value, 0);
                assert!(steps[0].enabled);
            }
            other => panic!("expected Gobo detail, got {:?}", other),
        }
    }

    #[test]
    fn atla_pan_tilt_movement_detail_is_preserved() {
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        // Channel 6 = PAN with physical width/offset + usable range.
        match channel(&loaded.profile, 6).detail.as_ref().expect("pan detail") {
            ChannelDetail::Movement { range, invert, width, offset, .. } => {
                assert_eq!(*width, Some(0.493));
                assert_eq!(*offset, Some(0.006));
                assert!(!*invert);
                let r = range.as_ref().expect("pan range");
                assert_eq!(r.low, 0.0);
                assert!((r.high - 0.498039).abs() < 1e-6, "pan high range kept: {}", r.high);
            }
            other => panic!("expected Movement detail, got {:?}", other),
        }
        // Channel 7 = TILT, negative offset preserved verbatim.
        match channel(&loaded.profile, 7).detail.as_ref().expect("tilt detail") {
            ChannelDetail::Movement { width, offset, .. } => {
                assert_eq!(*width, Some(0.492));
                assert_eq!(*offset, Some(-0.003));
            }
            other => panic!("expected Movement detail, got {:?}", other),
        }
    }

    #[test]
    fn atla_continuous_curve_range_is_preserved() {
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        // Channel 5 = ZOOM: a continuous curve carrying range + invert.
        match channel(&loaded.profile, 5).detail.as_ref().expect("zoom detail") {
            ChannelDetail::Range { range, invert, .. } => {
                assert!(!*invert);
                let r = range.as_ref().expect("zoom range");
                assert!((r.high - 0.498039).abs() < 1e-6, "zoom high range kept: {}", r.high);
            }
            other => panic!("expected Range detail, got {:?}", other),
        }
    }

    #[test]
    fn atla_static_channels_carry_static_detail() {
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        // Channel 1 = STATIC "Control mode", resting value 0.
        match channel(&loaded.profile, 1).detail.as_ref().expect("static detail") {
            ChannelDetail::Static { value } => assert_eq!(*value, 0),
            other => panic!("expected Static detail, got {:?}", other),
        }
    }

    #[test]
    fn atla_no_rich_setting_is_discarded() {
        // Every ATLABASE attribute that carries a rich nested setting
        // (colorWheelSetting, goboSetting, pan/tiltSetting, range, staticValue)
        // must surface a `detail`. The Scorpion exercises wheels, motion,
        // curve, and static channels in one fixture, so its detail coverage is
        // a tight regression guard against silent data loss.
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        // 10 channels: Control(static), Gobo, Color, Strobe(opaque, no detail),
        // Zoom(range), Pan(move), Tilt(move), 3x static rolling axes.
        let with_detail = loaded.profile.channels.iter().filter(|c| c.detail.is_some()).count();
        assert_eq!(with_detail, 9, "9 of 10 channels carry preserved detail (Strobe is opaque)");

        // Spot-check the kinds line up channel-for-channel.
        assert!(matches!(channel(&loaded.profile, 1).detail, Some(ChannelDetail::Static { .. })));
        assert!(matches!(channel(&loaded.profile, 2).detail, Some(ChannelDetail::Gobo { .. })));
        assert!(matches!(channel(&loaded.profile, 3).detail, Some(ChannelDetail::ColorWheel { .. })));
        assert!(matches!(channel(&loaded.profile, 8).detail, Some(ChannelDetail::Static { .. })));
    }

    #[test]
    fn fixture_profile_detail_survives_json_round_trip() {
        // The contract must serialize and re-parse without losing any rich
        // detail - this is what SaaS/Paperclip stores and serves.
        let loaded = load_atla_fixture_profile(ATLA_SCORPION, None).unwrap();
        let json = serde_json::to_string(&loaded.profile).unwrap();
        let round_tripped: FixtureProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.profile, round_tripped, "rich detail round-trips losslessly");
    }

    #[test]
    fn ofl_channels_carry_no_atla_detail() {
        // OFL-shaped profiles have no ATLABASE rich detail; the field stays
        // None and is omitted from the serialized contract.
        let loaded = load_ofl_fixture_profile(MOVING_HEAD, None).unwrap();
        assert!(loaded.profile.channels.iter().all(|c| c.detail.is_none()));
        let json = serde_json::to_string(&loaded.profile).unwrap();
        assert!(!json.contains("\"detail\""), "None detail omitted from JSON");
    }

    // --- all_profiles is served from the scan-time cache -------------------

    #[test]
    fn all_profiles_is_served_from_the_scan_time_cache() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fixture-profiles");
        let library = FixtureProfileLibrary::scan_dir(&dir);
        let first = library.all_profiles();
        assert!(first.len() >= 3, "the 3 sample ATLA profiles index");
        // Palette order contract: sorted by manufacturer + model + mode.
        for pair in first.windows(2) {
            let a = (&pair[0].manufacturer, &pair[0].model, &pair[0].mode);
            let b = (&pair[1].manufacturer, &pair[1].model, &pair[1].mode);
            assert!(a <= b, "palette must stay sorted: {:?} > {:?}", a, b);
        }
        // Identical across calls — the per-frame palette read hits the cache.
        assert_eq!(first, library.all_profiles());
        // Cache contents match an on-demand resolve of the same id (empty
        // mode = the profile's first mode, same as the cache build).
        for p in &first {
            let direct = library.resolve(&p.id, "").expect("cached profile resolves by id");
            assert_eq!(&direct, p, "cache must mirror a fresh parse of {}", p.id);
        }
        // The bundled (offline-floor) constructor caches too; an empty
        // library serves an empty palette.
        assert_eq!(FixtureProfileLibrary::bundled().all_profiles().len(), 7);
        assert!(FixtureProfileLibrary::default().all_profiles().is_empty());
    }

    // --- unknown mode names fall back loudly, never silently ---------------

    #[test]
    fn unknown_mode_falls_back_to_first_mode_in_both_paths() {
        // ATLA path: a stale SaaS mode name (profile edited/renamed on the
        // NAS) keeps resolving — degraded to the first mode, with a warn in
        // the log (the silent fallback was the rogue-strobe failure mode).
        let atla_default = load_atla_fixture_profile(ATLA_LED_BEAM, None).unwrap();
        let atla = load_atla_fixture_profile(ATLA_LED_BEAM, Some("No Such Mode")).unwrap();
        assert_eq!(atla.profile.mode, atla_default.profile.mode);
        assert_eq!(atla.profile.channel_count, atla_default.profile.channel_count);

        // OFL path: same degraded-but-loud fallback contract.
        let ofl_default = load_ofl_fixture_profile(MOVING_HEAD, None).unwrap();
        let ofl = load_ofl_fixture_profile(MOVING_HEAD, Some("No Such Mode")).unwrap();
        assert_eq!(ofl.profile.mode, ofl_default.profile.mode);
        assert_eq!(ofl.profile.channel_count, ofl_default.profile.channel_count);
    }

    // --- fine channels and word-boundary pan/tilt matching ------------------

    #[test]
    fn fine_channels_classify_as_custom_never_coarse() {
        assert_eq!(attribute_from_label("Pan fine"), DmxAttribute::Custom("Pan fine".to_string()));
        assert_eq!(attribute_from_label("Tilt Fine"), DmxAttribute::Custom("Tilt Fine".to_string()));
        assert_eq!(attribute_from_label("Pan-Fine"), DmxAttribute::Custom("Pan-Fine".to_string()));
        assert_eq!(attribute_from_label("Dimmer fine"), DmxAttribute::Custom("Dimmer fine".to_string()));
        // Coarse partners keep their first-class slots.
        assert_eq!(attribute_from_label("Pan"), DmxAttribute::Pan);
        assert_eq!(attribute_from_label("Tilt"), DmxAttribute::Tilt);
        assert_eq!(attribute_from_label("Dimmer"), DmxAttribute::Dimmer);
    }

    #[test]
    fn panel_mode_label_does_not_classify_as_pan() {
        assert_eq!(attribute_from_label("Panel Mode"), DmxAttribute::Custom("Panel Mode".to_string()));
        // Word-boundary matching still accepts real compound pan/tilt labels.
        assert_eq!(attribute_from_label("Pan movement"), DmxAttribute::Pan);
        assert_eq!(attribute_from_label("Pan/Tilt macro"), DmxAttribute::Pan);
        assert_eq!(attribute_from_label("Head Tilt"), DmxAttribute::Tilt);
    }

    #[test]
    fn uv_labels_classify_first_class_with_word_boundary_and_fine_rules() {
        // OFL "UV" labels map to the first-class Uv attribute (UV mode).
        assert_eq!(attribute_from_label("UV"), DmxAttribute::Uv);
        assert_eq!(attribute_from_label("UV LEDs"), DmxAttribute::Uv);
        assert_eq!(attribute_from_label("Ultraviolet (UV)"), DmxAttribute::Uv);
        // Word-boundary: "uv" inside a word must NOT claim the slot.
        assert_eq!(attribute_from_label("Fluvial"), DmxAttribute::Custom("Fluvial".to_string()));
        // 16-bit fine partner stays Custom so a coarse UV channel keeps the
        // slot (same last-wins rule as Pan/Tilt fine).
        assert_eq!(attribute_from_label("UV fine"), DmxAttribute::Custom("UV fine".to_string()));
    }

    #[test]
    fn atla_uv_type_maps_to_first_class_uv() {
        assert_eq!(attribute_from_atla(Some("UV"), "UV"), DmxAttribute::Uv);
        // AMBER is still un-promoted - keeps the channel name verbatim.
        assert_eq!(attribute_from_atla(Some("AMBER"), "Amber"), DmxAttribute::Custom("Amber".to_string()));
    }

    #[test]
    fn ofl_pan_fine_maps_coarse_channel_into_the_pan_slot() {
        // The 16-bit mover regression: "Pan fine" contains "pan", and
        // ChannelMap::from_profile is last-wins, so the FINE channel used to
        // end up in the Pan slot — the head pinned to one side. The fine
        // channels must classify Custom so the coarse offsets win.
        let json = r#"{
            "manufacturer": "Test",
            "model": "16-bit Mover",
            "fixtureKind": "spot",
            "modes": [{"name": "16bit", "channels": [
                {"name": "Pan", "attribute": "Pan"},
                {"name": "Pan fine", "attribute": "Pan fine"},
                {"name": "Tilt", "attribute": "Tilt"},
                {"name": "Tilt fine", "attribute": "Tilt fine"},
                {"name": "Dimmer", "attribute": "Dimmer"}
            ]}]
        }"#;
        let loaded = load_ofl_fixture_profile(json, None).unwrap();
        let p = &loaded.profile;
        assert_eq!(channel_attr(p, 1), &DmxAttribute::Pan);
        assert_eq!(channel_attr(p, 2), &DmxAttribute::Custom("Pan fine".to_string()));
        assert_eq!(channel_attr(p, 3), &DmxAttribute::Tilt);
        assert_eq!(channel_attr(p, 4), &DmxAttribute::Custom("Tilt fine".to_string()));
        // And the renderer map holds the COARSE addresses (start_address 1 →
        // absolute address == offset).
        let map = crate::emerge::contracts::ChannelMap::from_profile(1, p);
        assert_eq!(map.pan, Some(1), "Pan slot must hold the coarse channel");
        assert_eq!(map.tilt, Some(3), "Tilt slot must hold the coarse channel");
    }
}
