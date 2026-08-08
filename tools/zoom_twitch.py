#!/usr/bin/env python3
"""Measure the per-pixel twitch at an infinite zoom's seam.

    python3 tools/zoom_twitch.py scenes/astral_lattice.toml
    python3 tools/zoom_twitch.py scenes/wellspiral.toml --frames 300 --splat

`zoom_seam.py` is the other half of this and answers a different question. It
measures whether a wrap moves the **picture**, using mean frame brightness, and
it says so in its own docstring:

    Why mean brightness and not per-pixel: the point cloud is sampled, not
    continuous, so two cameras a period apart draw the same *structure* from
    different points. That per-pixel noise is ~40x the signal here and swamps
    a difference image.

That is right about the arithmetic and wrong about which part is the artifact.
The 40x *is* something you can see. A wrap leaves the invariant set alone and
moves the camera, so every dot on screen lands on a different pixel while the
distribution they are drawn from does not change at all. On a dense scene that
is invisible. On a sparse one — where individual points are resolvable — the
whole field of dots is resampled in one frame and it reads as a twitch: no edge
sweeps past, no region switches off, nothing moves in any direction. It is very
hard to name while you are watching it, and it is not a bug in the wrap. It is
the wrap being exactly what it is, seen through a finite number of points.

So this tool measures per-pixel mean absolute difference, and reports three
numbers that together say whether there is anything *else* wrong:

    seam step      the two consecutive frames of the flight that straddle the
                   wrap, found by walking the path and watching the folded
                   camera distance jump (`--info`, no GPU, so this is cheap)
    ordinary step  the median of the same measure between ordinary adjacent
                   frames — what a frame of motion costs when nothing folds
    resample floor the same camera rendered twice from two independent fills
                   of the point buffer. The picture is identical; only which
                   points were drawn differs. This is what "the dots were all
                   replaced" costs on its own, with no camera motion at all.

Read them as a pair of ratios. `seam / ordinary` is how much the seam stands
out from the motion around it. `seam / floor` is the diagnosis:

    ~1.0   the seam is exactly a resample and nothing more. There is no
           further bug to find; the fix, if one is wanted, is about making
           the point sample survive the wrap, not about the wrap.
    >>1.0  something else moves at the seam — a band edge, a density step, a
           haze range — and it is worth bisecting for.

Both frames of every pair come from the same deterministic offline renderer, so
apart from the fill difference the floor is built to have, every difference
reported is signal.
"""

import argparse
import os
import statistics
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from zoom_seam import load_png  # noqa: E402  (same directory, same PNG reader)


def mad(a, b):
    """Mean absolute per-pixel difference of two PNGs, in 0-255 units."""
    wa, ha, ca, pa = load_png(a)
    wb, hb, cb, pb = load_png(b)
    if (wa, ha, ca) != (wb, hb, cb):
        raise ValueError(f"{a} and {b} are different shapes")
    # Alpha carries no light; compare only the colour channels.
    keep = min(ca, 3)
    total, n = 0, 0
    for i in range(0, len(pa), ca):
        for j in range(keep):
            total += abs(pa[i + j] - pb[i + j])
            n += 1
    return total / n


def mean_luma(path):
    w, h, ch, px = load_png(path)
    keep = min(ch, 3)
    total = sum(px[i + j] for i in range(0, len(px), ch) for j in range(keep))
    return total / (keep * w * h)


class Renderer:
    """One still per call, cached on the arguments that change the image."""

    def __init__(self, args, tmp):
        self.args = args
        self.tmp = tmp
        self.n = 0
        self.cache = {}

    def frame(self, t, accumulate=None):
        key = (round(t, 9), accumulate)
        if key in self.cache:
            return self.cache[key]
        out = os.path.join(self.tmp, f"f{self.n:04d}.png")
        self.n += 1
        cmd = [self.args.binary, '--scene', self.args.scene,
               '--path-t', repr(t), '--render', out,
               '--width', str(self.args.width), '--height', str(self.args.height)]
        if self.args.splat:
            cmd.append('--splat')
        if self.args.points:
            cmd += ['--points', str(self.args.points)]
        if accumulate is not None:
            cmd += ['--accumulate', str(accumulate)]
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(f"render failed at t={t}:\n{r.stderr or r.stdout}")
        self.cache[key] = out
        return out


def folded_distance(binary, scene, t):
    """The camera distance `--path-t t` lands on, via --info. No GPU."""
    r = subprocess.run([binary, '--scene', scene, '--path-t', repr(t), '--info'],
                       capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"--info failed at t={t}:\n{r.stderr or r.stdout}")
    seen_camera = False
    for line in r.stdout.splitlines():
        if line.startswith('camera'):
            seen_camera = True
        if seen_camera and line.strip().startswith('distance'):
            return float(line.split()[1])
    sys.exit("--info printed no camera distance")


def zoom_scale(binary, scene):
    """The renormalizing map's contraction ratio, from --info's [zoom] block."""
    r = subprocess.run([binary, '--scene', scene, '--info'],
                       capture_output=True, text=True)
    in_zoom = False
    for line in r.stdout.splitlines():
        if line.startswith('zoom'):
            in_zoom = True
        elif line and not line.startswith(' '):
            in_zoom = False
        if in_zoom and line.strip().startswith('scale'):
            return float(line.split()[1])
    sys.exit(f"{scene} has no usable [zoom]: --info printed no contraction ratio")


