#!/usr/bin/env python3
"""Emit a Mandelbox-like scene: the inverse branches of the Mandelbox map as an IFS.

Not a Mandelbox. The Mandelbox people know is escape-time and Mandelbrot-type
(c = z0), which no IFS draws. What an IFS *can* draw is the Julia-type set of
one map, by inverse iteration, the way flam3's `julia` draws z^2 + c. For

    g(z) = s * spherefold(boxfold(z))        (c = 0)

every inverse branch is three pieces fracturize already has:

    A^-1  pre-affine scale 1/s
    S^-1  the sphere fold's three preimages: y/4, inversion, identity
          -> `linear = 0.25`, `spherical = 1`, `linear = 1`
    B^-1  the box fold's three per axis: x, 2 - x, -2 - x
          -> a post-translation in {0, +-2}^3 (with a reflection)

which is 3 x 27 = 81 maps. With c = 0, g commutes with the cube group Oh, so
`Octa` with `mirror = true` over 3 x 4 motifs (translation classes core,
face, edge, corner) has the same attractor: the group supplies the
reflections and the sign of s.

Each branch is only *valid* on part of space (y/4 below radius 2, the
inversion between 1 and 2, ...), and used everywhere the branches draw fog
and streaks. Xaos can't condition on position, but it can condition on the
last map, and the last map says roughly where the point is. So this runs the
proper restricted inverse iteration in numpy, always choosing a valid
branch, counts which motif class follows which, and writes those transition
frequencies as the xaos matrix. The validity conditions depend only on the
radius and on |coordinates|, so they commute with the group and the classes
mean the same thing under it.

The real chain drains into corner-pass (the box's outer frame), which on the
GPU, where walkers persist, would swallow the picture. --leak gives that row
weak exits; it is the one deliberate dose of invalid material.

    python3 tools/mandelbox_ifs.py --scale 3 > scenes/foldbox.toml
"""

import argparse

import numpy as np

CLASSES = [("core", (0, 0, 0)), ("face", (2, 0, 0)), ("edge", (2, 2, 0)), ("corner", (2, 2, 2))]
BRANCHES = [("quarter", "linear = 0.25"), ("invert", "spherical = 1.0"), ("pass", "linear = 1.0")]
COLOURS = {"quarter": [1.0, 0.55, 0.2], "invert": [0.3, 0.7, 1.0], "pass": [0.92, 0.86, 0.74]}
SHADE = {"core": 1.0, "face": 0.85, "edge": 0.7, "corner": 0.55}


def transitions(s, walkers, steps, seed):
    """Motif-class transition probabilities of the restricted inverse iteration."""
    rng = np.random.default_rng(seed)
    z = rng.uniform(-2, 2, (walkers, 3))
    prev = np.full(walkers, -1)
    counts = np.zeros((12, 12))
    for step in range(steps):
        y = z / s
        r = np.linalg.norm(y, axis=1)
        # which sphere-fold preimages are valid: y/4, inversion, identity
        ok = np.stack([r < 2, (r > 1) & (r < 2), r >= 1], 1).astype(float)
        k = (rng.random(walkers)[:, None] * ok.sum(1, keepdims=True) >= np.cumsum(ok, 1)).sum(1)
        r2 = (y * y).sum(1, keepdims=True)
        u = np.where(k[:, None] == 0, y / 4, np.where(k[:, None] == 1, y / r2, y))
        # and the box fold's, per axis: x, 2 - x, -2 - x
        okb = np.stack([np.abs(u) <= 1, u <= 1, u >= -1], 2).astype(float)
        b = (rng.random((walkers, 3))[..., None] * okb.sum(2, keepdims=True) >= np.cumsum(okb, 2)).sum(2)
        z = np.where(b == 0, u, np.where(b == 1, 2 - u, -2 - u))
        cls = k * 4 + (b != 0).sum(1)
        if step > 10:
            seen = prev >= 0
            np.add.at(counts, (prev[seen], cls[seen]), 1)
        prev = cls
    return counts / np.maximum(counts.sum(1, keepdims=True), 1)


def scene(s, leak, matrix):
    names = [f"{c}-{b}" for b, _ in BRANCHES for c, _ in CLASSES]
    m = np.where(matrix > 0.002, matrix, 0.0)
    m = np.round(m / np.maximum(m.max(1, keepdims=True), 1e-9), 3)
    corner_pass = names.index("corner-pass")
    m[corner_pass] = leak * (m[names.index("edge-invert")] > 0)
    m[corner_pass, corner_pass] = 1.0
    quoted = ", ".join(f'"{n}"' for n in names)
    rows = ",\n".join("  [" + ", ".join(f"{x:g}" for x in row) + "]" for row in m)
    out = [
        f"""# Foldbox: a Mandelbox-like set, drawn as an IFS. Not a Mandelbox.
#
# Written by tools/mandelbox_ifs.py --scale {s:g} --leak {leak:g}, whose docstring
# has the derivation. In short: these are the inverse branches of
# g(z) = {s:g} * spherefold(boxfold(z)), so the attractor approximates g's
# Julia-type set the way flam3's `julia` approximates z^2 + c. Each motif is
# one branch: pre-scale 1/{s:g}, then the sphere fold's preimage as a
# variation (quarter = y/4, invert = inversion, pass = identity), then the box
# fold's as a post-translation (core, face, edge, corner = how many axes it
# folds). The cube group supplies the other 69 of the 81 branches.
#
# The xaos matrix is measured, not authored: the transition frequencies of
# the proper inverse iteration, which picks only branches valid where the
# point is. It stands in for the position test an IFS can't make, and it is
# what turns a fog into a box. corner-pass's weak exits ({leak:g}) are the one
# departure from it, so the walk doesn't drain into the frame.
#
# Points draws an opaque cube with notched, scalloped edges; splat and
# volume show the lattice of quarter-branch rosettes inside it.
#
#   fracturize -s scenes/foldbox.toml
#   fracturize -s scenes/foldbox.toml --splat

[meta]
name = "Foldbox"
author = "Claude Opus 5.5"
point_size = 0.004
point_count = 8_000_000
color_mode = "mix"
color_speed = 0.6
background = [0.008, 0.008, 0.014]

[camera]
focus = [0.0, 0.0, 0.0]
distance = 13.0
yaw = 0.5
pitch = 0.45

[[symmetry]]
group = "Octa"
mirror = true
applies_to = [{quoted}]

[xaos]
maps = [{quoted}]
matrix = [
{rows}
]
"""
    ]
    for b, var in BRANCHES:
        for c, t in CLASSES:
            colour = [round(x * SHADE[c], 3) for x in COLOURS[b]]
            out.append(
                f"""
[[transform]]
name = "{c}-{b}"
translation = [0.0, 0.0, 0.0]
scale = {1 / s:.6f}
variations = {{ {var} }}
post_translation = [{t[0]:.1f}, {t[1]:.1f}, {t[2]:.1f}]
color = {colour}
"""
            )
    return "".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--scale", type=float, default=3.0, help="Mandelbox scale |s| [3]")
    ap.add_argument("--leak", type=float, default=0.05, help="corner-pass exit multiplier [0.05]")
    ap.add_argument("--walkers", type=int, default=200_000)
    ap.add_argument("--steps", type=int, default=80)
    ap.add_argument("--seed", type=int, default=0)
    a = ap.parse_args()
    print(scene(a.scale, a.leak, transitions(abs(a.scale), a.walkers, a.steps, a.seed)), end="")


if __name__ == "__main__":
    main()
