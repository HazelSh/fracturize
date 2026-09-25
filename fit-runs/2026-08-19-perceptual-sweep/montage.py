"""Top-3 lace cells per perceptual value, one row each, plus a likeness proxy.

Row order top to bottom: target, then 0.0 0.2 0.4 0.6 0.8 1.0.

The likeness proxy is deliberately not any of the three terms the fit
optimises: both images are tone-mapped renders, blurred and downsampled to
32x32, and compared by Pearson correlation. Crude, but it is answering a
question `lace` structurally cannot -- whether the structure is in the shape of
the subject or just present.
"""
import sys
import numpy as np
from PIL import Image
sys.path.insert(0, 'tools')
from lace_score import lace, cells

VALUES = ['0.0', '0.2', '0.4', '0.6', '0.8', '1.0']
CELL, TH = 480, 300
root = sys.argv[1]

def small(im):
    g = np.asarray(im.convert('L').resize((32, 32), Image.LANCZOS), float) / 255.
    return (g - g.mean()) / (g.std() + 1e-9)

target = small(Image.open(f'{root}/p0.0.png').crop((0, 0, CELL, CELL)))

rows, report = [], []
for tag in VALUES:
    sc = [(i, c, lace(c)) for i, c in cells(f'{root}/p{tag}.png', CELL, None)]
    sc.sort(key=lambda t: -t[2])
    top = sc[:3]
    rows.append([c for _, c, _ in top])
    lk = [float((small(c) * target).mean()) for _, c, _ in top]
    allk = [float((small(c) * target).mean()) for _, c, _ in sc]
    report.append((tag, [v for _, _, v in top], lk, np.mean(allk), np.max(allk)))

out = Image.new('RGB', (3 * TH, (len(rows) + 1) * TH), 'black')
out.paste(Image.open(f'{root}/p0.0.png').crop((0, 0, CELL, CELL)).resize((TH, TH)), (0, 0))
for r, row in enumerate(rows):
    for c, im in enumerate(row):
        out.paste(im.resize((TH, TH)), (c * TH, (r + 1) * TH))
out.save(f'{root}/montage.png')

print(f'{"perc":>5} {"top-3 lace":>22} {"top-3 likeness":>24} '
      f'{"likeness mean":>14} {"likeness best":>14}')
for tag, lc, lk, am, ax in report:
    print(f'{tag:>5} {" ".join(f"{v:.2f}" for v in lc):>22} '
          f'{" ".join(f"{v:+.3f}" for v in lk):>24} {am:>14.3f} {ax:>14.3f}')
