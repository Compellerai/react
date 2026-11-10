#!/usr/bin/env python3
"""
Generate a texture atlas for chroma wheel note labels.
12 cells in a row (C, C#, D, D#, E, F, F#, G, G#, A, A#, B)
"""
from PIL import Image, ImageDraw, ImageFont

# Note names in chromatic order
NOTES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

# Atlas dimensions: 12 columns x 1 row
COLS = 12
CELL_SIZE = 128  # pixels per cell
WIDTH = COLS * CELL_SIZE
HEIGHT = CELL_SIZE

# Create transparent image
img = Image.new('RGBA', (WIDTH, HEIGHT), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)

# Try to load a good font
font = None
font_sizes = [72, 64, 56]
font_paths = [
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
]

for fpath in font_paths:
    for fsize in font_sizes:
        try:
            font = ImageFont.truetype(fpath, fsize)
            print(f"Using font: {fpath} at size {fsize}")
            break
        except:
            continue
    if font:
        break

if not font:
    font = ImageFont.load_default()
    print("Using default font")

# Draw each note name
for i, note in enumerate(NOTES):
    x = i * CELL_SIZE + CELL_SIZE // 2
    y = CELL_SIZE // 2

    # White text with full opacity
    draw.text((x, y), note, fill=(255, 255, 255, 255), font=font, anchor="mm")

# Save the atlas
output_path = "chroma_notes_atlas.png"
img.save(output_path)
print(f"Saved atlas to {output_path}")
print(f"Atlas size: {WIDTH}x{HEIGHT} ({COLS} cells)")
