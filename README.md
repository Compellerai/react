# REACT by Compeller.ai

**Real-time, audio-reactive visuals, lighting and streaming for live performance — built to empower musicians and venues.**

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

REACT is a native macOS desktop application written in Rust (egui + OpenGL/Metal).
It listens to the music as it plays, analyzes it, and drives visuals, stage
lighting and live streams from what it hears.

## Quick start

Requires macOS 12 or later (developed and tested on Apple Silicon), the Xcode
Command Line Tools, [Rust](https://rustup.rs) and [Homebrew](https://brew.sh).

```bash
brew install cmake pkg-config ffmpeg gstreamer
git clone https://github.com/Compellerai/react.git
cd react
./start-react.sh
```

The first build compiles every dependency and takes a few minutes. macOS will
ask for Screen Recording permission; REACT needs it to capture system audio.

## Contents

- [Overview](#overview) · [Features](#features) · [Architecture](#architecture)
- [Harmonic Geometry Engine](#harmonic-geometry-engine) · [Audio Analysis Details](#audio-analysis-details)
- [Building](#building) · [Keyboard Shortcuts](#keyboard-shortcuts) · [Configuration](#configuration)
- [Database Schema](#database-schema) · [Troubleshooting](#troubleshooting) · [Dependencies](#dependencies)
- [Contributing](#contributing) · [License](#license)

## Overview

REACT is a professional-grade visualization platform designed for musicians, DJs, and entertainment venues. It captures audio in real-time, analyzes it for musical content (beats, frequencies, key, harmony), and generates stunning visuals that respond to the music.

**Key Differentiators:**
- **Harmonic Geometry Engine**: Original torus knot visualization driven by musical theory and quaternion mathematics
- **Mythos Engine**: Continuous field visualization with Collatz-sequence modulation, multi-layer rendering, and OSC export for DMX lighting
- **Hero's Journey System**: 4-act temporal narrative (Birth, Lock, Dive, Afterglow) that emerges from audio
- **Genre-Aware Lighting**: 45+ music genre profiles with tuned thresholds, palettes, and behaviors
- **3-Layer Cue Engine**: Palette → Zone → Accents state machine with envelope-driven effects
- **Camera Tracking System**: Multi-camera face detection with tracking mix modes
- **Crash-Resilient Recording**: Segmented recording with journal-based recovery
- **Dual RTMP Streaming**: Simultaneous multi-destination streaming
- **Three-Stage Gain Model**: Calibration + Level Assist + user gain with safety limiter
- **Philips Hue Entertainment**: DTLS/UDP streaming to Hue lights with role-based color mapping
- **WLED LED Strips**: UDP DRGB streaming with spectrum, beat, and pulse visualizations
- **Pro DJ Integration**: Native support for Pioneer/AlphaTheta CDJs via Pro DJ Link protocol
- **Live Streaming**: Native YouTube Live and Twitch streaming with GStreamer/FFmpeg encoding up to 4K60
- **Network Output**: NDI streaming at HD/4K/Ultrawide with bicubic upscaling
- **Zero-latency Audio**: ScreenCaptureKit on macOS for direct system audio capture

## Features

### Audio-Reactive Visualizations

- **Multi-Renderer Theme System**: Unified interface supporting multiple visualization backends
  - ProjectM (MilkDrop-compatible)
  - LED Wall (7 patterns, 32x18 pixel grid, instanced rendering)
  - Harmonic Geometry (Geometric Field)
  - Mythos (continuous field with Collatz-sequence modulation)
  - SonicGeometry (future)
- **LED Wall Patterns**: Spectrum, Bars, Pulse, Wave, VuMeter, Fire, Rain
- **Real-time Audio Capture**: ScreenCaptureKit (system/app audio), cpal (input devices), GStreamer (multi-channel USB)
  - 19+ recognized audio apps (Apple Music, Spotify, Chrome, Safari, Firefox, Logic Pro, Ableton Live, etc.)
  - 20+ DJ mixer/interface device profiles (Pioneer DJM-A9, V10, 900NXS2; Allen & Heath; Native Instruments; Denon; Rane; Focusrite; Universal Audio Apollo)
  - Auto stereo-pair detection for multi-channel devices via GStreamer probe
- **Beat Detection**: Multi-stage kick detection (ratio gate + envelope rise + dynamic floor), 80ms refractory period
- **Three-Stage Audio Gain Model**:
  - *Calibration*: Per-source defaults (SCK=2.0, mic=1.5, device=profile-specific)
  - *Level Assist*: Automatic normalization (target -12dBFS, one-shot, capped, hold/cooldown)
  - *User gain*: UI slider (0.1-16.0)
  - *Safety limiter*: 0.95 ceiling, 5ms attack, 100ms release
- **Audio Bus**: Lock-free SPSC fanout to 5 independent consumers (visualization, meters, NDI, recording, RTMP)
  - 960-frame chunks (20ms @ 48kHz), 12000-frame queue (~250ms)
  - Per-destination drop tracking
- **Adaptive Normalizer**: Percentile-based signal normalization with warmup convergence tracking

### Hero's Journey System

The visualization follows a 4-act temporal structure that emerges from audio analysis:

- **Birth**: Initial emergence, viewpoint outside the harmonic field
- **Lock**: Stability achieved, angular symmetry established
- **Dive**: Viewpoint crosses implicit surface boundary, interior traversal
- **Afterglow**: Residual harmonic coherence, graceful decay

Each act has mathematical invariants governing camera, topology, and geometry behavior.

The system uses **Topology Intent** states that declare visual behavior:
- `Stable`: Slow response (0.05), grounded feel
- `Transitional`: Moderate response (0.15), morphing between shapes
- `Emergent`: Fast response (0.35), rising complexity
- `SymmetryLock`: Instant response (1.0), hard structure lock

### Skin System

Video content integrates as visual textures on the harmonic geometry:

- **Surface Mode**: Compel video as albedo texture
- **Emissive Mode**: Video drives glow patterns
- **SurfaceEmissive Mode**: Combined surface + emissive blending
- **UV Modes**: Cylindrical (0), Triplanar (1)
- **Effects**: Rim/fresnel flare, bloom, starburst lens, specular sparkle, edge outlines
- **Hero Preset**: contrast=1.35, saturation=1.25, emissive=0.5, surface_mix=0.9
- **Shadow Lift and AO Clamping**: Dynamic ambient occlusion control
- **4-Act Gating**: Visibility controlled by Hero's Journey phase

### QS Wheel (Quaternion Symmetry Wheel)

Interactive visualization of harmonic relationships:

- **Anchor Quantization**: `anchor_k` → `anchor_theta_deg` = k × (360/N), ALWAYS on-grid
- **REG Degree**: `reg_degree_a` = harmonic integer (1-360), INFO telemetry only, never rotation authority
- **Tri-State Lattice**: N/A (QC off) | true (on 15° grid) | VIOLATION (off-grid)
- **Polarity Tracking**: Quadrant-based detection with hysteresis
- **Warp Jump Effects**: Shrink, pop, and jiggle animations during phase transitions
- **Camera Effects**: Yaw nudge and orbit direction based on polarity changes

### Mythos Engine

A continuous field visualization where an invisible medium is excited by music through a Collatz-sequence (3x+1 / hailstone) modulation layer. The field is the base visual reality; the state machine creates expansion and contraction patterns driven by audio energy and beats.

**Core Architecture:**
- **Continuous Field**: Multi-layer domain-warped FBM noise fields rendered as a fullscreen fragment shader
- **Collatz State Machine**: 3 independent hailstone sequences modulate field behavior
  - Odd steps (3x+1) → expansion, excitation, increased instability
  - Even steps (x/2) → contraction, relaxation, reduced instability
  - Beat-triggered stepping with energy-modulated interval (0.06-0.30s)
  - Musically-meaningful reseeding from dominant pitch class on strong beats
- **Multi-Layer Rendering**: 3 Collatz layers with independent seeds, stepping rates, and blend weights
  - Quality-gated: Low=1 layer, Med=2, High+=3
  - FBM octaves scale with quality tier (2/3/4 octaves)
- **Act System**: Emergent 4-phase topology from Collatz heat dynamics
  - Dormant → Rising → Peak → Falling (automatic transitions)
  - Modulates color temperature, brightness, and distortion

**5 Presets:**
| Preset | Palette | Character |
|--------|---------|-----------|
| Mythos | Violet→Cyan | Balanced default |
| Deep Aether | Navy→Ice | Slow, mysterious |
| Hailstone Storm | Purple→Pink | Aggressive, volatile |
| Harmonic Drift | Violet→Cyan | Chroma-influenced, flowing |
| Solar Furnace | Black→Yellow | Fire palette, beat-reactive |

**5 Color Palettes**: Default (violet-cyan), Warm (red-gold), Cold (navy-ice), Electric (purple-pink), Fire (black-yellow)

**Parameters** (all MIDI-learnable):
- **Instability**: How strongly Collatz state affects the field (0=calm, 1=volatile)
- **Field Strength**: Base intensity and scale of the continuous field
- **Disturbance Size**: Radius of audio-injected perturbations
- **Harmonic Influence**: How much 12-bin chroma shapes and tints the field
- **Decay**: How quickly disturbances fade

**Audio Integration:**
- Bass → large-scale radial undulation
- Mid → medium-frequency noise perturbation
- High → fine shimmer overlay
- Kick → expanding ring disturbance (decays with distance)
- 12-bin chroma → angular tonal geometry modulation

**Video Skin**: Surface, Emissive, or SurfaceEmissive blending of video content onto the field

**Lighting Signals** (available to all outputs: OSC, Hue, WLED, DMX-ArtNet):

Collatz state is embedded in `LightingMetrics` and `CueSnapshot`, making it a first-class lighting signal alongside energy, tension, and balance. All lighting consumers receive it automatically.

| Signal | Type | Description |
|--------|------|-------------|
| `collatz_heat` | Float 0-1 | Accumulated instability (color temperature, intensity) |
| `collatz_phase` | Float 0/1 | Expansion (1) vs contraction (0) (strobe gate, direction) |
| `collatz_intensity` | Float 0-1 | Current sequence magnitude (brightness) |
| `collatz_breath` | Float 0-1 | Smoothed expansion-contraction cycle (movement, dimming) |
| `collatz_act` | Int 0-3 | Dormant/Rising/Peak/Falling (scene selection) |

OSC addresses: `/mythos/heat`, `/mythos/phase`, `/mythos/intensity`, `/mythos/breath`, `/mythos/act`

### Compositor Effects

- **Multi-Scale Kawase Bloom**: 3-scale Gaussian with configurable intensity
- **Chromatic Aberration**: RGB channel offset for lens effect
- **Film Grain**: Dual-pass pseudo-random noise
- **Soft Vignette**: Screen-edge darkening
- **Luma Key Transparency**: Brightness-based compositing for both video overlay and camera layer

### Video Compositing

Render order (back to front):

1. **Theme** (ProjectM / LED Wall / Harmonic Geometry / Mythos)
2. **Fullscreen Camera Layer** — highest-alpha camera drawn fullscreen with optional luma key
3. **Video Overlay** — compel video with luma key (dark areas reveal camera layer beneath)
4. **Camera HG Compositor** — scheduler tick for HG QS wheel camera texture
5. **Logo / Track Overlays**

- **Fullscreen Camera Layer**: Live camera composited between theme and video on all render modes
  - Highest-alpha camera automatically selected for fullscreen display
  - Camera Luma Key slider (0.0–1.0): dark camera areas become transparent, revealing theme
  - MIDI-mappable camera luma key control
  - Momentary Camera Solo: hold MIDI note to solo a camera, release to restore
- **Video Overlay**: Layer MP4 videos over visualizations with transparency
- **Audio-Reactive Visibility**: Videos fade based on audio levels (configurable silence threshold)
- **Triple-Buffered PBOs**: Async 4K texture uploads for smooth performance
- **CVOpenGLTextureCache / IOSurface**: Zero-copy GPU texture sharing (macOS)
- **Logo Overlay**: Add branding with configurable position and opacity
- **TAA (Temporal Anti-Aliasing)**: Sub-pixel jittering and history-based blending
- **Grid View**: Visual video browser with 120x68 thumbnails in auto-wrapping layout
- **Compel Picker**: Fuzzy search browser for selecting compels
- **Compel-to-Track Matching**: Automatic compel assignment based on track metadata

### Output Options

- **NDI Streaming**: Network video output at HD (1080p), 4K (2160p), or Ultrawide (5120x1440)
  - Bicubic upscaling with Catmull-Rom interpolation
  - Optional audio passthrough
  - Tally support for live production
- **Output Window**: Dedicated borderless/fullscreen window for projection/capture
  - IOSurface zero-copy texture sharing
  - Proper aspect ratio handling
- **Segmented Recording** (crash-resilient):
  - 10-second segments with journal-based recovery
  - Post-recording stitch into single MP4
  - Hardware encoding via VideoToolbox (macOS)
  - Metrics: duration, segment count, committed segments
  - Automatic recovery on unexpected shutdown
- **Recording Fingerprint Pipeline** (forensic tracing):
  - **Visible Watermark**: Compeller.ai logo at 20% opacity in bottom-right via FFmpeg overlay filter. Removable as paid tier perk (driven by `entitlements.watermark`).
  - **Invisible Audio Fingerprint**: DSSS/BPSK ultrasonic signal at 19kHz encoding account_id + timestamp. Gold code spreading (31 chips/bit, 300 chips/sec), -40dB amplitude. Survives lossy compression, screen recording, and re-encoding. Always embedded for authenticated users (forensic, not premium-gated).
  - **MP4 Metadata**: XOR-encrypted account info in the comment field of both individual segments and stitched output. Extractable via `ffprobe`.
- **Dual RTMP Streaming**: Simultaneous multi-destination streaming
- **Legacy Recording**: Direct-to-disk monolithic MP4

### YouTube Live Streaming

Native YouTube Live integration via compeller.ai OAuth:

- **Server-Side OAuth**: Secure token management through compeller.ai proxy
  - Browser-based authentication flow
  - Automatic token refresh (5-minute margin)
  - Channel info display (name, thumbnail)
- **Eligibility Checking**: 24-hour cached verification of streaming permissions
  - Channel verification status
  - Live streaming enabled check
  - Regional restriction detection
- **Session State Machine**: Robust lifecycle management
  - States: `Idle` → `Creating` → `Bound` → `Connecting` → `Pushing` → `Testing` → `Live` → `Ending` → `Ended`
  - Hard latches prevent accidental repetition (3s GoLive cooldown, 5s transition cooldown)
  - Minimum state hold times for stability
- **Quality Presets**:
  | Preset | Resolution | FPS | Video Bitrate | Audio |
  |--------|------------|-----|---------------|-------|
  | HD 60 | 1920x1080 | 60 | 6 Mbps | 160kbps AAC-LC |
  | 2K 60 | 2560x1440 | 60 | 24 Mbps | 160kbps AAC-LC |
  | 4K 60 | 3840x2160 | 60 | 35 Mbps | 160kbps AAC-LC |
- **Privacy Settings**: Public, Unlisted, or Private broadcasts
- **Retry Logic**: Exponential backoff for API (2-15s, 3 attempts) and RTMP (2-30s, 10 attempts)
- **Real-Time Status**: Stream health monitoring with dropped frame tracking

### Twitch Streaming

Direct RTMP streaming to Twitch with intelligent server selection:

- **Stream Configuration**:
  - Stream key authentication
  - Custom stream title, category (default: "Music"), and tags
  - Tag management UI with add/remove functionality
- **Server Discovery**: Automatic latency-based server selection
  - 11 Twitch ingest servers (US regions: SFO, LAX, SEA, PHX, DEN, DFW, ORD, JFK, MIA, IAD, ATL)
  - TCP ping on port 1935 with 1500ms timeout
  - Results cached for 5 minutes, max age 1 hour
  - Database persistence for fast startup
- **Degradation Tracking**: Automatic failover on connection issues
  - Failed servers blacklisted for 10 minutes
  - Fallback to oldest degraded server if all unavailable
  - Ultimate fallback: `live.twitch.tv`
- **Output Modes**:
  | Mode | Resolution | FPS | Video Bitrate | Audio |
  |------|------------|-----|---------------|-------|
  | 720p30 | 1280x720 | 30 | 3 Mbps | 160kbps AAC-LC |
  | 720p60 | 1280x720 | 60 | 6 Mbps | 160kbps AAC-LC |
  | 1080p30 | 1920x1080 | 30 | 4.5 Mbps | 160kbps AAC-LC |
  | 1080p60 | 1920x1080 | 60 | 6 Mbps | 160kbps AAC-LC |
  | 1440p30 | 2560x1440 | 30 | 18 Mbps | 160kbps AAC-LC |
  | 1440p60 | 2560x1440 | 60 | 24 Mbps | 160kbps AAC-LC |
  | 2160p30 | 3840x2160 | 30 | 25 Mbps | 160kbps AAC-LC |
  | 2160p60 | 3840x2160 | 60 | 35 Mbps | 160kbps AAC-LC |
- **Stream Statistics**: Duration timer, connection status display

### Streaming Architecture

The streaming system supports both GStreamer and FFmpeg encoding backends:

- **GStreamer Pipeline**: Native pipeline with proper A/V synchronization
  - `appsrc` for push-based video/audio input
  - `vtenc_h264` hardware encoder (VideoToolbox on macOS)
  - `fdkaacenc` for 160kbps AAC-LC audio at 48kHz stereo
  - `flvmux` for RTMP container format (100ms latency)
- **FFmpeg Pipeline**: Software encoding with advanced features
  - `libx264` high profile with B-frames=3 and rc-lookahead=20
  - CBR with 2-second VBV buffer
  - Optimized for reduced latency
- **Lock-Free Frame Delivery (FrameRing)**: Ring buffer architecture
  - 5 pre-allocated frame slots (up to 33MB each for 4K)
  - Atomic index updates, no mutex on hot path
  - 4-frame tolerance (~66ms at 60fps) before overwrite
- **Audio Ring Buffer (AudioRing)**: Dedicated audio delivery
  - 32 slots × 960 samples = 20ms per slot at 48kHz
  - Interleaved stereo float32 format
  - Monotonic PTS timestamps for sync
- **Backlog Management**:
  - 5-second threshold for P-frame dropping (never keyframes)
  - Reconnect trigger on sustained backlog
  - Exponential backoff (2-60s base delay)
  - Maximum 10 reconnection attempts

### Cloud Sync

- **Compel Sync**: Download video content from Compeller.ai
  - 256KB chunk size, max 5 retries per chunk
  - Per-video pause/resume/cancel
- **Set Sync**: Synchronize presets, compels, and settings
- **Resume Support**: Interrupted downloads continue from where they left off
- **Database-Backed State**: Sync progress persists across app restarts

### Licensing and Entitlements

Value-driven entitlements derived from HMAC-SHA256 signed license blobs fetched from compeller.ai:

- **License Blob**: Server signs plan limits as concrete values (render_scale, max_fps, watermark flag, etc.)
- **Offline Grace**: Blob cached in SQLite; valid for 7 days without server refresh
- **Derivation Over Gating**: Values used directly in computations (resolution multiplier, shader uniform), not boolean checks — patching a single gate won't help
- **Entitlements**:
  | Field | FREE | STARTER+ |
  |-------|------|----------|
  | `account_id` | 0 (anon) | Account ID (for fingerprinting) |
  | `render_scale` | 0.5 (540p cap) | 0.75 - 1.0 |
  | `max_fps` | 30 | 60 |
  | `watermark` | true | false |
  | `max_record_minutes` | 5 | 0 (unlimited) |
  | `max_streams` | 0 | 1+ |
  | `lighting_enabled` | false | true |
  | `youtube_enabled` | false | true |

### Recording Fingerprint Pipeline

Three-layer forensic tracing baked into recordings before files leave the user's machine:

1. **Visible Logo Watermark** — Compeller.ai logo PNG at 20% opacity in bottom-right corner
   - Implemented as FFmpeg `-filter_complex` overlay on the video encoding step
   - Logo written to temp file from embedded PNG, auto-cleaned on recording stop
   - Controlled by `entitlements.watermark` (FREE = watermark on, STARTER+ = off)

2. **Invisible Audio Fingerprint** — DSSS/BPSK at 19kHz carrier
   - Encodes 64-bit payload: 32-bit `account_id` + 32-bit Unix timestamp
   - Gold code spreading (degree-5 LFSR, 31 chips/bit) provides ~15dB processing gain
   - 300 chips/sec = ~9.7 bits/sec = full payload every ~6.6 seconds
   - -40dB amplitude (inaudible), survives lossy compression and re-encoding
   - Injected in `spawn_output_writer()` after the limiter stage, recording outputs only
   - Always embedded when `account_id > 0` (forensic tracing, not premium-gated)

3. **MP4 Metadata Comment** — XOR-encrypted `cai:{account_id}:{timestamp}` in base64
   - Embedded in both individual segment and stitched output metadata
   - Easy to extract via `ffprobe -show_format`, easy to strip — catches casual redistribution
   - Always embedded when `account_id > 0`

**Server-Side Decoding** (future): Bandpass filter 18-20kHz, correlate with Gold code, BPSK demodulate to recover account_id and timestamp. Multiple payload repetitions enable majority voting for error correction.

### Track Integration

- **Apple Music**: Automatic track detection with artist/title display (macOS)
- **Pro DJ Link (CDJ)**: Native Rust integration with Pioneer/AlphaTheta CDJs via UDP/TCP
  - Real-time status: playing, looping, paused, master, on-air
  - Track metadata fetching (title, artist, BPM)
  - CDJ takes priority over Apple Music when master deck is playing
  - Device records persisted to database
- **Track Announcements**: On-screen display of currently playing track

### MIDI Controller Support

- **Device Auto-Detection**: Automatically connects to all available MIDI devices
- **MIDI Learn**: Map any MIDI control to app parameters
  - Select target parameter, move a control, mapping saved
  - Supports CC, Note On/Off, and Pitch Bend messages (ParsedMidiMessage types)
- **Preset Bindings**: Map MIDI notes directly to specific presets
  - Device-specific or any-device bindings
  - Channel matching with wildcard support
- **Mappable Controls**:
  - Audio Gain, Kick Sensitivity
  - Theme/Video Visibility and Scale
  - Logo Opacity and Scale
  - Random Preset, Next/Prev Preset triggers
  - Toggle Theme/Video visibility
  - Mythos: Instability, Field Strength, Disturbance, Harmonic, Decay
- **Persistent Mappings**: MIDI mappings saved to database and restored on startup
- **Non-blocking**: MIDI processing runs on separate threads

### OSC Support

Open Sound Control output for professional lighting systems (Maestro, QLC+, GrandMA compatible).

#### 3-Layer Cue Engine

The CueEngine implements a hierarchical state machine for intelligent lighting control:

**Layer 1: Palette** - Color mood based on spectral balance
- `ICE`: balance > 0.7, sparse/ambient content
- `COOL_A`: balance 0.4-0.7, moderate density
- `COOL_B`: balance < 0.4, dense/saturated content

**Layer 2: EnergyZone** - 5-state intensity control
| Zone | Energy Range | Description |
|------|--------------|-------------|
| SILENT | raw_energy < 0.0003 | True audio absence (2s hold) |
| STILL | energy < 0.15 | Ambient noise/silence |
| LOW | energy 0.15-0.40 | Warm, subtle lighting |
| MID | energy 0.40-0.70 | Energetic, dance-ready |
| HIGH | energy > 0.70 | Intense, party mode |

**Layer 3: Accents** - Reactive triggers
- **Strobe**: Kick-triggered with cooldown, budget (max 8 per 30s), stability gate
- **Bass Pump**: 2× bass intensity brightness boost on kicks
- **Drop Impact**: 40% brightness boost + 70% floor on detected drops
- **Dominant Flash**: Pitch class change accents
- **Polarity Flash**: Quadrant transitions (E↔W, N↔S)
- **Blackout**: Configurable delay after silence

#### Macro State Machine

Buildup → Pump → Cool → Normal cycle for phrase-aware lighting:
- **Pump (OverdriveStrobe)**: 1.5× brightness, aggressive strobing
- **Cool (Recover)**: 85% brightness, reduced effects
- **Normal**: Energy-based floor (40% + 20% × energy)

#### Target Profiles

| Profile | Address Namespace | Notes |
|---------|-------------------|-------|
| Generic | `/react/*` | User maps in DMX software |
| MaestroDMX | `/live/1/*` | Native Maestro protocol |
| QLC+ | `/qlc/*` | QLC+ native commands |
| Custom | `/react/*` | Uses trigger_mappings |

#### OSC Message Types

**Continuous Values** (Throttled):
- Tier 1 (20Hz): `/react/brightness`, `/react/env/strobe`, `/react/env/dominant`, `/react/env/bass_pump`, `/react/env/drop_impact`
- Tier 2 (10Hz): `/react/energy`, `/react/hue`, `/react/saturation`, `/react/balance`, `/react/tension`, `/react/stability`
- Tier 3 (5Hz): `/debug/*` (normalizer diagnostics)
- Mythos (10Hz): `/mythos/heat`, `/mythos/phase`, `/mythos/intensity`, `/mythos/value`, `/mythos/breath`, `/mythos/act`

**State Messages** (5Hz):
- `/state/harmonic`: 0=Stable, 1=Rotating, 2=Ambiguous, 3=Atonal
- `/state/key`: 0-11 (C through B)
- `/state/mode`: 0=minor, 1=major

**Mover/FX Messages**:
- `/react/mover/speed`: Fixture movement (tension-scaled)
- `/react/fx/intensity`: Effect intensity (0-1)
- `/react/fx/index`: Effect palette index (0-3)

#### Genre Profiles

45+ music genre profiles with tuned lighting behavior:

**Categories:**
- **Electronic** (30 variants): House, Deep House, Tech House, Progressive House, Techno, Minimal Techno, Melodic Techno, Industrial Techno, Trance, Progressive Trance, Psytrance, Goa Trance, Drum & Bass, Dubstep, Bassline, UK Garage, Jungle, Hardstyle, Hardcore, Gabber, Breakbeat, EDM, Future Bass, Future Rave, Electro, Synthwave, Electro Pop, Vaporwave, IDM, Glitch, Ambient, Chillout
- **Rock** (4 variants): Alternative, Metal, Indie, Classic Rock
- **Instrumental** (4 variants): Classical, Jazz, Soundtrack/Cinematic, Lo-Fi/Chill
- **Pop** (3 variants): Pop/Top 40, Hip Hop/R&B, Latin
- **Live/Other** (3 variants): Live Band, DJ Set (Mixed), Custom

**Profile Parameters:**
- `zone_thresholds`: Energy breakpoints for STILL/LOW/MID/HIGH
- `palette_bias`: Cool, Warm, Neutral, or Vivid
- `strobe_enabled`, `strobe_sensitivity`, `strobe_zone_minimum`
- `pattern_change_rate`: Zone response speed
- `blackout_on_silence`, `blackout_delay_ms`
- `mover_speed_range`, `mover_intensity`
- `enable_buildup_blinder`: Auto-blinder on drop detection

#### Reactivity Control

Master reactivity slider (0.0-1.0) controls:
- Brightness floor scaling
- Audio drive gain
- Macro threshold sensitivity
- Higher = brighter base, punchier bass, easier macro triggers

### Philips Hue Integration

Low-latency direct RGB control of Philips Hue lights via the Entertainment API:

- **DTLS/UDP Streaming**: Encrypted entertainment streaming at 25fps with OpenSSL DTLS 1.2
  - PSK-based authentication (`TLS_PSK_WITH_AES_128_GCM_SHA256`)
  - 16-bit RGB per light, up to 10 lights per entertainment area
  - Background sender thread with non-blocking channel
- **Bridge Discovery & Pairing**: Push-link pairing flow with status feedback
- **Entertainment Areas**: Select and control configured Hue entertainment areas
- **Light Role Assignment**: Each light assigned a role controlling its behavior:
  - **Wash** (250ms τ): Smooth energy tracking, subtle breathing sine wave
  - **Accent** (150ms τ): Punchy, reactive to triggers (strobe bloom, pitch color, polarity flash)
  - **Ambient** (500ms τ): Slow atmospheric breathing, never fully off
- **Tonality-Aware Color**: Major mode → warm (red/amber), Minor → cool (blue/cyan)
- **Dominant Pitch Hue Rotation**: 12-pitch chromatic wheel color blending based on tension
- **Spatial Polarity Quadrants**: Directional white flashes routed to lights by index
- **Frame-Rate Independent Smoothing**: Exponential interpolation `alpha = 1 - exp(-dt/tau)`
- **Persistent Configuration**: Bridge IP, credentials, area, and light assignments saved to database

### WLED Integration

UDP streaming to WLED LED strip controllers using the DRGB protocol:

- **Three Visualization Modes**:
  - **Spectrum**: Center-mirrored frequency band timeline (R=bass, G=mid, B=high), scrolls outward
  - **Beat**: Spatial envelope spread from center — kick (red), dominant pitch change (pitch-colored), polarity flash (white)
  - **Pulse**: Whole-strip energy wash with palette + tonality + chroma-weighted harmonic color
- **DRGB Protocol**: Simple UDP packets (mode byte + timeout + RGB per LED), no authentication
- **Per-Strip Configuration**: IP address, LED count, UDP port, RGBW support, visualization mode
- **Device Discovery**: HTTP JSON API (`/json/info`) for device info
- **Force-Saturated Colors**: HSV saturation forced to 1.0 for vivid LED output
- **Background Sender Thread**: Non-blocking at ~50fps recommended rate
- **Persistent Configuration**: Strip settings saved to database as JSON

### Unified Lighting Device Types

Shared types for cross-backend device management:

- **FormFactor**: Bulb, Spot, Strip, PlayBar, Unknown
- **DeviceBackend**: Hue, Wled
- **DeviceCapabilities**: `supports_segments`, `channel_count`

### Camera System

- **Multi-Camera Face Detection**: Multiple camera input with real-time face detection
- **Tracking Mix Modes**: Configurable blending between camera sources
- **Per-Camera Influence/Confidence**: Weighted contribution from each camera
- **Shared Texture Management**: Efficient GPU texture sharing across cameras
- **Quality Tiers**: Adjustable detection quality for performance tuning
- **Fullscreen Camera Layer**: Live camera composited between theme and video overlay on all themes
- **Camera Luma Key**: Dark camera areas become transparent to reveal theme beneath (0.0–1.0 slider)
- **Momentary Solo**: MIDI note hold solos a camera, release restores previous state
- **MIDI Control**: Per-camera visibility, enable/disable, luma key, and solo all MIDI-mappable

### State Buffer & Scrub

- **Ring Buffer**: 300-600 frames (5-10 seconds at 60fps)
- **Freeze/Pause State**: Halt visualization for inspection
- **Scrub Position**: Navigate buffer (0.0-1.0)
- **Audio Scrub Playback**: Speed and direction adaptive audio
- **Video Frame History**: Synchronized video frame recall

## Architecture

```
react/
├── src/
│   ├── main.rs              # Entry point, single-instance lock
│   ├── app.rs               # Main app state & render loop (325KB)
│   ├── config.rs            # Configuration management
│   ├── db.rs                # SQLite persistence (141KB)
│   ├── sync.rs              # Cloud video sync with resume
│   ├── set_sync.rs          # Set synchronization
│   ├── compel_picker.rs     # Compel browser with fuzzy search
│   ├── compel_matcher.rs    # Auto-match compels to tracks
│   ├── lighting.rs          # Shared lighting device types
│   │
│   ├── audio/
│   │   ├── mod.rs           # Module exports
│   │   ├── capture.rs       # ScreenCaptureKit + cpal capture
│   │   ├── gst_capture.rs   # GStreamer multi-channel USB audio
│   │   ├── analyzer.rs      # FFT, beat detection, chroma, key
│   │   ├── meters.rs        # fundsp band-split metering
│   │   ├── normalizer.rs    # Adaptive signal normalization
│   │   ├── audio_bus.rs     # Lock-free SPSC fanout (5 consumers)
│   │   └── output.rs        # Audio playback for scrub
│   │
│   ├── render/
│   │   ├── mod.rs           # State buffer, overlays, LightingMetrics
│   │   ├── pipeline.rs      # OpenGL FBO pipeline, PBO uploads (117KB)
│   │   ├── compositor.rs    # Bloom, chromatic aberration, grain, vignette
│   │   ├── projectm_renderer.rs  # MilkDrop integration
│   │   ├── registry.rs      # Unified preset registry (favorites, ratings)
│   │   ├── theme.rs         # Renderer trait (ProjectM, LED, HG, Mythos, Sonic)
│   │   ├── led_wall.rs      # LED panel simulation (7 patterns)
│   │   ├── led_wall_metal.rs     # Metal compute LED wall
│   │   │
│   │   ├── harmonic_geometry/
│   │   │   ├── mod.rs            # State machine, REG, topology router
│   │   │   ├── renderer.rs       # Shader pipeline, HarmonicFieldState
│   │   │   ├── harmonic_field.rs  # Chroma → geometry mapping
│   │   │   ├── key_detector.rs    # Key detection, harmonic labels
│   │   │   ├── quaternion_charges.rs  # QC ports, REG degree, lattice
│   │   │   ├── skin.rs           # Video skin (surface/emissive/combined)
│   │   │   ├── presets.rs         # Hero Journey configuration
│   │   │   ├── audio_mapper.rs    # Audio → visual force mapping
│   │   │   ├── gate_system.rs     # 12/24 harmonic gate, magnetic snap
│   │   │   ├── harmonic_states.rs # Topology intent states
│   │   │   └── shaders/          # GLSL shader modules
│   │   │
│   │   └── mythos/
│   │       └── mod.rs            # Continuous field engine: 3-layer Collatz state machine,
│   │                             # 5 presets, 5 palettes, act system, video skin,
│   │                             # quality-tier FBM scaling, CollatzOutput for OSC/DMX
│   │
│   ├── output/
│   │   ├── mod.rs                # Output orchestration, gain model, watermark lifecycle
│   │   ├── audio_fingerprint.rs  # DSSS/BPSK ultrasonic fingerprint (19kHz)
│   │   ├── ndi_sender.rs         # NDI (HD/4K/Ultrawide, audio, tally)
│   │   ├── native_window.rs      # Fullscreen projection window
│   │   ├── recorder.rs           # Legacy monolithic MP4
│   │   ├── segmented_recorder.rs # Crash-resilient segmented recording
│   │   ├── segment_journal.rs    # Recording state journal
│   │   ├── segment_stitcher.rs   # Post-recording segment concat
│   │   ├── fmp4_muxer.rs         # Fragmented MP4 muxer (+ watermark overlay)
│   │   ├── videotoolbox_encoder.rs # Hardware H.264/H.265 (macOS)
│   │   ├── rtmp_streamer.rs      # RTMP streaming core
│   │   ├── ffmpeg_streamer.rs    # FFmpeg encoding (8 OutputMode presets)
│   │   ├── gstreamer_streamer.rs # GStreamer RTMP pipeline
│   │   ├── dual_rtmp_streamer.rs # Simultaneous multi-destination RTMP
│   │   └── twitch_servers.rs     # Twitch server discovery & failover
│   │
│   ├── youtube/
│   │   ├── mod.rs           # Timing constants, enums
│   │   ├── auth.rs          # OAuth via compeller.ai proxy
│   │   ├── session.rs       # Session state machine
│   │   ├── control_plane.rs # YouTube API (create/bind/transition)
│   │   └── eligibility.rs   # 24-hour cached eligibility
│   │
│   ├── video/
│   │   ├── mod.rs           # Video module coordination
│   │   ├── decoder.rs       # Async ring-buffer decoding
│   │   ├── avfoundation.rs  # Hardware decode (macOS)
│   │   ├── texture_cache.rs # CVOpenGLTexture caching
│   │   └── thumbnail.rs     # Background thumbnail generation
│   │
│   ├── track/
│   │   ├── mod.rs           # Track module coordination
│   │   ├── apple_music.rs   # Apple Music integration
│   │   └── prodjlink.rs     # Pioneer CDJ via UDP/TCP
│   │
│   ├── camera/
│   │   ├── mod.rs           # Camera module
│   │   ├── capture.rs       # Video capture
│   │   ├── detector.rs      # Face detection
│   │   ├── tracker.rs       # Tracking algorithms
│   │   └── state.rs         # Camera state machine
│   │
│   ├── midi/
│   │   ├── mod.rs           # Device detection & events
│   │   ├── mapping.rs       # CC/Note parameter mapping
│   │   ├── learn.rs         # MIDI learn mode
│   │   └── preset_bindings.rs # Note → preset bindings
│   │
│   ├── osc/
│   │   ├── mod.rs           # OscManager coordinator
│   │   ├── sender.rs        # UDP OSC output
│   │   ├── thread.rs        # Background thread management
│   │   ├── mappings.rs      # OscConfig, TargetProfile, OscMapping
│   │   ├── cue_engine.rs    # 3-layer cue state machine with envelopes
│   │   ├── genre.rs         # GenreProfile, PaletteBias, ZoneThresholds
│   │   ├── key_signal.rs    # Fallback harmonic signal processor
│   │   ├── triggers.rs      # Manual trigger buttons
│   │   └── maestro_api.rs   # MaestroDMX REST client
│   │
│   ├── hue/
│   │   ├── mod.rs           # HueManager coordinator
│   │   ├── bridge.rs        # Bridge discovery, pairing, REST API
│   │   ├── streaming.rs     # DTLS 1.2/UDP entertainment streaming
│   │   ├── mappings.rs      # CueEngine → RGB color translation
│   │   └── thread.rs        # Background update thread
│   │
│   ├── wled/
│   │   ├── mod.rs           # WledManager coordinator
│   │   ├── streaming.rs     # UDP DRGB packet sender
│   │   └── mappings.rs      # Audio → LED visualizations
│   │
│   ├── auth/
│   │   ├── mod.rs           # Auth state machine, Entitlements (account_id, watermark, plan)
│   │   ├── license.rs       # HMAC-signed license blob verification + offline cache
│   │   └── connected.rs     # Connected account state (YouTube, etc.)
│   │
│   └── ui/
│       ├── mod.rs           # UI module coordination
│       ├── controls.rs      # egui control panel (150KB+)
│       └── floating_logos.rs # Logo/track overlay rendering
│
├── presets/                 # MilkDrop visualization presets
├── assets/                  # Texture atlases, fixture profiles
├── models/                  # Vision model export scripts (weights are not tracked)
├── public/                  # Static assets (fonts, images, 3D models)
├── libs/                    # projectM 4.x dylibs
├── deps/projectm-sys/       # Patched projectm-sys crate
├── ndi-sdk/                 # NDI SDK (not tracked — install locally)
├── docs/                    # Smoke-test and fixture-profile notes
├── scripts/                 # Shared build env, model export, tooling
├── tools/                   # Standalone hardware bench tools
├── stream-smoke/            # Stream overlay smoke-test server
├── Cargo.toml / build.rs    # Crate manifest and build script
├── build.sh                 # Release build: bundle, sign, notarize, DMG
└── start-react.sh           # Build and run from source
```

## Harmonic Geometry Engine

The harmonic geometry visualization system is a sophisticated audio-reactive renderer based on musical theory and quaternion mathematics. It generates torus knot visualizations that respond to music.

### Musical Analysis

- **Key Detection**: Real-time detection with TuningReference (440Hz, 432Hz)
- **Harmonic Labels**: Stable, Rotating, Ambiguous, Atonal
- **12-Bin Chroma**: FIFO history (~2.5s window)
- **Dominant Pitch Latching**: 0.25s hold, 15% threshold
- **Anchor Quantization**: `anchor_k` (0..N-1) → `anchor_theta_deg` (always on 30°/15° grid)
- **REG Degree A**: Harmonic integer (1-360), INFO telemetry, not rotation authority
- **Phrase Detection**: Awareness of musical phrases for density envelope modulation
- **Polarity Tracking**: Quadrant-based polarity detection with hysteresis

### Visual Elements

- **Torus Knot**: Primary geometric form with configurable p/q winding ratios
- **Spice System**: "Exotic" harmonic ratios (7-limit, 11-limit intervals) revealed through gating
- **Ornament Layers**: Filigree, notches, and halos that respond to harmonic content
- **Density Envelopes**: Phrase-aware modulation of visual complexity
- **Warp Jump Effects**: Scale animations (shrink, pop, jiggle) during phase transitions
- **Ring Dive**: 4-phase curve (lock_on → warp_out → guided_flight → warp_back) with 3 quality levels
- **Portal FPS**: Rail shooter inside portal dive with lane system

### Dynamic Effects

- **Polarity Flash/Smoke**: Visual effects triggered on polarity changes
- **Camera Nudge**: Yaw offset (±7.5°) based on polarity direction
- **Orbit Direction**: Flip based on polarity changes with smooth transitions
- **Beat-Driven Updates**: Camera orbit, tube radius, vortex strength synchronized to beats
- **Camera Rotation**: Driven by anchor_k only (quantized angular position)

### Tuning Specification

For detailed implementation of the density pipeline, spice gating system, and ornament layers, see:
`src/render/harmonic_geometry/TORUS_ICONIC_TUNING.md`

This spec covers:
- Phrase-aware density envelopes with beat-driven breathing
- Gated spice reveals for 7-limit and 11-limit intervals (cinematic moments)
- Ornament layer system (Radial Filigree, Harmonic Notches, Knot Halo)
- WI sector accents with onset-triggered highlights
- Revised bead tables and complexity bucket thresholds

### Topology System (24-fold)

The harmonic geometry supports multiple topology states that form a progression of visual complexity. The system uses a **Topology Router** to deterministically switch between topologies at bar boundaries based on audio characteristics.

#### Topology Identifiers

| ID | Topology | Description |
|----|----------|-------------|
| 0 | **Scaffold24** | Base 24-fold scaffold, entry point for all engagement |
| 1 | **TorusKnotCymatics** | Torus knot with cymatics-driven modulation |
| 2 | **Vortex** | Spiraling drain with gate-quantized circulation |
| 3 | **RadialInterferenceShell** | Shell-based interference patterns |
| 4 | **BraidedStandingWave** | Braided standing wave visualization |
| 5 | **AfterglowResidual** | Post-dive residual coherence |
| 6 | **DodecahedralResonance** | Crystalline 12-face arrival topology |

#### Topology Router

The router evaluates transitions **only at bar edges** (quantized to 4 beats) to prevent jarring mid-bar switches. Key features:

- **Bar-edge quantization**: Routing decisions happen at bar boundaries only
- **Dwell timers**: Conditions must hold for a minimum duration before triggering
- **Hysteresis**: Different entry/exit thresholds prevent oscillation
- **Crossfade blending**: 1.2-2.0s alpha blend between stable projections (not morphing)
- **Cooldown**: 1.0s post-transition lockout prevents rapid switching

#### Routing Predicates

Routes are evaluated based on bar-averaged audio metrics:

| Route | From → To | Condition |
|-------|-----------|-----------|
| 0 | Scaffold → TorusKnot | High energy + harmony, NOT mid-dominant |
| 1 | TorusKnot → Scaffold | Low energy retreat |
| 2 | Scaffold → Vortex | Mid-dominant + moderate conditions |
| 3 | TorusKnot → Vortex | Mid-dominant, highs calm, tension falling |
| 4 | Vortex → RadialShell | High-dominant + busy onsets |
| 5 | Vortex → Scaffold | Retreat conditions |
| 6 | Vortex → BraidedWave | Very stable harmony |
| 8 | Vortex → TorusKnot | Re-escalation (high energy) |

#### Fold24 Permission vs Engagement Model

The 24-fold geometry system uses a dual-path engagement model to solve the "chicken-and-egg" problem where topologies require fold24.active but fold24 requires certain topologies:

1. **Permission** (`permitted`): Whether 24-fold features are allowed (energy threshold met)
2. **Engagement** (`engaged`): Whether 24-fold is actively rendering

Engagement can be triggered via two paths:
- **RingDive path**: Traditional engagement through dive progression
- **TopologyRequest path**: Router requests a 24-fold topology (Vortex, etc.)

When engagement begins, the system enters an **Entry Funnel** (2 bars) that:
- Forces Scaffold24 as the initial topology
- Blocks topology switching during the funnel
- Stores the requested topology as preference
- After funnel ends, preferred topology gets routing priority

**Tri-State Lattice**: N/A (QC off) | true (on 15° grid) | VIOLATION (off-grid)

#### VortexState

The Vortex topology creates a spiraling drain effect with these parameters:

| Parameter | Description |
|-----------|-------------|
| `gate_center` | 24-fold quantized gate position (0-23), inherited on entry |
| `circulation_step` | ±1 gate step per beat maximum |
| `direction` | Inward or Outward spiral |
| `pressure_env` | Envelope for visual intensity (attack/decay) |
| `pitch_q` | 24-fold quantized pitch from dominant chroma |
| `shell_set_id` | Shell pattern selection based on stability bands |

**Gate Confidence**: When stability drops below 0.25 (AMBIGUOUS gate), circulation stepping freezes and pressure envelope decays only.

### Portal FPS

Rail shooter experience inside portal dive:

- **4 Path Types**: Corridor, TunnelGrid, RibLanes, KnotPass
- **8-Segment Sequence** (~2.8s): Runway → Accelerate → Threat → Vista → Threat → Needle → Decompress → Exit
- **Lane System**: -2 to +2, near-miss events, threat debt fallback
- **REG-Pure Quantization**: All angular values locked to grid

### Ring Dive

4-phase immersive dive into the geometry:

- **Phases**: lock_on → warp_out → guided_flight → warp_back
- **3 Quality Levels**: Low, Medium, High
- **Streaks**: Aligned to dominant pitch class

### REG Camera System

Quantized camera motion driven by harmonic analysis:

- **CameraRig Modes**: Orbit, AxialPush, PortalDive
- **Quantized Motion**: anchor_k × (TAU/N) targets only
- **Beat-Gated Commits**: ±1 step limit per beat
- **Portal Lock**: Anchor frozen during portal dive
- **Silence Freeze**: Anchor frozen when energy < threshold
- **Hold/Cooldown Timers**: Prevent rapid switching

### REG Invariant Contract

| INV | Rule | Description |
|-----|------|-------------|
| 1 | Discrete range | anchor_k ∈ [0, N-1] |
| 2 | Step limit | ±1 per beat commit |
| 3 | Beat gating | Commits only on beat crossings |
| 4 | Portal lock | Anchor frozen during portal dive |
| 5 | Freeze rule | Anchor frozen when energy < threshold |
| 6 | Unit contract | All fields in documented ranges |

Plus 2 `debug_assert!` guards ensuring render rotation uses `anchor_theta` only.

### MIDI Energy Mapping

Energy thresholds are tuned for 13-notch MIDI controllers:

| MIDI Range | Energy | Visual State |
|------------|--------|--------------|
| Notch 0 | 0% | Silent/none |
| Notch 1 | ~8% | Birth emerges |
| Notches 2-5 | 12-38% | 12-fold geometry |
| Notches 6-8 | 40-57% | 24-fold engagement (Scaffold, Vortex) |
| Notches 8-13 | 58%+ | TorusKnot + advanced topologies |

Key thresholds:
- `energy_enter_thresh`: 0.12 (notch 2) - 12-fold entry
- `E_ENTER`: 0.40 (notch 6) - 24-fold permission
- `TorusKnot energy_thresh`: 0.58 (notch 8) - TorusKnot routing

### Reference Materials
This system is informed by research (not included in this repository) on:
- Mathematical foundations of harmonic relationships
- Quaternion symmetry and icosietragon geometry
- Wave theory and cymatics
- Fractal number theory

## Audio Analysis Details

### FFT Processing
- **Window**: 8192-point, Hann window
- **Resolution**: 5.9 Hz per bin at 48kHz

### Frequency Bands
- **Bass**: 20-400Hz
- **Mid**: 300-1200Hz
- **High**: 1400-20kHz

### Kick Detection (4-stage)
1. Ratio gate (bass vs overall energy)
2. Envelope tracking
3. Rise detection (positive slope threshold)
4. Refractory period (80ms minimum between triggers)

### Chroma Analysis
- 12-bin folding across octaves
- Parabolic interpolation for precision
- Octave damping for stability

### Tonal State Detection
- Hysteresis thresholds: enter >0.25 for 3 frames, exit <0.12 for 8 frames

### Band Centroids
6 geometric bands: Sub, Bass, Low-mid, Mid, High-mid, Air

### Dual Smoothing
- **FAST**: UI and shader reactive parameters (~60ms)
- **SLOW**: LED and sustained visual effects (stable bands)

### LightingMetrics Structure

Core audio-reactive signals for OSC/Hue/WLED:
```
energy        - Zone driver (0-1)
stability     - Strobe permission gate (0-1)
balance       - Palette selector (0-1)
tension       - Contrast/intensity (0-1)
beat_confidence - Beat detection confidence (0-1)
onset         - Transient detected (bool)
kick          - Kick detected (bool)
dominant_changed - Pitch class changed (bool)
dominant_pc   - Current pitch class (0-11)
polarity_quadrant - Harmonic quadrant (0-3)
polarity_changed - Quadrant changed (bool)
major_likelihood - Mode (0=minor, 1=major)
bass/mid/high - Frequency bands (0-1)
raw_energy    - Pre-normalization energy for silence detection
```

## Building

### Prerequisites

- **Rust** (stable — selected by `rust-toolchain.toml`)
- **cmake** and **pkg-config** (projectM and native bindings)
- **FFmpeg** (for video decoding)
- **GStreamer** (for streaming pipelines): `brew install gstreamer` (the formula includes the plugin sets)
- **NDI SDK** (optional, for NDI output) — proprietary and not part of this
  repository. Place it at `./ndi-sdk` so that `ndi-sdk/lib/libndi.dylib` exists,
  or point `NDI_SDK_DIR` at your copy. Without it, NDI output is unavailable.

### Development

```bash
# Build and run (sets up library paths, then `cargo run --release`)
./start-react.sh
```

### macOS release build

```bash
# Ad-hoc signed .app, no private settings required
./build.sh --dev --no-license-key

# Signed, notarized .app + DMG (needs .env.local)
./build.sh
```

### Private settings

Signing identity, notarization credentials, the license verification key and
the development API host are never stored in this repository. They are read
from an untracked `.env.local` — copy `.env.example` and fill it in.

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `M` | Toggle menu panel |
| `Space` | Toggle freeze (harmonic state scrubbing) |
| `N` | Toggle NDI output |
| `O` | Toggle output window |
| `F` | Toggle fullscreen (output window) |
| `H` | Toggle HUD overlay |

## Configuration

Settings are persisted in a SQLite database at:
- **macOS**: `~/Library/Application Support/compeller/react.db`

### Stored Settings
- Audio gain level and source
- Video visibility and silence threshold
- Menu visibility state
- Selected preset
- Video/recording directories
- Sync progress
- MIDI mappings and preset bindings
- Quality tier and background quality
- Auto-scaling preferences
- QS wheel mode and math HUD
- Render mode selection
- Twitch settings (channel, stream key, title, category, tags, output mode)
- YouTube settings (stream title, quality preset, privacy)
- Hue settings (bridge IP, credentials, entertainment area, light role assignments)
- WLED settings (strip IPs, LED counts, visualization modes)
- OSC settings (target profile, genre profile, reactivity, feature toggles)

## Database Schema

Key tables:
- `settings`: Key-value app configuration
- `app_state`: Persisted UI state (gain, visibility, quality tier, render mode, etc.)
- `favorites`: Favorited presets
- `video_metadata`: Video file metadata and favorites
  - `thumbnail_path`: Path to cached thumbnail PNG
  - `thumbnail_generated`: Whether thumbnail has been created
- `sync_videos`: Cloud sync state with resume support
- `set_uploads`: Set synchronization tracking
- `recordings`: Recording metadata
- `segmented_recordings`: Segmented recording sessions
- `segment_journal`: Journal entries for crash recovery
- `tracks`: Track information for videos
- `midi_mappings`: MIDI controller mappings (device, channel, CC/note -> target)
- `midi_preset_bindings`: MIDI note to preset mappings
- `cdj_devices`: Pioneer CDJ device records
- `twitch_ingest_servers`: Cached Twitch server ping results
  - `host`: Server hostname (primary key)
  - `name`: Display name (e.g., "San Francisco")
  - `latency_ms`: Last measured latency
  - `last_ping_time`: Unix timestamp of last ping
  - `degraded_until`: Unix timestamp when degradation expires

### Streaming Settings (in `settings` table)
- `twitch_channel`, `twitch_stream_key`, `twitch_stream_title`
- `twitch_stream_category`, `twitch_stream_tags`
- `twitch_output_mode`: Output mode enum (720p30, 1080p60, etc.)
- `youtube_stream_title`, `youtube_quality`, `youtube_privacy`

### Hue Settings (in `settings` table)
- `hue_enabled`: Enable/disable Hue integration
- `hue_bridge_ip`: Bridge IP address
- `hue_username`: API username from pairing
- `hue_clientkey`: Hex PSK for DTLS authentication
- `hue_area_id`: Selected entertainment area UUID
- `hue_light_assignments`: JSON array of light role assignments (`[{"channel_id": 1, "name": "...", "role": "Wash"}]`)

### WLED Settings (in `settings` table)
- `wled_enabled`: Enable/disable WLED integration
- `wled_strips`: JSON array of strip configs (`[{"ip": "...", "name": "...", "led_count": 30, "udp_port": 21324, "rgbw": false, "mode": "Spectrum", "enabled": true}]`)

### OSC Settings (in `settings` table)
- `osc_enabled`: Enable/disable OSC output
- `osc_target_ip`: Target IP address
- `osc_target_port`: Target port (default 7672)
- `osc_target_profile`: Generic, MaestroDMX, QLC+, or Custom
- `osc_genre_profile`: JSON serialized GenreProfile
- `osc_reactivity`: Master reactivity slider (0.0-1.0)
- `osc_enable_polarity_flash`, `osc_enable_dominant_accent`, etc.

## Troubleshooting

### NDI Not Working
- Ensure NDI Tools is installed on receiving devices
- Check firewall allows NDI traffic (port 5353 for discovery)
- Verify sender appears in NDI Studio Monitor
- For Ultrawide mode, ensure receiving device supports 5120x1440

### Audio Not Capturing
- Grant microphone permissions in System Preferences
- Select correct audio source in Audio section
- Check audio gain is not at zero
- For multi-channel USB devices, ensure GStreamer is installed
- Check device profile is recognized (see logs for "Using profile")

### Video Playback Issues
- Videos play on all render modes (ProjectM, LED Wall, Geometric Field)
- Check video format is H.264 MP4
- Verify file is not corrupted
- Thumbnails are cached in `~/Library/Caches/compeller/thumbnails/`

### Camera Issues
- Grant camera permissions in System Preferences
- Check camera is not exclusively claimed by another application
- Lower quality tier if detection is too slow

### Recording Recovery
- If app crashes during segmented recording, segments are preserved
- On next launch, check journal for uncommitted segments
- Use segment stitcher to recover partial recordings

### Dual Streaming Issues
- Both RTMP destinations must be configured before starting
- Check bandwidth supports combined bitrate of both streams
- Each stream operates independently (one can fail without affecting the other)

### Sync Issues
- Check internet connection
- Click "Refresh" to rescan for partial downloads
- Check `.part` files exist in video directory for resume

### MIDI Not Working
- Click the refresh button in MIDI Controllers section to rescan devices
- Ensure MIDI device is connected before starting the app
- Check device is not exclusively claimed by another application
- Mappings are stored per-device name; reconnecting a device restores mappings

### YouTube Streaming Issues
- **"Not logged into Compeller"**: Log into your Compeller account first
- **Eligibility errors**: Ensure your YouTube channel has live streaming enabled
  - Channel must be verified
  - No active strikes or restrictions
  - Wait 24 hours after enabling live streaming
- **Stream stuck in "Testing"**: YouTube takes time to verify stream health
  - Ensure stable RTMP connection
  - Check video/audio is being sent (frame counters incrementing)
- **RTMP connection failures**: Retry logic will attempt up to 10 reconnections
  - Check internet stability
  - YouTube ingest endpoints may be temporarily unavailable
- **"GoLive" button disabled**: 3-second cooldown after clicking, or session already active

### Twitch Streaming Issues
- **No stream appearing on Twitch**: Verify stream key is correct
  - Stream keys are found in Twitch Creator Dashboard → Settings → Stream
- **High latency/stuttering**:
  - Try a different output mode (lower resolution/framerate)
  - Check if current server is degraded (visible in server list)
  - Click refresh to re-ping servers and select a faster one
- **All servers showing high latency**:
  - Check your internet connection
  - Servers are pinged on TCP port 1935
- **Stream dropping frequently**:
  - Server will be auto-blacklisted for 10 minutes after failure
  - App will automatically try next best server
  - Check network stability

### Hue Integration Issues
- **Pairing fails**: Press the bridge button within 30 seconds, ensure bridge IP is correct
- **No entertainment areas**: Create an entertainment area in the Philips Hue app first
- **DTLS connection drops**: Check network stability; bridge must be on same LAN
- **Lights not responding**: Verify lights are assigned to the selected entertainment area
- **Dim or wrong colors**: Check light role assignments (Wash/Accent/Ambient)

### WLED Integration Issues
- **Strip not connecting**: Verify WLED device IP and UDP port (default 21324)
- **No LEDs lighting up**: Check LED count matches device configuration
- **Colors look washed out**: WLED devices may need brightness adjustment in WLED settings
- **High latency**: Ensure WLED device is on same network; check for WiFi congestion

### OSC/Lighting Issues
- **No messages being sent**: Check target IP and port, verify OSC is enabled
- **Wrong addresses**: Select correct Target Profile (Generic, MaestroDMX, QLC+)
- **Zones not changing**: Adjust genre profile thresholds or reactivity slider
- **Strobes not firing**: Check strobe_enabled in genre profile, verify zone meets minimum
- **Lighting too reactive/calm**: Adjust reactivity slider (0.0-1.0)
- **SILENT zone not triggering**: Requires 2s of true audio silence (raw_energy < 0.0003)

### Build Lock Issues
If you see "Blocking waiting for file lock on build directory":
```bash
# Kill stale cargo/rustc processes
pkill -9 -f "cargo.*--features camera"
pkill -9 -f "rustc.*compeller_react"
```

## Dependencies

### Core
- `eframe` / `egui` - Cross-platform GUI
- `glow` - OpenGL bindings
- `projectm` - MilkDrop visualization engine
- `metal` - Metal GPU compute (macOS)

### Audio
- `cpal` - Cross-platform audio I/O
- `rustfft` - FFT for frequency analysis
- `fundsp` - DSP library
- `rtrb` - Real-time safe SPSC ring buffer

### Platform (macOS)
- `screencapturekit` - System audio capture
- `cocoa` / `objc2` - macOS framework bindings
- `io-surface` - Zero-copy GPU texture sharing

### Integration
- `midir` / `wmidi` - MIDI support
- `rosc` - Open Sound Control
- NDI SDK - Network video streaming
- `openssl` / `openssl-sys` - DTLS 1.2 for Hue Entertainment API

### Streaming
- `gstreamer` / `gstreamer-app` / `gstreamer-video` / `gstreamer-audio` - GStreamer pipeline
- `ffmpeg-next` - FFmpeg libav* bindings for encoding
- `ureq` - HTTP client for YouTube API
- `zeroize` - Secure secret clearing

### Utilities
- `rusqlite` - SQLite database
- `tokio` - Async runtime
- `serde` - Serialization
- `crossbeam` - Concurrent data structures
- `fs2` - File locking (single instance)

## Contributing

Bug reports and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Copyright 2026 Compeller.ai. Created by Yakoob Ahmad to empower musicians and
venues.

Licensed under the [Apache License, Version 2.0](LICENSE). If you redistribute
REACT or a derivative of it, you must keep the attribution in [NOTICE](NOTICE).

The license does not grant rights to the REACT and Compeller names and logos
(Apache-2.0, Section 6).

Third-party components in this repository — projectM, the Milkdrop preset pack,
Roboto — keep their own licenses; see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
