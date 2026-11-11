#!/usr/bin/env python3
"""Export the REACT camera detector to ONNX.

Downloads an official checkpoint, exports it with fixed shapes (batch 1) and
stores the label list and the classes REACT should report in the model file's
metadata, so the app needs nothing but the .onnx.

    python3 scripts/export-detector.py                       # models/rfdetr-nano.onnx
    python3 scripts/export-detector.py --model rfdetr-small  # larger input, slower
    python3 scripts/export-detector.py --detect person,chair
    python3 scripts/export-detector.py --info models/rfdetr-nano.onnx
    python3 scripts/export-detector.py --set-detect all models/rfdetr-nano.onnx

Setup (once, in a virtual environment):

    pip install "rfdetr[onnx]==1.12.0"

The D-FINE presets (--model dfine-m-coco, dfine-s-coco) and other D-FINE
checkpoints need D-FINE's own requirements instead. See models/README.md for
that, the model contract, measured speed and licences.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import urllib.request

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Versions the shipped model was exported with. Pinned so an export is reproducible.
RFDETR_VERSION = "1.12.0"
DFINE_REPO = "https://github.com/Peterande/D-FINE.git"
DFINE_COMMIT = "956d1709314c2c6a4df6f34de232054578a7449f"

RFDETR_WEIGHTS = "https://repo.roboflow.com/rfdetr/"
DFINE_WEIGHTS = "https://github.com/Peterande/storage/releases/download/dfinev1.0/"

# Checkpoints trained on COCO. The code and these weights are Apache-2.0.
PRESETS = {
    "rfdetr-nano": {
        "family": "rfdetr",
        "name": "RF-DETR Nano COCO",
        "class": "RFDETRNano",
        "checkpoint": RFDETR_WEIGHTS + "nano_coco/checkpoint_best_regular.pth",
        "file": "rf-detr-nano.pth",
        "sha256": "d8d6b9ee57d4d0ed2b1f305163624712a0532cb7bce0c747317984fc5457440d",
    },
    "rfdetr-small": {
        "family": "rfdetr",
        "name": "RF-DETR Small COCO",
        "class": "RFDETRSmall",
        "checkpoint": RFDETR_WEIGHTS + "small_coco/checkpoint_best_regular.pth",
        "file": "rf-detr-small.pth",
        "sha256": "d81979a9213a2109345158ce9232668df4c1ae52e9b8db3f2ec0a8cbad959b33",
    },
    "dfine-m-coco": {
        "family": "dfine",
        "name": "D-FINE-M COCO",
        "config": "configs/dfine/dfine_hgnetv2_m_coco.yml",
        "checkpoint": DFINE_WEIGHTS + "dfine_m_coco.pth",
        "file": "dfine_m_coco.pth",
        "sha256": "b44a7586bf490858c7b8bce9e44bd025cb88724df9a07a8deb3ae1c12e608195",
    },
    "dfine-s-coco": {
        "family": "dfine",
        "name": "D-FINE-S COCO",
        "config": "configs/dfine/dfine_hgnetv2_s_coco.yml",
        "checkpoint": DFINE_WEIGHTS + "dfine_s_coco.pth",
        "file": "dfine_s_coco.pth",
        "sha256": "48a6c8cc43eb57186843f752e2e8461ddd3326e0d3c575e71e6e960844683e89",
    },
}
DEFAULT_MODEL = "rfdetr-nano"

# The input normalization src/camera/detector.rs applies to RF-DETR models
IMAGENET_MEAN = [0.485, 0.456, 0.406]
IMAGENET_STD = [0.229, 0.224, 0.225]

DFINE_INPUT_SIZE = 640
DFINE_OPSET = 17


def fail(message):
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch_checkpoint(source, weights_dir, expected_sha256, filename=None):
    """Return a local path for the checkpoint, downloading it when `source` is a URL."""
    if not source.startswith(("http://", "https://")):
        if not os.path.isfile(source):
            fail(f"checkpoint not found: {source}")
        return os.path.abspath(source)

    os.makedirs(weights_dir, exist_ok=True)
    path = os.path.join(weights_dir, filename or os.path.basename(source))
    if not os.path.isfile(path):
        print(f"Downloading {source} ...")
        tmp = path + ".part"
        urllib.request.urlretrieve(source, tmp)
        os.replace(tmp, path)
    if expected_sha256:
        actual = sha256_of(path)
        if actual != expected_sha256:
            fail(f"{path} has SHA-256 {actual}, expected {expected_sha256}. Delete it and retry.")
    return path


def parse_detect(text, labels):
    """`all` reports every class; otherwise a comma-separated list of label names."""
    if text.strip().lower() == "all":
        return None
    wanted = [name.strip() for name in text.split(",") if name.strip()]
    known = {label.lower(): label for label in labels if label}
    unknown = [name for name in wanted if name.lower() not in known]
    if unknown:
        fail(f"--detect names not in the label list: {', '.join(unknown)}")
    return [known[name.lower()] for name in wanted]


def set_metadata(model, props):
    """Replace the given metadata keys, leaving the others alone. `None` removes a key."""
    kept = [p for p in model.metadata_props if p.key not in props]
    del model.metadata_props[:]
    model.metadata_props.extend(kept)
    for key, value in props.items():
        if value is not None:
            entry = model.metadata_props.add()
            entry.key, entry.value = key, value


def read_metadata(path):
    import onnx
    model = onnx.load(path, load_external_data=False)
    return {p.key: p.value for p in model.metadata_props}


def print_info(path):
    meta = read_metadata(path)
    named = [label for label in json.loads(meta.get("labels", "[]")) if label]
    detect = json.loads(meta["detect"]) if "detect" in meta else None
    print(f"{path}  ({os.path.getsize(path) / 1e6:.1f} MB)")
    for key in ("model", "source", "license", "rfdetr_version", "dfine_commit"):
        if key in meta:
            print(f"  {key}: {meta[key]}")
    print(f"  labels: {len(named)}" + (f"  ({', '.join(named[:6])}, ...)" if named else "  (none: app shows class_N)"))
    print(f"  detect: {', '.join(detect) if detect else 'every class'}")


def source_label(checkpoint_source):
    """A local checkpoint is recorded by file name only: no local paths in the model file."""
    if checkpoint_source.startswith(("http://", "https://")):
        return checkpoint_source
    return os.path.basename(checkpoint_source)


def export_rfdetr(args, preset, out, weights_dir):
    try:
        import numpy as np
        import onnx
        import onnxruntime
        import rfdetr
    except ImportError as e:
        fail(f"{e}. Install the export requirements:\n"
             f"  pip install \"rfdetr[onnx]=={RFDETR_VERSION}\"")
    from importlib.metadata import version
    if version("rfdetr") != RFDETR_VERSION:
        print(f"note: this script was verified with rfdetr {RFDETR_VERSION}, "
              f"found {version('rfdetr')}")

    checkpoint = fetch_checkpoint(preset["checkpoint"], weights_dir, preset["sha256"], preset["file"])
    print(f"Exporting {preset['name']} to ONNX ...")
    model = getattr(rfdetr, preset["class"])(pretrain_weights=checkpoint)
    with tempfile.TemporaryDirectory() as tmp:
        graph = onnx.load(str(model.export(output_dir=tmp, verbose=False)))

    # RF-DETR's export describes itself; check what detector.rs takes for granted
    described = {p.key: p.value for p in graph.metadata_props}.get("rfdetr_inference")
    if not described:
        fail("this rfdetr version writes no 'rfdetr_inference' metadata; "
             f"use rfdetr {RFDETR_VERSION}")
    described = json.loads(described)
    expected = {"means": IMAGENET_MEAN, "stds": IMAGENET_STD, "channel_order": "RGB",
                "box_format": "cxcywh_normalized", "input_name": "input",
                "outputs": {"pred_boxes": "dets", "pred_logits": "labels"}}
    for key, value in expected.items():
        if described.get(key) != value:
            fail(f"unexpected export contract: {key} is {described.get(key)!r}, "
                 f"REACT expects {value!r}")

    # Logit column = class id; ids the dataset does not use stay unnamed
    logits = next(o for o in graph.graph.output if o.name == "labels")
    num_classes = logits.type.tensor_type.shape.dim[-1].dim_value
    names = described["class_id_to_name"]
    labels = [names.get(str(i), "") for i in range(num_classes)]
    detect = parse_detect(args.detect, labels)

    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    set_metadata(graph, {
        "labels": json.dumps(labels),
        "detect": json.dumps(detect) if detect else None,
        "model": preset["name"],
        "source": source_label(preset["checkpoint"]),
        "license": "Apache-2.0 (RF-DETR)",
        "rfdetr_version": version("rfdetr"),
    })
    onnx.save(graph, out)
    onnx.checker.check_model(out)

    # Run it once the way the app does and check the contract detector.rs relies on
    session = onnxruntime.InferenceSession(out, providers=["CPUExecutionProvider"])
    inputs = {i.name: i.shape for i in session.get_inputs()}
    outputs = {o.name: o.shape for o in session.get_outputs()}
    size = inputs["input"][2]
    assert isinstance(size, int) and inputs == {"input": [1, 3, size, size]}, inputs
    assert outputs["dets"][2] == 4 and outputs["labels"][2] == len(labels), outputs
    boxes, scores = session.run(["dets", "labels"], {
        "input": np.random.rand(1, 3, size, size).astype(np.float32),
    })
    assert np.isfinite(boxes).all() and np.isfinite(scores).all(), "model output is not finite"

    print()
    print_info(out)
    print(f"  input: {inputs}")
    print(f"  outputs: {outputs}")


def ensure_dfine(dfine_dir):
    """Clone D-FINE at the pinned commit unless a checkout is already there."""
    if os.path.isfile(os.path.join(dfine_dir, "src", "core", "__init__.py")):
        return
    print(f"Cloning D-FINE into {dfine_dir} ...")
    os.makedirs(os.path.dirname(dfine_dir), exist_ok=True)
    subprocess.run(["git", "clone", "--quiet", DFINE_REPO, dfine_dir], check=True)
    subprocess.run(["git", "-C", dfine_dir, "checkout", "--quiet", DFINE_COMMIT], check=True)


def load_dfine_labels(spec, dfine_dir):
    """`coco` reads D-FINE's own COCO name table; anything else is a JSON array file."""
    if spec == "coco":
        sys.path.insert(0, dfine_dir)
        from src.data.dataset.coco_dataset import mscoco_category2name
        return list(mscoco_category2name.values())
    with open(spec) as f:
        labels = json.load(f)
    if not (isinstance(labels, list) and all(isinstance(x, str) for x in labels)):
        fail(f"{spec} must be a JSON array of strings (index = class id)")
    return labels


