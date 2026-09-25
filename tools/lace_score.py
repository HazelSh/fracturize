#!/usr/bin/env python3
"""Score a --fit-preview contact sheet for lace-vs-fog, cell by cell.

    tools/lace_score.py sheet.png [sheet2.png ...] [--cell 480] [--cols auto]

Why this exists: FIT-OPTIMIZER-PLAN.md rule 1 says never judge a fit by its
loss, render the sheet and look. That is right, and it does not scale past a
few sheets -- "I looked at twenty cells and the 0.4 ones seemed better" is the
same n<=4 eyeball judgement the plan warns about, just with more cells. This
puts one number on the single axis §1 is actually about: does the attractor
have internal structure, or is it a smooth cloud?

    lace = mean over occupied pixels of (local sd / local mean)

A local coefficient of variation over a 9x9 window. Fog is bright and smooth,
so its local sd is near zero against a large local mean and it scores low.
Filaments, holes and lattices swing between bright and empty within the window
and score high.

Two things it deliberately is not:

  * It is not the SSIM term the fit optimises. Scoring candidates with the
    objective under test would be circular -- the question is whether that
    objective picks good pictures, which it cannot answer about itself.

  * It is not a measure of likeness, and it must never be read as one. A
    lattice of straight lines scores near the top of this metric and looks
    nothing like the reference. Structure is necessary for a good fit here and
    nowhere near sufficient, so a high score means "worth looking at", not
    "good". The eye still decides; this only says which cells to spend it on.

**Always read `lace` next to `cov`.** A nearly-empty cell -- a collapsed fit,
a few line segments on black -- has enormous local contrast and scores at the
very top of this metric. Measured on the 2026-08-19 perceptual sweep,
corr(lace, coverage) was -0.54 at `--fit-perceptual 0.4` and -0.87 at 1.0, so
at the sparse end `lace` is mostly a readout of emptiness and ranking by it
alone selects degenerate fits. Coverage -- the fraction of the cell carrying
any material -- is what separates the three cases:

    fog        high coverage, low lace
    filigree   mid coverage,  high lace     <- the one you want
    collapsed  low coverage,  high lace

That is why `cov` is printed in the same table and not offered as an option.
"""
import argparse
import re
import sys

import numpy as np
from PIL import Image

# Height of the label plate drawn by src/glyphs.rs at a 480 px cell, plus
# slack. Skipped so the amber text does not read as high local contrast.
LABEL_SKIP = 32
WINDOW = 9
# Below this local mean a pixel is background, not thin structure.
OCCUPIED = 0.02


def box(a, k):
    """Mean over a k x k window, via a summed-area table. k odd."""
    r = k // 2
    p = np.pad(a, r, mode="edge")
    c = np.cumsum(np.cumsum(p, 0), 1)
    c = np.pad(c, ((1, 0), (1, 0)))
    h, w = a.shape
    return (c[k:k + h, k:k + w] - c[0:h, k:k + w]
            - c[k:k + h, 0:w] + c[0:h, 0:w]) / (k * k)


def coverage(cell):
    """Fraction of the cell carrying any material.

    The axis `lace` is blind to. Read them together -- see the module docstring.
    """
    g = np.asarray(cell.convert("L"), dtype=np.float64)[LABEL_SKIP:, :] / 255.0
    return float((g > OCCUPIED).mean())


def lace(cell):
    g = np.asarray(cell.convert("L"), dtype=np.float64) / 255.0
    g = g[LABEL_SKIP:, :]
    m = box(g, WINDOW)
    v = np.maximum(box(g * g, WINDOW) - m * m, 0.0)
    occupied = m > OCCUPIED
    if occupied.sum() < 100:
        return 0.0
    return float((np.sqrt(v[occupied]) / (m[occupied] + 1e-6)).mean())


