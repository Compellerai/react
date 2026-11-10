#!/usr/bin/env python3
"""
REACT Vision - Model Export Script

Reads classes.json and exports a YOLO-World model with those classes baked in.

Usage:
    cd models
    python3 export_model.py

Requirements:
    pip3 install ultralytics
"""

import json
import os
import sys

def main():
    # Check we're in the right directory
    if not os.path.exists('classes.json'):
        print("Error: classes.json not found")
        print("Run this script from the models/ directory")
        sys.exit(1)

    # Load classes from JSON
    print("Loading classes.json...")
    with open('classes.json', 'r') as f:
        classes = json.load(f)

    print(f"Found {len(classes)} classes:")
    for i, c in enumerate(classes[:10]):
        print(f"  {i}: {c}")
    if len(classes) > 10:
        print(f"  ... and {len(classes) - 10} more")

    # Import ultralytics
    try:
        from ultralytics import YOLOWorld
    except ImportError:
        print("\nError: ultralytics not installed")
        print("Run: pip3 install ultralytics")
        sys.exit(1)

    # Model size options:
    # - yolov8s-worldv2.pt (small, fast, ~25MB)
    # - yolov8m-worldv2.pt (medium, balanced, ~55MB)
    # - yolov8l-worldv2.pt (large, accurate, ~90MB)
    MODEL_SIZE = 's'  # Change to 'm' or 'l' if needed

    # Load base model (downloads if needed)
    print(f"\nLoading YOLO-World base model (size: {MODEL_SIZE})...")
    model = YOLOWorld(f'yolov8{MODEL_SIZE}-worldv2.pt')

    # Set classes
    print(f"Setting {len(classes)} classes...")
    model.set_classes(classes)

    # Export to ONNX
    print("Exporting to ONNX (this takes a minute)...")
    model.export(format='onnx', imgsz=640)

    # Rename output file
    exported_file = f'yolov8{MODEL_SIZE}-worldv2.onnx'
    if os.path.exists(exported_file):
        if os.path.exists('yolov8m-world.onnx'):
            os.remove('yolov8m-world.onnx')
        os.rename(exported_file, 'yolov8m-world.onnx')
        print(f"\nSuccess! Created yolov8m-world.onnx (from {MODEL_SIZE} model)")
    else:
        print("\nError: Export failed - no output file created")
        sys.exit(1)

    # Clean up downloaded base model
    base_model = f'yolov8{MODEL_SIZE}-worldv2.pt'
    if os.path.exists(base_model):
        os.remove(base_model)
        print("Cleaned up base model file")

    print("\nDone! Restart REACT to use the new model.")

if __name__ == '__main__':
    main()
