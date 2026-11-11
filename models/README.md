# Camera detection model

Camera tracking finds performers with **RF-DETR Nano**, a real-time detection
transformer from Roboflow, trained on COCO. REACT loads it from
`models/rfdetr-nano.onnx` and runs it with ONNX Runtime on the CPU.

Model weights are not tracked in git (`/models/*.onnx` is ignored). Export the
file once; `build.sh` then copies it into the app bundle.

## Export the model

Needs Python 3.10 or newer.

```bash
python3 -m venv ~/.cache/react-vision/venv && source ~/.cache/react-vision/venv/bin/activate
pip install "rfdetr[onnx]==1.12.0"
python3 scripts/export-detector.py
```

This downloads the official checkpoint (about 370 MB, checked against a pinned
SHA-256), exports it and writes `models/rfdetr-nano.onnx` (108 MB). It needs no
GPU. Last verified from an empty environment with Python 3.12, rfdetr 1.12.0,
PyTorch 2.15 and ONNX 1.23.

Inspect a file at any time:

```bash
python3 scripts/export-detector.py --info models/rfdetr-nano.onnx
```

## Which file REACT loads

REACT looks in `models/` under the working directory, then in the source tree
it was built from, then in the app bundle's `Contents/Resources/models/`. In
each folder it takes `rfdetr-nano.onnx` when present, otherwise the
alphabetically first `rfdetr-*.onnx`, otherwise the alphabetically first
`dfine-*.onnx`. A file in neither format is rejected when camera tracking
starts, with a message that says so.

`build.sh` copies every `.onnx` file in `models/` into the app, so keep only
the one you ship there.

## Choosing what is detected

The label list and the classes REACT reports are stored inside the `.onnx`
file as metadata, so there is no separate class file to keep in sync.

The default export reports only `person`: the tracker has a fixed number of
slots, and chairs or bottles should not take them from performers. Change the
list without exporting again:

```bash
python3 scripts/export-detector.py --set-detect person,laptop models/rfdetr-nano.onnx
python3 scripts/export-detector.py --set-detect all models/rfdetr-nano.onnx
```

The class filter in the camera panel then narrows tracking further, by name,
within what the model file reports.

## Other models

RF-DETR Small takes a larger input (512 px against 384 px) and is slower:

```bash
python3 scripts/export-detector.py --model rfdetr-small   # models/rfdetr-small.onnx, 114 MB
```

