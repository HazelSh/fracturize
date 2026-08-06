#!/usr/bin/env python3
"""Measure whether an infinite-zoom wrap is seamless, offline.

    python3 tools/zoom_seam.py scenes/octave-edge-visual.toml
    python3 tools/zoom_seam.py scenes/octave-edge-visual.toml --guard 1

Steps the camera down through two zoom periods and compares consecutive
frames. Because `--distance` is folded back into the canonical period before
anything renders, the frames either side of a wrap are ordinary stills: no
animation, no interpolation, and the offline renderer is deterministic, so
every difference is signal.

Read the *wrap step*, as a multiple of an ordinary frame step. A seamless wrap
is ~0; a hard band edge shows up as a multiple of it. On
`scenes/octave-edge-visual.toml`, which is built to have the problem:

    edge_guard = 0      11.9x        (the artifact, once per period)
    edge_guard = 1       0.0x        (the guard; see renorm.rs)

Why mean brightness and not per-pixel: the point cloud is sampled, not
continuous, so two cameras a period apart draw the same *structure* from
different points. That per-pixel noise is ~40x the signal here and swamps a
difference image. It cancels in the mean, which is what the wrap actually
moves — the picture dimming as an octave leaves.

Why not an animation render: `--render out.mp4` on a `path_zoom_loop` covers
exactly one period and wraps the camera every frame, so the seam always comes
out as one ordinary frame step whatever the band is doing. That check reports
1.04x on a band short enough to make `--info` print BAND TOO SHORT.
"""

import argparse
import os
import statistics
import struct
import subprocess
import sys
import tempfile
import zlib


def load_png(path):
    """Decode an 8-bit PNG to (width, height, channels, bytes)."""
    d = open(path, 'rb').read()
    if d[:8] != b'\x89PNG\r\n\x1a\n':
        raise ValueError(f"{path} is not a PNG")
    i, idat, w, h, bd, ct = 8, b'', None, None, None, None
    while i < len(d):
        ln = struct.unpack('>I', d[i:i + 4])[0]
        typ, data = d[i + 4:i + 8], d[i + 8:i + 8 + ln]
        i += 12 + ln
        if typ == b'IHDR':
            w, h, bd, ct = struct.unpack('>IIBB', data[:10])
        elif typ == b'IDAT':
            idat += data
    if bd != 8:
        raise ValueError(f"{path}: only 8-bit PNGs, got {bd}")
    raw = zlib.decompress(idat)
    ch = {0: 1, 2: 3, 4: 2, 6: 4}[ct]
    stride = w * ch
    out, prev, pos = bytearray(), bytearray(stride), 0
    for _ in range(h):
        f = raw[pos]
        pos += 1
        line = bytearray(raw[pos:pos + stride])
        pos += stride
        if f:
            for x in range(stride):
                a = line[x - ch] if x >= ch else 0
                b = prev[x]
                c = prev[x - ch] if x >= ch else 0
                if f == 1:
                    line[x] = (line[x] + a) & 255
                elif f == 2:
                    line[x] = (line[x] + b) & 255
                elif f == 3:
                    line[x] = (line[x] + (a + b) // 2) & 255
                elif f == 4:
                    p = a + b - c
                    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                    pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                    line[x] = (line[x] + pr) & 255
        out += line
        prev = line
    return w, h, ch, bytes(out)


def mean_luma(path):
    w, h, ch, px = load_png(path)
    total = sum(px[i * ch] + px[i * ch + 1] + px[i * ch + 2] for i in range(w * h))
    return total / (3 * w * h * 255.0)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('scene')
    ap.add_argument('--guard', type=float, default=None,
                    help="override [zoom] edge_guard (0 = hard edge)")
    ap.add_argument('--distance', type=float, default=None,
                    help="camera distance to start from [the scene's]")
    ap.add_argument('--periods', type=int, default=2, help="zoom periods to walk")
    ap.add_argument('--per-period', type=int, default=16, help="frames per period")
    ap.add_argument('--width', type=int, default=480)
    ap.add_argument('--height', type=int, default=270)
    ap.add_argument('--binary', default='./target/release/fracturize')
    ap.add_argument('--keep', metavar='DIR', help="keep the frames here")
    args = ap.parse_args()

    start = args.distance
    if start is None:
        # The scene's own [camera] distance, read without a TOML library
        for line in open(args.scene):
            line = line.strip()
            if line.startswith('distance') and '=' in line:
                start = float(line.split('=', 1)[1].split('#')[0])
                break
    if start is None:
        sys.exit("no [camera] distance in the scene; pass --distance")

    # One period per `--per-period` frames, so the wrap lands on a known index:
    # frame k has distance start * 2^(-k/per_period) and the wrap happens
    # whenever the folded camera jumps from the band's inner edge to its outer.
    n = args.periods * args.per_period + 1
    tmp = args.keep or tempfile.mkdtemp(prefix='zoom_seam_')
    os.makedirs(tmp, exist_ok=True)

    frames = []
    for k in range(n):
        d = start * 2.0 ** (-k / args.per_period)
        out = os.path.join(tmp, f'f{k:03d}.png')
        cmd = [args.binary, '--scene', args.scene, '--distance', repr(d),
               '--render', out, '--width', str(args.width), '--height', str(args.height)]
        if args.guard is not None:
            cmd[1:1] = ['--set', f'zoom.edge_guard={args.guard}']
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(f"render failed at frame {k}:\n{r.stderr or r.stdout}")
        frames.append(out)
        print(f"\rrendering {k + 1}/{n}", end='', flush=True)
    print()

    means = [mean_luma(f) for f in frames]
    steps = [abs(b - a) for a, b in zip(means, means[1:])]
    # The wrap fires between the frame whose distance reaches the band's inner
    # edge and the next one, i.e. every `per_period` frames starting at 0.
    wraps = list(range(0, len(steps), args.per_period))
    ordinary = statistics.median([v for i, v in enumerate(steps) if i not in wraps])

    print(f"scene         {args.scene}")
    if args.guard is not None:
        print(f"edge_guard    {args.guard}")
    print(f"frames        {n} over {args.periods} period(s)")
    print(f"ordinary step {ordinary:.5f} (mean frame brightness, 0-1)")
    for i in wraps:
        print(f"wrap step     {steps[i]:.5f}  = {steps[i] / ordinary:5.1f}x "
              f"an ordinary frame (frames {i} -> {i + 1})")
    if not args.keep:
        for f in frames:
            os.unlink(f)
        os.rmdir(tmp)


if __name__ == '__main__':
    main()
