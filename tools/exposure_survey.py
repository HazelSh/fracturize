#!/usr/bin/env python3
"""Survey what exposure several scenes *want*, to settle decision 5.

Decision 5 in RENDER-QUALITY-PLAN.md is whether exposure should be normalized
per image from that image's own density (what Apophysis does), or stay the
scene-independent constant it is today.

A gamma sweep cannot answer that, because the question is not about one
picture — it is about whether **different scenes land at comfortable brightness
under one exposure**. So this renders several scenes, reads the linear density
straight out of each `.fgrade`, and asks each one: *what exposure would put
your bright material at a target coverage?*

    all scenes want roughly the same exposure  ->  adaptive normalization buys
                                                   nothing; keep the invariant
    they want wildly different exposures       ->  it is doing real work, and
                                                   today every new scene needs
                                                   hand-tuning to find it

But "what the scene wants" depends on which part of the histogram you anchor
to, and that turns out to matter more than the scenes do. So this reports the
spread under several anchoring rules rather than picking one and presenting the
result as if it were the answer. If the spread is large under every rule, the
scenes genuinely disagree. If it collapses under some rule, the disagreement
was about *dynamic range* -- which is gamma's job, not exposure's.

Usage:
    tools/exposure_survey.py                        # four contrasting scenes
    tools/exposure_survey.py scenes/a.toml scenes/b.toml
    tools/exposure_survey.py --effort medium --pct 99.5

Writes a table to stdout and a side-by-side comparison sheet, each scene at
the fixed exposure and at its own adaptive one, so the numbers can be checked
against what they look like.
"""

import argparse
import math
import pathlib
import re
import struct
import subprocess
import sys

try:
    import numpy as np
    from PIL import Image, ImageDraw
except ImportError:
    sys.exit("needs numpy and pillow: pip install numpy pillow")

ROOT = pathlib.Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "fracturize"

# Scenes chosen to *disagree*: a survey where every scene has the same density
# profile would report agreement it did not earn. These span a sparse dusty
# zoom, a dense solid body, a thin filamentary form and a bright-core one.
DEFAULT_SCENES = ["blossom", "galaxy", "fern_3d", "glasshouse"]


def tonemap_constants():
    """Read GAIN and EXPOSURE_K out of splat.rs rather than restating them.

    They are the arithmetic this whole survey inverts. A copy here that drifted
    would produce a confident table describing a renderer that no longer
    exists.
    """
    src = (ROOT / "src" / "gpu" / "points" / "splat.rs").read_text()
    def const(name):
        m = re.search(rf"const {name}: f32 = ([0-9.]+);", src)
        if not m:
            sys.exit(f"could not find `{name}` in splat.rs — has the tonemap changed?")
        return float(m.group(1))
    return const("EXPOSURE_K"), const("GAIN")


def read_fgrade(path):
    """Parse a .fgrade: header dict + float32 RGBA array shaped (h, w, 4)."""
    raw = path.read_bytes()
    magic, rest = raw.split(b"\n", 1)
    if not magic.startswith(b"fracturize-grade"):
        sys.exit(f"{path} is not a grade buffer")
    length, rest = rest.split(b"\n", 1)
    n = int(length)
    header_text = rest[:n].decode("utf-8")
    body = rest[n:]

    header = {}
    in_block = False
    for line in header_text.splitlines():
        line = line.strip()
        if line.startswith("["):
            in_block = line == "[grade_buffer]"
            continue
        if in_block and "=" in line and not line.startswith("#"):
            k, v = line.split("=", 1)
            header[k.strip()] = v.strip()

    w, h = int(float(header["width"])), int(float(header["height"]))
    px = np.frombuffer(body, dtype="<f4", count=w * h * 4).reshape(h, w, 4)
    return header, px


def exposure_for(density, target_coverage, samples, screen_height, k, gain):
    """Invert the tonemap: what exposure puts `density` at `target_coverage`?

    Forward:  coverage = log2(1 + density * exposure_scale) * gain
              exposure_scale = exposure * k * screen_height^2 / samples
    """
    if density <= 0:
        return float("nan")
    needed_scale = (2.0 ** (target_coverage / gain) - 1.0) / density
    return needed_scale * samples / (k * screen_height * screen_height)