REACT also reads [D-FINE](https://github.com/Peterande/D-FINE) exports. They
need D-FINE's own requirements, best in a separate virtual environment:

```bash
git clone https://github.com/Peterande/D-FINE.git ~/.cache/react-vision/D-FINE
git -C ~/.cache/react-vision/D-FINE checkout 956d1709314c2c6a4df6f34de232054578a7449f
pip install -r ~/.cache/react-vision/D-FINE/requirements.txt onnx onnxslim onnxruntime
python3 scripts/export-detector.py --model dfine-s-coco   # or dfine-m-coco
```

Any other D-FINE checkpoint works too. Pass its config, weights and a JSON
array of class names (index = class id):

```bash
python3 scripts/export-detector.py \
  --config configs/dfine/objects365/dfine_hgnetv2_m_obj365.yml \
  --checkpoint /path/to/dfine_m_obj365.pth \
  --labels /path/to/objects365-labels.json \
  --name "D-FINE-M Objects365" \
  --detect person,guitar,drum,microphone \
  --out models/dfine-m-obj365.onnx
```

The Objects365 checkpoints know 365 object types, including instruments and
microphones, and run through REACT unchanged. Their label list has 366 entries:
index 0 is unused and index 1 is `Person`. Read the licence note below before
shipping one.

## Model contract

`src/camera/detector.rs` tells the two formats apart by these tensor names, and
the export script checks them. Shapes are fixed, batch 1. The frame is
stretched to the input size (no letterbox). Neither model needs non-maximum
suppression.

RF-DETR:

| | Name | Type and shape | Meaning |
|---|---|---|---|
| Input | `input` | f32 `[1, 3, 384, 384]` | RGB, 0..1, then ImageNet mean/std per channel |
| Output | `dets` | f32 `[1, 300, 4]` | centre x, centre y, width, height as fractions of the frame |
| Output | `labels` | f32 `[1, 300, 91]` | class logits; column = index into the label list |

A query reports its best enabled class when the sigmoid of that logit reaches
the detection threshold. The label list is indexed by COCO category id: 1 is
`person`, and ids COCO does not use are empty strings that are never reported.

D-FINE:

| | Name | Type and shape | Meaning |
|---|---|---|---|
| Input | `images` | f32 `[1, 3, 640, 640]` | RGB, 0..1 |
| Input | `orig_target_sizes` | i64 `[1, 2]` | frame width, height in pixels |
| Output | `labels` | i64 `[1, 300]` | index into the label list |
| Output | `boxes` | f32 `[1, 300, 4]` | x1, y1, x2, y2 in frame pixels |
| Output | `scores` | f32 `[1, 300]` | confidence, 0..1 |

Metadata keys for both: `labels` and `detect` (JSON arrays of strings; no
`detect` means every class), plus `model`, `source`, `license` and the exporter
version for provenance.

## Measured speed

MacBook Pro, Apple M4 Max, November 2026. ONNX Runtime 1.23.2 through the `ort`
crate, CPU provider, 2 threads: the way REACT runs it. Times are the median of
a whole `Detector::detect` call (resize, inference, decoding) in a release
build.

| Model | File | Input | Time per frame |
|---|---|---|---|
| RF-DETR Nano (default) | 108 MB | 384 px | 39 to 45 ms |
| RF-DETR Small | 114 MB | 512 px | 73 ms |
| D-FINE-S COCO | 43 MB | 640 px | 53 ms |
| D-FINE-M COCO | 80 MB | 640 px | 86 to 89 ms |

The default detection rate is 12 per second, which is 83 ms per frame, so
RF-DETR Nano fits with room to spare. Cameras are processed one after another,
so each extra camera adds the same time again to every round. The machine was
in everyday use with other apps running; one RF-DETR Nano run in ten came out
at 85 ms, so expect an occasional slow frame. Time a model with the test below,
not with the Python `onnxruntime` wheel, which measured about 3.5 times slower
than the build REACT links.

## Test without a camera

The decoders, class table and frame preprocessing have unit tests that need no
model file:

```bash
cargo test camera::
```

To run the real model on a picture (a PNG with at least one person in it):

```bash
REACT_VISION_TEST_IMAGE=/path/to/people.png \
  cargo test --release --features camera detector_finds_people_in_image -- --ignored --nocapture
```

It prints every detection and the median time per frame. Set
`REACT_VISION_TEST_MODEL` to test a file other than the one in `models/`. On
the photos tried, the output matched RF-DETR's own Python prediction box for
box.

## Licences

- RF-DETR code and the Nano, Small, Medium and Large detection checkpoints:
  Apache-2.0 (<https://github.com/roboflow/rf-detr>). Roboflow's other
  detection sizes (Atto, Femto, Pico, XL, 2XL) are under its Platform Model
  License and are not used here.
- D-FINE code and its COCO checkpoints: Apache-2.0.
- The presets detect the COCO 2017 classes; COCO's annotations are CC BY 4.0.
- D-FINE Objects365 checkpoints (`*_obj365`, and `*_obj2coco`, which are
  pretrained on Objects365): the D-FINE README says they "may be subject to the
  Objects365 dataset terms and should not be assumed to be commercially cleared
  under the D-FINE license". Settle that before shipping one.
- The detector REACT used before (YOLOv8-World) was AGPL-3.0, weights included.
  It is no longer loaded.
