#!/usr/bin/env python3
"""Compare two screen captures.

usage: compare.py A.png B.png [SIDE.png] [HEIGHT]

Prints the mean absolute pixel difference (0 = identical) over the common
top 800 x HEIGHT area (default 1200) and optionally writes A and B side by side.
Requires Pillow.
"""
import sys

from PIL import Image, ImageChops, ImageStat

a = Image.open(sys.argv[1]).convert("RGB")
b = Image.open(sys.argv[2]).convert("RGB")
h = int(sys.argv[4]) if len(sys.argv) > 4 else 1200
h = min(h, a.height, b.height)
ca, cb = a.crop((0, 0, 800, h)), b.crop((0, 0, 800, h))
diff = ImageStat.Stat(ImageChops.difference(ca, cb)).mean
print(f"mean_abs_diff={sum(diff) / 3:.2f} (0=identical) over 800x{h}")
if len(sys.argv) > 3:
    out = Image.new("RGB", (1600, h), "white")
    out.paste(ca, (0, 0))
    out.paste(cb, (800, 0))
    out.save(sys.argv[3])