def cells(path, cell, cols):
    """Every cell of a sheet except cell 0, which --fit-preview fills with
    the target. Yields (index, image)."""
    sheet = Image.open(path)
    if cols is None:
        cols = max(2, sheet.width // cell)
    rows = sheet.height // cell
    for i in range(1, cols * rows):
        x, y = (i % cols) * cell, (i // cols) * cell
        crop = sheet.crop((x, y, x + cell, y + cell))
        # Trailing slots in the last row are empty padding, not fits.
        if np.asarray(crop.convert("L")).max() < 8:
            continue
        yield i, crop


def losses_from_log(path, which=-1):
    """{sheet cell index: (seed, loss)} out of a --fit run's ranked table.

    Keyed by *index*, not by the log's own "cell N": the sheet puts the target
    in slot 0 and the log calls the first fit "cell 2", so the two differ by
    one. Getting this wrong pairs every score with the wrong seed and still
    prints a plausible-looking table, which is why it is done in one place.

    A log holding several runs (a shell loop over conditions) has one table
    each; `which` selects it, defaulting to the last.
    """
    if not path:
        return {}
    text = open(path).read()
    tables, current = [], {}
    for line in text.splitlines():
        m = re.match(r"cell\s+(\d+)\s+seed\s+(\d+): loss ([\d.]+)", line.strip())
        if m:
            current[int(m.group(1)) - 1] = (int(m.group(2)), float(m.group(3)))
        elif current:
            tables.append(current)
            current = {}
    if current:
        tables.append(current)
    if not tables:
        return {}
    if len(tables) > 1:
        print(f"note: {path} holds {len(tables)} runs; using #{which % len(tables) + 1}",
              file=sys.stderr)
    return tables[which]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("sheets", nargs="+")
    ap.add_argument("--cell", type=int, default=480, help="cell side in px")
    ap.add_argument("--cols", type=int, default=None, help="default: inferred")
    ap.add_argument("--log", default=None,
                    help="a --fit run's stdout, to pair scores with losses")
    ap.add_argument("--run", type=int, default=-1,
                    help="which run in a multi-run log (default: the last)")
    args = ap.parse_args()

    losses = losses_from_log(args.log, args.run)
    for path in args.sheets:
        scored = [(i, lace(c), coverage(c))
                  for i, c in cells(path, args.cell, args.cols)]
        if not scored:
            print(f"{path}: no cells found", file=sys.stderr)
            continue
        vals = np.array([v for _, v, _ in scored])
        covs = np.array([c for _, _, c in scored])
        print(f"\n=== {path} ===")
        print(f"{len(scored)} cells   mean {vals.mean():.3f}   "
              f"median {np.median(vals):.3f}   "
              f"min {vals.min():.3f}   max {vals.max():.3f}")
        print(f"coverage  mean {covs.mean():.3f}   min {covs.min():.3f}"
              f"   corr(lace, cov) {np.corrcoef(vals, covs)[0, 1]:+.3f}")
        if (covs < 0.10).any():
            print(f"  !! {int((covs < 0.10).sum())} cell(s) under 10% coverage -- "
                  f"collapsed fits, which score HIGH on lace. Ignore their rank.")

        if losses:
            paired = [(losses[i][0], losses[i][1], v, c)
                      for i, v, c in scored if i in losses]
            if len(paired) > 2:
                ls = np.array([p[1] for p in paired])
                vs = np.array([p[2] for p in paired])
                r = np.corrcoef(ls, vs)[0, 1]
                print(f"corr(loss, lace) = {r:+.3f} over {len(paired)} fits"
                      f"   ({'lower loss = more fog' if r > 0 else 'lower loss = more lace'})")
            print("  seed   loss     lace    cov")
            for seed, loss, v, c in sorted(paired, key=lambda t: -t[2]):
                flag = "  <- collapsed" if c < 0.10 else ""
                print(f"  {seed:>4}   {loss:.4f}   {v:.3f}  {c:.3f}{flag}")
        else:
            print("  cell    lace    cov")
            for i, v, c in sorted(scored, key=lambda t: -t[1]):
                flag = "  <- collapsed" if c < 0.10 else ""
                print(f"  {i:>4}   {v:.3f}  {c:.3f}{flag}")


if __name__ == "__main__":
    main()
