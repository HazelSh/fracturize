#!/usr/bin/env python3
"""Turn a reference image into the point cloud an IFS fit would aim at.

Slice 0 of LIKENESS-PLAN.md, as a prototype. The interesting finding this
exists to make visible is that *how* you turn pixels into density is itself an
artistic choice, not a preprocessing detail -- the same reference gives three
unrelated pictures:

    matte      subject vs background, keyed on value+saturation.
               A solid body. The fit chases silhouette.
    luminance  bright pixels are dense. Glows and highlights dominate and the
               body hollows out. The fit chases light, not matter.
    edges      gradient magnitude. A blueprint/wireframe of the subject.
               The fit chases outline and internal seams.

Weighted by a soft subject/setting separation (see `saliency`), importance-
sampled into a point set carrying RGB -- the thing a Wasserstein or Chamfer
loss actually measures against -- and written out as a contact sheet, one row
per image.

    python3 tools/image_to_target.py *.png -o cards-targets.png
    python3 tools/image_to_target.py clarent.png --weight none --map luminance
"""

import argparse

import numpy as np
from PIL import Image
from scipy import ndimage


def luminance(rgb):
    return rgb @ np.array([0.2126, 0.7152, 0.0722])


def matte(rgb, blur=2.0):
    """Subject vs background, keyed on value and saturation.

    Workshop-lit card art is a bright subject on a dark, desaturated ground,
    so value alone nearly does it; saturation rescues coloured-but-dim parts
    of the subject (brass in shadow) from the floor.
    """
    v = rgb.max(axis=-1)
    s = (v - rgb.min(axis=-1)) / np.maximum(v, 1e-6)
    key = np.maximum(v, 0.6 * v + 0.4 * s * v ** 0.5)
    key = ndimage.gaussian_filter(key, blur)
    lo, hi = np.percentile(key, [45, 97])
    return np.clip((key - lo) / max(hi - lo, 1e-6), 0, 1)


def border_distinctiveness(rgb, ring=0.07, bins=6, blur=1.0):
    """How unlike the frame's border colours each pixel is.

    Card art bleeds its setting out to the edges of the art box; the subject
    rarely touches all four. So a colour model built from a border ring is a
    background model, and distance from it is a subject signal -- one that
    does not care whether the subject is bright, which brightness keying does
    and which is wrong for half these cards.
    """
    h, w, _ = rgb.shape
    r = max(int(min(h, w) * ring), 1)
    edge = np.concatenate([
        rgb[:r].reshape(-1, 3), rgb[-r:].reshape(-1, 3),
        rgb[:, :r].reshape(-1, 3), rgb[:, -r:].reshape(-1, 3),
    ])
    idx = np.clip((edge * bins).astype(int), 0, bins - 1)
    hist = np.zeros((bins, bins, bins))
    np.add.at(hist, (idx[:, 0], idx[:, 1], idx[:, 2]), 1.0)
    hist = ndimage.gaussian_filter(hist, blur) / len(edge)

    q = np.clip((rgb * bins).astype(int), 0, bins - 1)
    p = hist[q[..., 0], q[..., 1], q[..., 2]]
    return 1.0 - p / max(p.max(), 1e-9)


def detail_density(rgb, scale=0.06):
    """Local high-frequency energy.

    Illustrators render the subject crisply and let the setting go soft --
    depth of field, atmospheric haze, looser brushwork. That difference
    survives even when the background is the brighter thing.
    """
    g = luminance(rgb)
    gy, gx = np.gradient(g)
    energy = np.hypot(gx, gy)
    k = max(int(min(g.shape) * scale) | 1, 3)
    local = ndimage.uniform_filter(energy, k)
    return np.clip(local / max(np.percentile(local, 97), 1e-9), 0, 1)


def margin_taper(shape, margin=0.025):
    """Fade the outermost band to zero.

    The art box has a painted border, and a border is a very strong edge --
    left in, the fit spends maps drawing a rectangle around the subject. No
    crop is precise enough to avoid this on every card, so taper instead.
    """
    def ramp(n):
        k = max(int(n * margin), 1)
        r = np.ones(n)
        fade = np.linspace(0, 1, k) ** 0.5
        r[:k] = fade
        r[n - k:] = fade[::-1]
        return r
    return ramp(shape[0])[:, None] * ramp(shape[1])[None, :]


def center_prior(shape, sigma=0.42):
    ys = np.linspace(-1, 1, shape[0])[:, None]
    xs = np.linspace(-1, 1, shape[1])[None, :]
    return np.exp(-(xs ** 2 + ys ** 2) / (2 * sigma ** 2))


def saliency(rgb, blur=0.03):
    """A soft subject weight: unlike the border, finely detailed, near centre.

    Deliberately a weight and not a mask. A wrong mask deletes the subject; a
    wrong weight only lets some scenery in, and scenery at low density reads
    as atmosphere rather than as error.
    """
    s = (border_distinctiveness(rgb) ** 1.0
         * detail_density(rgb) ** 0.7
         * center_prior(rgb.shape[:2]) ** 0.6)
    s = ndimage.gaussian_filter(s, max(min(rgb.shape[:2]) * blur, 1.0))
    lo, hi = np.percentile(s, [30, 99])
    return np.clip((s - lo) / max(hi - lo, 1e-9), 0, 1)