def export_dfine(args, preset, out, weights_dir, dfine_dir):
    """`preset` is None for a custom export (--config and --checkpoint)."""
    config = args.config or preset["config"]
    checkpoint_source = args.checkpoint or preset["checkpoint"]
    name = args.name or (preset["name"] if preset else "custom D-FINE export")
    labels_spec = os.path.abspath(args.labels) if args.labels else "coco"
    out = os.path.abspath(out)

    ensure_dfine(dfine_dir)
    checkpoint = fetch_checkpoint(
        checkpoint_source, weights_dir,
        preset["sha256"] if preset else None,
        preset["file"] if preset else None,
    )

    try:
        import onnx
        import onnxruntime
        import onnxslim
        import torch
        import torch.nn as nn
    except ImportError as e:
        fail(f"{e}. Install the D-FINE export requirements:\n"
             f"  pip install -r {dfine_dir}/requirements.txt onnx onnxslim onnxruntime")

    labels = load_dfine_labels(labels_spec, dfine_dir)
    detect = parse_detect(args.detect, labels)

    # D-FINE resolves its config includes relative to its own checkout
    sys.path.insert(0, dfine_dir)
    os.chdir(dfine_dir)
    from src.core import YAMLConfig

    cfg = YAMLConfig(config, resume=checkpoint)
    if "HGNetv2" in cfg.yaml_cfg:
        cfg.yaml_cfg["HGNetv2"]["pretrained"] = False  # weights come from the checkpoint
    state = torch.load(checkpoint, map_location="cpu", weights_only=True)
    cfg.model.load_state_dict(state["ema"]["module"] if "ema" in state else state["model"])

    class Deploy(nn.Module):
        def __init__(self):
            super().__init__()
            self.model = cfg.model.deploy()
            self.postprocessor = cfg.postprocessor.deploy()

        def forward(self, images, orig_target_sizes):
            return self.postprocessor(self.model(images), orig_target_sizes)

    size = DFINE_INPUT_SIZE
    model = Deploy().eval()
    images = torch.rand(1, 3, size, size)
    sizes = torch.tensor([[size, size]])
    with torch.no_grad():
        model(images, sizes)

    os.makedirs(os.path.dirname(out), exist_ok=True)
    export_args = dict(
        input_names=["images", "orig_target_sizes"],
        output_names=["labels", "boxes", "scores"],
        opset_version=DFINE_OPSET,
        do_constant_folding=True,
    )
    print(f"Exporting {name} to ONNX (opset {DFINE_OPSET}, {size}x{size}, batch 1) ...")
    try:
        torch.onnx.export(model, (images, sizes), out, dynamo=False, **export_args)
    except TypeError:  # PyTorch older than 2.5 has no `dynamo` argument
        torch.onnx.export(model, (images, sizes), out, **export_args)

    graph = onnxslim.slim(out)
    set_metadata(graph, {
        "labels": json.dumps(labels),
        "detect": json.dumps(detect) if detect else None,
        "model": name,
        "source": source_label(checkpoint_source),
        "license": "Apache-2.0 (D-FINE)" if preset else None,
        "dfine_commit": subprocess.run(
            ["git", "-C", dfine_dir, "rev-parse", "HEAD"], capture_output=True, text=True
        ).stdout.strip() or None,
    })
    onnx.save(graph, out)
    onnx.checker.check_model(out)

    # Run it once the way the app does and check the contract detector.rs relies on
    session = onnxruntime.InferenceSession(out, providers=["CPUExecutionProvider"])
    inputs = {i.name: i.shape for i in session.get_inputs()}
    outputs = {o.name: o.shape for o in session.get_outputs()}
    assert inputs == {"images": [1, 3, size, size], "orig_target_sizes": [1, 2]}, inputs
    assert set(outputs) == {"labels", "boxes", "scores"}, outputs
    result = session.run(["labels", "boxes", "scores"], {
        "images": images.numpy(), "orig_target_sizes": sizes.numpy(),
    })
    assert result[1].shape[-1] == 4 and result[0].max() < len(labels), "label ids exceed the label list"

    print()
    print_info(out)
    print(f"  outputs: {outputs}")


