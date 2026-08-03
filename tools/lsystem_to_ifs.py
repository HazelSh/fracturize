#!/usr/bin/env python3
"""Port a bracketed 3D L-system production to fracturize IFS transforms.

An L-system production like  X -> F[+X][-X]F^X  is read with a 3D turtle:

    F        draw a trunk segment (one step forward along local +Y)
    X        place a scaled copy of the whole system at the current pose
    + / -    yaw   left/right   (around local Z)
    ^ / &    pitch up/down      (around local X)
    / / \\   roll  cw/ccw       (around local Y)
    [ / ]    push / pop turtle state

Each X becomes an IFS map (translation + rotation + uniform scale): the
attractor re-appears at every placement, which is exactly the L-system's
recursion. Each F becomes a "trunk" map with per-axis scale that squashes
the whole attractor onto that segment, making the skeleton visible (the
classic IFS-tree trick; needs fracturize's scale = [x, y, z] support).

--depth N expands the system to all length-N words over the maps before
emitting, which leaves the attractor unchanged but multiplies the transform
count (stress testing) -- trunk maps terminate a word (nothing after a
squash contributes new geometry at visible scale), branch maps extend it.
"""

import argparse
import math
import numpy as np


def rot(axis, deg):
    a = math.radians(deg)
    c, s = math.cos(a), math.sin(a)
    m = np.eye(3)
    i, j = {"x": (1, 2), "y": (2, 0), "z": (0, 1)}[axis]
    m[i, i], m[i, j], m[j, i], m[j, j] = c, -s, s, c
    return m


class Map:
    """pos + orient (3x3) + per-axis scale in local frame: p -> pos + R @ (S * p)"""

    def __init__(self, pos, orient, scale, kind, color, weight):
        self.pos = np.asarray(pos, float)
        self.orient = np.asarray(orient, float)
        self.scale = np.asarray(scale, float)  # per-axis, applied in local frame
        self.kind = kind  # "branch" | "trunk"
        self.color = color
        self.weight = weight

    def compose(self, other):
        """self ∘ other. Only valid when self.scale is uniform."""
        s = self.scale[0]
        assert np.allclose(self.scale, s), "can only compose after a uniform map"
        return Map(
            self.pos + self.orient @ (s * other.pos),
            self.orient @ other.orient,
            s * other.scale,
            other.kind,
            [(a + b) / 2 for a, b in zip(self.color, other.color)],
            self.weight * other.weight,
        )

    def euler_xyz_deg(self):
        """Decompose orient to XYZ Euler (matching glam EulerRot::XYZ: R = Rx Ry Rz)."""
        m = self.orient
        # R = Rx(a) Ry(b) Rz(c);  m[0,2] = sin(b)
        b = math.asin(max(-1.0, min(1.0, m[0, 2])))
        if abs(math.cos(b)) > 1e-6:
            a = math.atan2(-m[1, 2], m[2, 2])
            c = math.atan2(-m[0, 1], m[0, 0])
        else:  # gimbal lock
            a = math.atan2(m[2, 1], m[1, 1])
            c = 0.0
        return [math.degrees(v) for v in (a, b, c)]

    def is_chartable(self):
        """Whether XYZ euler can name this rotation without going degenerate.

        Looking along the branch axis, `b` approaches +/-90 deg and the other
        two angles collapse into one control: the numbers stay finite and stop
        meaning anything individually. An L-system that turns hard enough hits
        this routinely, which is why `rotvec` exists.
        """
        return abs(math.cos(math.asin(max(-1.0, min(1.0, self.orient[0, 2]))))) > 0.01

    def rotvec(self):
        """Decompose orient to a rotation vector (axis * angle, radians).

        The exact form fracturize reads from `rotvec = [x, y, z]`: no chart, no
        poles, and it round-trips whatever the rotation is.
        """
        m = self.orient
        cos_t = max(-1.0, min(1.0, (m.trace() - 1.0) / 2.0))
        theta = math.acos(cos_t)
        if theta < 1e-8:
            return [0.0, 0.0, 0.0]
        if abs(math.pi - theta) < 1e-6:
            # Half a turn: the skew part vanishes, so read the axis off
            # (R + I)/2, whose columns are all parallel to it.
            k = max(range(3), key=lambda i: m[i, i])
            axis = [(m[i, k] + (1.0 if i == k else 0.0)) for i in range(3)]
            n = math.sqrt(sum(v * v for v in axis)) or 1.0
            return [v / n * theta for v in axis]
        s = 2.0 * math.sin(theta)
        return [
            (m[2, 1] - m[1, 2]) / s * theta,
            (m[0, 2] - m[2, 0]) / s * theta,
            (m[1, 0] - m[0, 1]) / s * theta,
        ]