def edges(rgb, blur=1.5):
    g = ndimage.gaussian_filter(luminance(rgb), blur)
    gy, gx = np.gradient(g)
    m = np.hypot(gx, gy)
    return np.clip(m / max(np.percentile(m, 99), 1e-6), 0, 1)


def sample(density, rgb, n, gamma, rng):
    """Importance-sample n points, returning (x, y) in [-1, 1] and their RGB.

    Jittered within each source pixel so the cloud does not inherit the
    reference's raster -- a fit against a visibly gridded target learns the
    grid.
    """
    p = np.maximum(density, 0.0).astype(np.float64) ** gamma
    total = p.sum()
    if total <= 0:
        raise SystemExit("density map is empty -- nothing to sample")
    flat = rng.choice(p.size, size=n, p=(p / total).ravel())
    ys, xs = np.unravel_index(flat, p.shape)
    cols = rgb[ys, xs]
    xs = xs + rng.random(n)
    ys = ys + rng.random(n)

    # Centre and scale so the longer axis spans the padded box, matching the
    # convention the fitter normalises its attractor into.
    pts = np.stack([xs, p.shape[0] - ys], axis=1)
    lo, hi = pts.min(axis=0), pts.max(axis=0)
    pts -= (lo + hi) / 2
    return pts / (max(hi - lo) / 2) * 0.75, cols


def splat(pts, cols, size=480):
    """Render the point set in colour, the way the app would.

    Colour is carried on the points rather than recovered afterwards: each
    sample keeps the reference's RGB where it was drawn from. A fit whose
    target is (x, y, r, g, b) can match colour in the same loss that matches
    shape -- sliced Wasserstein in 5D costs exactly what it costs in 2D.
    """
    acc = np.zeros((size, size))
    rgb = np.zeros((size, size, 3))
    ij = ((pts * 0.5 + 0.5) * (size - 1)).astype(int)
    at = (size - 1 - ij[:, 1], ij[:, 0])
    np.add.at(acc, at, 1.0)
    for c in range(3):
        np.add.at(rgb[..., c], at, cols[:, c])

    tone = np.log1p(ndimage.gaussian_filter(acc, 0.7) * 6.0)
    tone /= max(tone.max(), 1e-9)
    mean = rgb / np.maximum(acc, 1e-9)[..., None]
    return np.clip(ndimage.gaussian_filter(mean, (0.7, 0.7, 0)) * tone[..., None] * 1.6, 0, 1)


def to_img(a, size=480):
    """Fit into a square cell without stretching -- these get compared by eye."""
    im = Image.fromarray((np.clip(a, 0, 1) * 255).astype(np.uint8)).convert("RGB")
    im.thumbnail((size, size), Image.LANCZOS)
    cell = Image.new("RGB", (size, size), "black")
    cell.paste(im, ((size - im.width) // 2, (size - im.height) // 2))
    return cell


# Fractional art box of a standard Grand Archive card frame. Good enough to
# batch a folder; override per card where the layout differs (tokens run
# lower, full-art cards have no box at all).
ART_BOX = (0.050, 0.122, 0.950, 0.660)


def art_crop(im, spec=None):
    w, h = im.size
    if spec:
        return im.crop(tuple(int(v) for v in spec.split(",")))
    l, t, r, b = ART_BOX
    return im.crop((int(l * w), int(t * h), int(r * w), int(b * h)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("images", nargs="+")
    ap.add_argument("--crop", help="left,top,right,bottom in source pixels "
                                   "(default: the standard art box)")
    ap.add_argument("--map", default="edges", choices=["matte", "luminance", "edges"])
    ap.add_argument("--weight", default="saliency",
                    choices=["saliency", "none"],
                    help="how the subject is separated from the setting")
    ap.add_argument("--points", type=int, default=60000)
    ap.add_argument("--gamma", type=float, default=1.6,
                    help="density contrast; >1 concentrates on dense regions")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("-o", "--out", default="targets.png")
    args = ap.parse_args()

    rng = np.random.default_rng(args.seed)
    cell = 420
    sheet = Image.new("RGB", (cell * 4, cell * len(args.images)), "black")

    for row, path in enumerate(args.images):
        rgb = np.asarray(art_crop(Image.open(path).convert("RGB"), args.crop),
                         dtype=np.float64) / 255.0
        density = {"matte": matte, "luminance": luminance, "edges": edges}[args.map](rgb)

        w = saliency(rgb) if args.weight == "saliency" else np.ones(rgb.shape[:2])

        target = density * w * margin_taper(rgb.shape[:2])
        pts, cols = sample(target, rgb, args.points, args.gamma, rng)

        for col, cellimg in enumerate([rgb, w, target, splat(pts, cols)]):
            sheet.paste(to_img(cellimg, cell), (col * cell, row * cell))
        print(f"{path}: {args.map} x {args.weight}, "
              f"subject weight covers {(w > 0.5).mean():.0%} of the art box")

    sheet.save(args.out)
    print(f"wrote {args.out}  (columns: art, subject weight, target density, sampled cloud)")


if __name__ == "__main__":
    main()