def main():
    parser = argparse.ArgumentParser(
        description="Export the REACT camera detector to ONNX.",
        epilog="See models/README.md.",
    )
    parser.add_argument("--model", choices=sorted(PRESETS), default=DEFAULT_MODEL,
                        help=f"which checkpoint to export (default: {DEFAULT_MODEL})")
    parser.add_argument("--out", help="output file (default: models/<model>.onnx)")
    parser.add_argument("--detect", default="person",
                        help="comma-separated classes REACT reports, or 'all' (default: person)")
    parser.add_argument("--cache-dir", default="~/.cache/react-vision",
                        help="where checkpoints and the D-FINE checkout are kept")
    parser.add_argument("--info", metavar="ONNX", help="print a model file's metadata and exit")
    parser.add_argument("--set-detect", nargs=2, metavar=("CLASSES", "ONNX"),
                        help="change the reported classes of an exported file without re-exporting")
    custom = parser.add_argument_group(
        "other D-FINE checkpoints", "give --config, --checkpoint and --out together")
    custom.add_argument("--config", help="D-FINE config, relative to the D-FINE checkout")
    custom.add_argument("--checkpoint", help="D-FINE checkpoint: URL or local .pth")
    custom.add_argument("--labels", help="JSON array of class names, index = class id (default: COCO)")
    custom.add_argument("--name", help="model name stored in the file's metadata")
    custom.add_argument("--dfine-dir", help="D-FINE checkout (default: <cache-dir>/D-FINE, "
                                            "cloned at the pinned commit when missing)")
    args = parser.parse_args()

    if args.info:
        print_info(args.info)
        return
    if args.set_detect:
        import onnx
        classes, path = args.set_detect
        labels = json.loads(read_metadata(path).get("labels", "[]"))
        detect = parse_detect(classes, labels)
        model = onnx.load(path)
        set_metadata(model, {"detect": json.dumps(detect) if detect else None})
        onnx.save(model, path)
        print_info(path)
        return

    cache_dir = os.path.abspath(os.path.expanduser(args.cache_dir))
    weights_dir = os.path.join(cache_dir, "weights")
    dfine_dir = os.path.abspath(os.path.expanduser(args.dfine_dir or os.path.join(cache_dir, "D-FINE")))

    if args.config or args.checkpoint or args.labels or args.name:
        if not (args.config and args.checkpoint and args.out):
            fail("a custom D-FINE export needs --config, --checkpoint and --out "
                 "(name the file models/dfine-<something>.onnx so REACT finds it)")
        export_dfine(args, None, args.out, weights_dir, dfine_dir)
        return

    preset = PRESETS[args.model]
    out = args.out or os.path.join(REPO_ROOT, "models", f"{args.model}.onnx")
    if preset["family"] == "rfdetr":
        export_rfdetr(args, preset, out, weights_dir)
    else:
        export_dfine(args, preset, out, weights_dir, dfine_dir)


if __name__ == "__main__":
    main()