def turtle_maps(production, angle, step, branch_scale, trunk_width, colors):
    pos = np.zeros(3)
    orient = np.eye(3)
    stack = []
    maps = []
    n_branch = 0
    turns = {
        "+": ("z", 1), "-": ("z", -1),
        "^": ("x", 1), "&": ("x", -1),
        "/": ("y", 1), "\\": ("y", -1),
    }
    for ch in production:
        if ch == "F":
            # squash the unit-tall attractor onto this segment
            mid = pos + orient @ np.array([0.0, step / 2, 0.0])
            maps.append(Map(mid - orient @ np.array([0.0, step / 2, 0.0]), orient,
                            [trunk_width, step, trunk_width], "trunk",
                            colors["trunk"], 1.0))
            pos = pos + orient @ np.array([0.0, step, 0.0])
        elif ch == "X":
            c = colors["branches"][n_branch % len(colors["branches"])]
            maps.append(Map(pos.copy(), orient.copy(),
                            [branch_scale] * 3, "branch", c, 1.0))
            n_branch += 1
        elif ch in turns:
            axis, sign = turns[ch]
            orient = orient @ rot(axis, sign * angle)
        elif ch == "[":
            stack.append((pos.copy(), orient.copy()))
        elif ch == "]":
            pos, orient = stack.pop()
        else:
            raise ValueError(f"unknown symbol {ch!r}")
    return maps


def expand(maps, depth):
    """All words: (branch)^(depth-1) . (any), plus shorter words ending in trunk."""
    words = list(maps)
    for _ in range(depth - 1):
        nxt = []
        for w in words:
            if w.kind == "trunk":
                nxt.append(w)  # trunk terminates: keep as-is
            else:
                for m in maps:
                    nxt.append(w.compose(m))
        words = nxt
    return words


def emit(maps, name, args):
    total_w = sum(m.weight for m in maps)
    lines = [
        "[meta]",
        f'name = "{name}"',
        'author = "Claude Fable 5 (L-system port)"',
        f"point_size = {args.point_size}",
        f"point_count = {args.point_count}",
        "color_falloff = 0.8",
        "color_contrast = 1.5",
        "",
        "[camera]",
        "focus = [0.0, 0.9, 0.0]",
        "distance = 3.2",
        "pitch = 0.15",
        "",
    ]
    for m in maps:
        e = m.euler_xyz_deg()
        s = m.scale
        scale = (round(s[0], 4) if np.allclose(s, s[0])
                 else [round(v, 4) for v in s])
        # trunk maps get lower weight: they only draw skeleton, and every
        # walker that lands there stops producing fine detail
        w = m.weight * (args.trunk_weight if m.kind == "trunk" else 1.0)
        lines += [
            "[[transform]]",
            f'name = "{m.kind}"',
            f"translation = [{', '.join(f'{v:.4f}' for v in m.pos)}]",
            f"scale = {scale}",
            # Degrees while they mean something; the exact rotation vector
            # where the euler chart has gone degenerate.
            (
                f"rotation = [{', '.join(f'{v:.2f}' for v in e)}]"
                if m.is_chartable()
                else f"rotvec = [{', '.join(f'{v:.6f}' for v in m.rotvec())}]"
            ),
            f"color = [{', '.join(f'{v:.3f}' for v in m.color)}]",
            f"weight = {w / total_w:.5f}",
            "",
        ]
    return "\n".join(lines)


# Preset design note: every recursive X inside brackets needs an
# out-of-plane turn (^ & / \) somewhere on its path. If a subset of branch
# maps preserves a plane (e.g. only +/- yaws), that subset's sub-attractor
# is a dense 2D fern pressed flat against it — the original birch preset
# ("F[+X]F[-X]^X") shipped that bug: 30% of all points landed within 0.01
# of the z=0 plane, a hard wall when viewed edge-on.
PRESETS = {
    # name: (production, angle, step, branch_scale)
    "bush3d": ("F[+X][-X][^X][&X]/X", 28, 0.5, 0.55),
    "birch": ("F[^+X]F[&-X]/X", 22, 0.45, 0.6),
    "kelp": ("F[/+X][\\-X]FX", 25, 0.4, 0.62),
}

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("preset", choices=PRESETS, nargs="?", default="bush3d")
    ap.add_argument("--production")
    ap.add_argument("--angle", type=float)
    ap.add_argument("--depth", type=int, default=1)
    ap.add_argument("--trunk-width", type=float, default=0.06)
    ap.add_argument("--trunk-weight", type=float, default=0.35)
    ap.add_argument("--point-size", type=float, default=0.004)
    ap.add_argument("--point-count", type=int, default=6_000_000)
    ap.add_argument("-o", "--out", required=True)
    args = ap.parse_args()

    production, angle, step, branch_scale = PRESETS[args.preset]
    if args.production:
        production = args.production
    if args.angle is not None:
        angle = args.angle

    colors = {
        "trunk": [0.45, 0.3, 0.15],
        "branches": [[0.3, 0.75, 0.25], [0.55, 0.85, 0.3],
                     [0.2, 0.6, 0.35], [0.75, 0.8, 0.3], [0.4, 0.9, 0.5]],
    }
    maps = turtle_maps(production, angle, step, branch_scale, args.trunk_width, colors)
    maps = expand(maps, args.depth)
    name = f"{args.preset} d{args.depth} ({len(maps)} maps)"
    with open(args.out, "w") as f:
        f.write(emit(maps, name, args))
    branches = sum(1 for m in maps if m.kind == "branch")
    print(f"{args.out}: {len(maps)} transforms ({branches} branch, "
          f"{len(maps) - branches} trunk) from '{production}' depth {args.depth}")
