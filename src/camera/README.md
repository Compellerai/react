# Multi-Camera Tracking System

## Overview

V1: Per-camera detection + tracking with operator-selected program camera
V2: Cross-camera identity using embeddings + auto camera selection

## Architecture

```
camera/
├── mod.rs              # Public API, CameraManager
├── capture.rs          # nokhwa webcam capture, frame ring buffers
├── detector.rs         # ONNX inference (RF-DETR, see models/README.md)
├── tracker.rs          # SORT-style per-camera tracking
├── state.rs            # Track, Detection, EntityState structs
└── integration.rs      # Bridge to harmonic geometry (TOI → modulation)
```

## Data Flow

```
[USB Cameras] → capture.rs (frame_ring per camera)
                    ↓
              detector.rs (10-15 fps, bounded)
                    ↓
              tracker.rs (IoU + Kalman, per-camera IDs)
                    ↓
              integration.rs (select TOI → quantize → latch)
                    ↓
              [AudioAnalysis extended with tracking data]
                    ↓
              harmonic_geometry (gate anchor, accent intensity)
```

## Quality Tier Scaling

| Tier   | Cameras | Det FPS | Det Res  | Tracks/Cam |
|--------|---------|---------|----------|------------|
| Low    | 1       | 10      | 320x240  | 8          |
| Medium | 2       | 12      | 480x360  | 12         |
| High   | 4       | 15      | 640x480  | 16         |

## Key Design Decisions

1. **Bounded everything**: Max cameras, max tracks, fixed ring buffers
2. **Non-blocking render**: Tracking runs on background thread
3. **Quantized output**: anchor_index = quantize_to_N(x, N=12/24)
4. **Hysteresis**: ±1 step max per change, hold + cooldown for switches
5. **Director intent**: One active entity drives viz, camera switches are rare