def find_seams(binary, scene, frames, scale):
    """Frame indices `i` where the fold fires between frame i-1 and frame i.

    A wrap multiplies the eye's distance from the fixed point by exactly 1/s,
    and it is the only step of a flight that makes the distance go *up* by
    anything like that much. The threshold is halfway there rather than a hair
    above 1, because a path that does not descend — a full orbit, say — holds
    its distance constant and jitters in the last couple of float digits, and
    every one of those would otherwise be reported as a seam.
    """
    d = [folded_distance(binary, scene, i / frames) for i in range(frames)]
    lift = 1.0 + (1.0 / scale - 1.0) * 0.5
    return [i for i in range(frames) if d[i] > d[i - 1] * lift], d


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('scene')
    ap.add_argument('--frames', type=int, default=480,
                    help="frames the loop is watched at (16s at 30fps) [480]")
    ap.add_argument('--controls', type=int, default=8,
                    help="ordinary adjacent pairs sampled for the median [8]")
    ap.add_argument('--width', type=int, default=480)
    ap.add_argument('--height', type=int, default=270)
    ap.add_argument('--splat', action='store_true', help="render with --splat")
    ap.add_argument('--points', type=int, help="point buffer capacity [the scene's]")
    ap.add_argument('--cycle', type=int, default=1000,
                    help="chaos frames between the two fills of the floor pair; "
                         "the buffer turns over in ~800 [1000]")
    ap.add_argument('--binary', default='./target/release/fracturize')
    ap.add_argument('--keep', metavar='DIR', help="keep the frames here")
    args = ap.parse_args()

    print(f"scene         {args.scene}")
    print(f"watching      {args.frames} frames of the loop, {args.width}x{args.height}")

    scale = zoom_scale(args.binary, args.scene)
    seams, dist = find_seams(args.binary, args.scene, args.frames, scale)
    if not seams:
        sys.exit("no wrap in this flight: the folded distance never steps up by "
                 f"anything like 1/{scale:.3f}. The scene has a [zoom], but this "
                 "path does not descend a whole period — a full orbit holds its "
                 "distance, and there is nothing for a wrap to be between.")
    shown = seams if len(seams) <= 6 else seams[:6]
    print(f"wrap at       {', '.join(f't={i / args.frames:.4f} (frame {i})' for i in shown)}"
          + (f", and {len(seams) - 6} more" if len(seams) > 6 else ""))

    tmp = args.keep or tempfile.mkdtemp(prefix='zoom_twitch_')
    os.makedirs(tmp, exist_ok=True)
    r = Renderer(args, tmp)

    def step(i):
        """Per-pixel step between frame i-1 and frame i of the flight."""
        return mad(r.frame((i - 1) % args.frames / args.frames),
                   r.frame(i / args.frames))

    # A steep descent can cross dozens of periods in one flight; six of them
    # say whatever forty-three would.
    seam_steps = [(i, step(i)) for i in shown]

    # Control pairs, spread over the loop and kept clear of every seam.
    far = [i for i in range(args.frames)
           if all(abs(i - s) > 2 for s in seams)]
    picks = [far[len(far) * k // args.controls] for k in range(args.controls)]
    controls = [step(i) for i in picks]
    ordinary = statistics.median(controls)

    # The floor: one camera, two independent fills of the point buffer. Taken
    # at the seam's own framing, so it is the right structure at the right
    # scale and not an easier part of the flight.
    at = seams[0] / args.frames
    floor = mad(r.frame(at, accumulate=32), r.frame(at, accumulate=32 + args.cycle))

    print()
    print(f"ordinary step {ordinary:7.3f}  /255 per pixel, median of {len(controls)}"
          f" adjacent pairs")
    print(f"resample floor{floor:7.3f}  same camera, two independent point fills")
    for i, v in seam_steps:
        print(f"seam step     {v:7.3f}  frames {(i - 1) % args.frames} -> {i}"
              f"   = {v / ordinary:5.2f}x ordinary,  {v / floor:5.2f}x the floor")

    print()
    worst = max(v for _, v in seam_steps)
    if worst / ordinary < 1.25:
        print("the seam costs an ordinary frame of motion. There is nothing there to")
        print("see: the points were carried through the wrap with the camera.")
    elif worst / floor < 1.35:
        print("the seam is a resample and nothing else: it costs what redrawing the")
        print("same structure from a fresh set of points costs, and no more. Every")
        print("dot on screen is replaced in one frame while no light moves, which is")
        print("the twitch. Look at PointCompute::rewrap — either it is not running")
        print("or it is not being given the change in fold depth.")
    else:
        print("something moves at the seam beyond the resample — worth bisecting.")
        print("zoom_seam.py's brightness reading is the first place to look: a real")
        print("edge or density step moves light, and a resample does not.")

    # Mean brightness too, so a run of this can be read against zoom_seam.py
    # without a second render. This is the channel that hides the twitch.
    lum = {i: abs(mean_luma(r.frame((i - 1) % args.frames / args.frames))
                  - mean_luma(r.frame(i / args.frames))) for i, _ in seam_steps}
    lum_ord = statistics.median(
        [abs(mean_luma(r.frame((i - 1) % args.frames / args.frames))
             - mean_luma(r.frame(i / args.frames))) for i in picks])
    print()
    print(f"as brightness {lum_ord:7.3f}  ordinary, and at the seam(s): "
          + ", ".join(f"{v:.3f} ({v / lum_ord:.2f}x)" for v in lum.values()))
    print("brightness is what zoom_seam.py reads; a seam that is only a resample")
    print("moves no light at all and passes there while still being visible.")

    if not args.keep:
        for f in set(r.cache.values()):
            os.unlink(f)
        os.rmdir(tmp)
    else:
        print(f"\nframes kept in {tmp}")


if __name__ == '__main__':
    main()
