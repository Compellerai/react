# Third-party notices

REACT itself is licensed under the Apache License 2.0 (see `LICENSE` and
`NOTICE`). The components below are included in this repository, or linked
when it is built, and remain under their own licenses.

## Included in this repository

| Component | Location | License |
|-----------|----------|---------|
| projectM 4.1.6 (source) | `deps/projectm-sys/libprojectM/` | LGPL 2.1 — `deps/projectm-sys/libprojectM/LICENSE.txt` |
| projectM 4.1.6 (prebuilt macOS dynamic libraries) | `libs/` | LGPL 2.1 — built from the source above |
| `projectm-sys` Rust bindings (local fork of [projectm-rs](https://github.com/projectM-visualizer/projectm-rs)) | `deps/projectm-sys/` | LGPL — `deps/projectm-sys/LICENSE` |
| Milkdrop "Cream of the Crop" preset pack, curated by ISOSCELES, as distributed with projectM | `presets/` | See `presets/LICENSE.md` — individual presets were released by their authors without a formal license |
| Roboto Black, Copyright 2015 Google Inc. | `public/fonts/Roboto-Black.ttf` | Apache License 2.0 |
| DMX fixture definitions derived from ATLABASE fixture data | `assets/fixture-profiles/`, `assets/nas-fixture-profiles/`, `assets/fixture-capability-snapshots/` | Fixture data describes third-party hardware; trademarks belong to the respective manufacturers |

projectM is linked dynamically. You may replace the libraries in `libs/` with
your own build of the vendored source.

## Rust dependencies

Exact versions are pinned in `Cargo.lock`. To list every crate with its license:

```bash
cargo tree --features camera -e normal,build --prefix none --format '{p} {l}'
```

Most are MIT and/or Apache-2.0. The ones that are not purely permissive:

- `projectm`, `projectm-sys` — LGPL
- `symphonia` and its codec/format crates, `serialport`, `option-ext` — MPL-2.0

## Linked at build time, not included

| Library | Used for | License |
|---------|----------|---------|
| FFmpeg | Video decoding, recording, RTMP | LGPL 2.1+ or GPL, depending on how your copy was built (Homebrew's build is GPL) |
| GStreamer and its plugins | Streaming pipelines, multi-channel capture | LGPL 2.1+ for the core; individual plugins vary |
| NDI SDK (optional) | NDI output | Proprietary. NDI® is a registered trademark of Vizrt NDI AB |

## Model weights, not included

The camera detection model is exported locally (see `models/README.md`) and is
bundled into built apps, not into this repository.

| Model | Used for | License |
|-------|----------|---------|
| [RF-DETR](https://github.com/roboflow/rf-detr) Nano, COCO checkpoint, Copyright 2025 Roboflow | Camera person detection | Apache License 2.0. Trained on [COCO 2017](https://cocodataset.org/) (annotations CC BY 4.0) |

If you distribute a built copy of REACT, you are responsible for complying with
the licenses of every library you bundle with it.
