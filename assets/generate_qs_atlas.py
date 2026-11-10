#!/usr/bin/env python3
"""
Generate a 24×15 number atlas (1-360) for the QS Wheel.
Layout matches the wheel: 24 columns (wedges) × 15 rows (rings).
"""

from PIL import Image, ImageDraw, ImageFont
import os

# Atlas dimensions: 24 columns × 15 rows = 360 cells
COLS, ROWS = 24, 15
CELL_SIZE = 128  # pixels per cell
WIDTH, HEIGHT = COLS * CELL_SIZE, ROWS * CELL_SIZE

# Create transparent image
img = Image.new('RGBA', (WIDTH, HEIGHT), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)

# Try to load a good font
font_paths = [
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/Library/Fonts/Arial Bold.ttf",
]

font = None
for fp in font_paths:
    if os.path.exists(fp):
        try:
            font = ImageFont.truetype(fp, 64)
            print(f"Using font: {fp}")
            break
        except:
            continue

if font is None:
    font = ImageFont.load_default()
    print("Using default font")

# Generate numbers 1-360
for i in range(360):
    num = i + 1  # 1-360
    col = i % COLS      # 0-23
    row = i // COLS     # 0-14

    # Center of this cell
    x = col * CELL_SIZE + CELL_SIZE // 2
    y = row * CELL_SIZE + CELL_SIZE // 2

    # Draw pure white number - alpha defines shape, shader blends with wedge color
    text = str(num)
    draw.text((x, y), text, fill=(255, 255, 255, 255),
              font=font, anchor="mm")

# Save the atlas
output_path = os.path.join(os.path.dirname(__file__), "qs_numbers_atlas.png")
img.save(output_path)
print(f"Saved atlas to: {output_path}")
print(f"Dimensions: {WIDTH}×{HEIGHT} ({COLS} cols × {ROWS} rows, {CELL_SIZE}px cells)")