# Where in the density histogram to anchor an adaptive exposure. The choice is
# the actual open question -- see the module docstring.
RULES = {
    "p99.5": lambda d: np.percentile(d[d > 0], 99.5),
    "p99": lambda d: np.percentile(d[d > 0], 99.0),
    "p95": lambda d: np.percentile(d[d > 0], 95.0),
    "median-lit": lambda d: np.percentile(d[d > 0], 50.0),
    "mean-all": lambda d: d.mean(),
}


def render(scene_path, out_png, effort, width, height, extra=()):
    cmd = [
        str(BIN), "-s", str(scene_path), "--splat", "--effort", effort,
        "--width", str(width), "--height", str(height),
        "--grade-out", "-r", str(out_png),
    ]
    cmd.extend(extra)
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"render failed for {scene_path}:\n{r.stdout}\n{r.stderr}")


def regrade(fgrade, out_png, exposure):
    r = subprocess.run(
        [str(BIN), "--retonemap", str(fgrade), "--exposure", f"{exposure:.6g}",
         "-r", str(out_png)],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        sys.exit(f"re-grade failed for {fgrade}:\n{r.stdout}\n{r.stderr}")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("scenes", nargs="*", default=None,
                    help="scene files or bare names (default: four contrasting ones)")
    ap.add_argument("--effort", default="small",
                    help="size tier for the survey renders [small]")
    ap.add_argument("--width", type=int, default=480)
    ap.add_argument("--height", type=int, default=320)
    ap.add_argument("--pct", type=float, default=99.5,
                    help="percentile for the per-scene profile table [99.5]")
    ap.add_argument("--rule", default="median-lit",
                    choices=list(RULES), help="anchoring rule the sheet uses [median-lit]")
    ap.add_argument("--target", type=float, default=0.75,
                    help="coverage that percentile should reach, 0-1 [0.75]")
    ap.add_argument("--fixed-exposure", type=float, default=1.0)
    ap.add_argument("--out", default="renders/exposure-survey.png")
    args = ap.parse_args()

    if not BIN.exists():
        sys.exit(f"{BIN} not found — cargo build --release first")

    names = args.scenes or DEFAULT_SCENES
    scenes = []
    for n in names:
        p = pathlib.Path(n)
        if not p.exists():
            p = ROOT / "scenes" / (n if n.endswith(".toml") else f"{n}.toml")
        if not p.exists():
            sys.exit(f"no such scene: {n}")
        scenes.append(p)

    k, gain = tonemap_constants()
    work = ROOT / "renders" / "_exposure_survey"
    work.mkdir(parents=True, exist_ok=True)

    print(f"tonemap constants from splat.rs: EXPOSURE_K={k}, GAIN={gain}")
    print(f"rendering {len(scenes)} scenes at --effort {args.effort}, "
          f"{args.width}x{args.height}\n")

    rows = []
    for p in scenes:
        stem = p.stem
        png = work / f"{stem}.png"
        render(p, png, args.effort, args.width, args.height)
        fg = png.with_suffix(".fgrade")
        header, px = read_fgrade(fg)

        samples = float(header["samples"])
        screen_h = float(header["screen_height"])
        density = px[:, :, 3]
        lit = density[density > 0]
        if lit.size == 0:
            sys.exit(f"{stem}: no density at all — is the camera pointing at it?")

        d_pct = float(np.percentile(lit, args.pct))
        d_max = float(lit.max())
        # Fraction of the frame with any density at all: a sparse scene and a
        # dense one can want the same exposure for quite different reasons, and
        # this is what tells them apart when reading the table.
        fill = float((density > 0).mean())
        rows.append(dict(stem=stem, fgrade=fg, png=png, d_pct=d_pct, d_max=d_max,
                         fill=fill, samples=samples, screen_h=screen_h,
                         density=density, d_med=float(np.percentile(lit, 50))))

    w = max(len(r["stem"]) for r in rows) + 1
    print(f"{'scene':<{w}} {'p' + str(args.pct):>12} {'max':>12} {'lit px':>8} "
          f"{'median lit':>12}")
    print("-" * (w + 48))
    for r in rows:
        print(f"{r['stem']:<{w}} {r['d_pct']:>12.4g} {r['d_max']:>12.4g} "
              f"{r['fill']*100:>7.1f}% {r['d_med']:>12.4g}")

    # The rule comparison, which is the actual deliverable. Note the spread is
    # independent of --target: the (2^(target/gain) - 1) factor is common to
    # every scene, so it cancels out of the ratio entirely. Only the *anchor*
    # moves it.
    print(f"\n{'anchoring rule':<14}" + "".join(f"{r['stem']:>12}" for r in rows)
          + f"{'spread':>10}")
    print("-" * (14 + 12 * len(rows) + 10))
    spreads = {}
    for name, f in RULES.items():
        es = [exposure_for(float(f(r["density"])), args.target, r["samples"],
                           r["screen_h"], k, gain) for r in rows]
        spreads[name] = (max(es) / min(es), es)
        print(f"{name:<14}" + "".join(f"{e:>12.3f}" for e in es)
              + f"{spreads[name][0]:>9.1f}x")

    best = min(spreads, key=lambda n: spreads[n][0])
    worst = max(spreads, key=lambda n: spreads[n][0])
    print(f"\nspread ranges from {spreads[best][0]:.1f}x ({best}) to "
          f"{spreads[worst][0]:.1f}x ({worst}).")

    if spreads[best][0] < 3:
        print(f"""
reading: the scenes AGREE about typical brightness ({best}: {spreads[best][0]:.1f}x)
  and disagree about peaks ({worst}: {spreads[worst][0]:.1f}x). That is a difference
  in *dynamic range*, not in exposure — and dynamic range is what --gamma and
  --gamma-threshold are for. An adaptive exposure anchored on peaks would be
  making a large correction for something exposure is the wrong tool for.
  Leans: keep exposure fixed and scene-independent.""")
    else:
        print(f"""
reading: the scenes disagree under every anchoring rule (best {spreads[best][0]:.1f}x
  via {best}). A fixed exposure means each new scene needs hand-tuning.
  Leans: adaptive normalization is earning its keep — pick an anchor by
  looking at the sheet below.""")

    chosen = spreads[args.rule][1]
    for r, e in zip(rows, chosen):
        r["want"] = e

    # And the picture, because a ratio is not a look.
    print("\nbuilding the comparison sheet...")
    tiles = []
    for r in rows:
        fixed = work / f"{r['stem']}-fixed.png"
        adapt = work / f"{r['stem']}-adaptive.png"
        regrade(r["fgrade"], fixed, args.fixed_exposure)
        regrade(r["fgrade"], adapt, r["want"])
        tiles.append((r["stem"], r["want"], Image.open(fixed).convert("RGB"),
                      Image.open(adapt).convert("RGB")))

    tw, th = tiles[0][2].size
    pad, label_h = 8, 20
    sheet = Image.new("RGB", (tw * 2 + pad * 3, (th + label_h) * len(tiles) + pad),
                      (16, 16, 20))
    draw = ImageDraw.Draw(sheet)
    for i, (stem, want, a, b) in enumerate(tiles):
        y = pad + i * (th + label_h)
        draw.text((pad, y), f"{stem}   fixed {args.fixed_exposure:g}", fill=(200, 200, 210))
        draw.text((pad * 2 + tw, y), f"{args.rule} {want:.3f}", fill=(255, 200, 120))
        sheet.paste(a, (pad, y + label_h))
        sheet.paste(b, (pad * 2 + tw, y + label_h))

    out = pathlib.Path(args.out)
    if not out.is_absolute():
        out = ROOT / out
    out.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(out)
    print(f"left column: one fixed exposure for every scene (what happens today)")
    print(f"right column: each scene at the exposure it asked for")
    print(f"-> {out}")


if __name__ == "__main__":
    main()
