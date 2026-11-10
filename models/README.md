# Models Directory

Place ONNX detection models here for camera tracking.

## Required Model

**yolov8n.onnx** - YOLOv8 Nano for person detection

### Download

```bash
# Install ultralytics
pip install ultralytics

# Export YOLOv8n to ONNX
yolo export model=yolov8n.pt format=onnx imgsz=640
mv yolov8n.onnx models/
```

Or download pre-exported from:
- https://github.com/ultralytics/assets/releases

### Model Info

- Input: [1, 3, 640, 640] - RGB image normalized to 0-1
- Output: [1, 84, 8400] - 8400 predictions, 84 features (4 bbox + 80 classes)
- COCO class 0 = person

The detector will fall back to stub detection if no model is found.
