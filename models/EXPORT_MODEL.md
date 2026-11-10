# REACT Vision - Model Export Guide

## How It Works

REACT Vision uses **YOLO-World**, an open-vocabulary object detector. You tell it what words/classes to detect, and it learns to find those things.

**Two files needed:**
- `yolov8m-world.onnx` - The model (detects objects)
- `classes.json` - The class names (maps detection IDs to words)

These must match! The model is "baked" with specific classes during export.

---

## Quick Start: Change What Gets Detected

**Step 1:** Edit `classes.json`
```json
[
  "person",
  "face",
  "guitar",
  "your custom thing"
]
```

**Step 2:** Run the export script
```bash
cd models
python3 export_model.py
```

**Step 3:** Restart REACT

That's it! The model now detects your custom classes.

---

## Requirements

- Python 3.8+
- Internet connection (downloads ~55MB base model)

**One-time setup:**
```bash
pip3 install ultralytics
```

---

## Current Class List

The default `classes.json` includes 26 classes optimized for live events:

| Category | Classes |
|----------|---------|
| People | person, face, hand |
| Body Art | tattoo |
| Eyewear | glasses, sunglasses |
| Headwear | hat |
| Jewelry | necklace, chain, bracelet, earring, watch |
| Drinks | cup, bottle, beer |
| Tech | phone, camera, laptop |
| Festival | glow stick, lighter, flag |
| Music | microphone, guitar, drum, headphones, turntable |

---

## Customizing Classes

Edit `classes.json` with any objects you want to detect:

```json
[
  "person",
  "face",
  "tattoo",
  "custom object",
  "another thing"
]
```

**Tips:**
- Use simple, common words (YOLO-World understands natural language)
- Avoid similar/overlapping classes (e.g., don't use both "hoodie" and "jacket")
- Fewer classes = faster detection, fewer false positives
- Test with 10-30 classes first, add more if needed

---

## How the System Works

```
┌─────────────────────────────────────────────────────────────┐
│                     EXPORT (one time)                        │
├─────────────────────────────────────────────────────────────┤
│  classes.json          export_model.py                       │
│  ["person",    ───────►  Downloads base model                │
│   "face",                Sets classes                        │
│   "guitar"]              Exports to ONNX                     │
│                              │                               │
│                              ▼                               │
│                     yolov8m-world.onnx                       │
│                     (classes baked in)                       │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     RUNTIME (every frame)                    │
├─────────────────────────────────────────────────────────────┤
│  Camera Frame ──► Model ──► "class 0 at x,y"                │
│                              "class 1 at x,y"                │
│                                   │                          │
│  classes.json ──────────────────►│                          │
│  (maps IDs to names)              ▼                          │
│                             "person at x,y"                  │
│                             "face at x,y"                    │
└─────────────────────────────────────────────────────────────┘
```

---

## Troubleshooting

**Labels show "class_0", "class_1":**
- `classes.json` missing or doesn't match model
- Re-run `python3 export_model.py`

**Wrong labels (face shows as "bottle"):**
- Model was exported with different classes
- Re-run `python3 export_model.py`

**Too many false positives:**
- Remove similar/overlapping classes from list
- Use more specific words

**Nothing detected:**
- Check model file exists: `yolov8m-world.onnx`
- Re-run export script

---

## Files

| File | Purpose |
|------|---------|
| `yolov8m-world.onnx` | Detection model (~100MB) |
| `classes.json` | Class names - edit this! |
| `export_model.py` | Export script |
| `EXPORT_MODEL.md` | This guide |
