# REACT Virtual Stage fixture profiles

COM-406 keeps fixture ingestion practical for the MVP.

## MVP source: Open Fixture Library style JSON

The first loader accepts a small OFL-inspired JSON subset that is easy to ship
with REACT assets and easy to hand-author for venues. It normalizes fixture
manufacturer/model/mode/kind and per-channel attributes into the backend-neutral
`FixtureProfile` contract from `src/dmx/contracts.rs`.

Included sample classes:

- RGBW PAR / wash
- Moving-head spot / beam
- Simple DMX laser macro fixture

Manual fallback channel maps remain supported because many low-cost fixtures
have incomplete or inconsistent metadata.

## Commerce metadata

COM-406 stores cheap extension points for future product guidance:

- price tier
- street price range
- recommendation notes

This metadata is downstream. It must not block the first quality gate: a working
virtual stage simulator.

## GDTF phase-two notes

The current Rust crate landscape has `gdtf = 0.2.0`, described as tools to read
and inspect General Device Type Format files. That is useful for phase-two
research, but COM-406 intentionally does not make GDTF a runtime dependency.

Likely reliable phase-two fields:

- fixture/manufacturer/model names
- DMX mode names
- logical channel and attribute names
- basic geometry hints where present

Fields to treat cautiously until tested with real venue fixtures:

- wheels/gobos/prisms/macros
- physical beam/lens details
- calibration and manufacturer-specific personality quirks
- safety-relevant laser semantics

Decision: defer native GDTF ingestion until the virtual stage renderer works with
our normalized contracts and OFL/manual profiles.
