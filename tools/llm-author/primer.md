# Fracturize: a primer for scene authors

Fracturize draws 3D fractals. You write a small TOML file, a **scene**, and the
program renders it. You are not drawing a picture: you are writing down a *rule*,
and the picture is what that rule settles into.

## How the picture is made

A scene is a list of **transforms** (maps). Each one takes a point in 3D space
and moves it: scale it, rotate it, translate it, then optionally bend it through
nonlinear **variations**. Thousands of points each pick a random transform,
apply it, and repeat, millions of times. Where the points pile up is the fractal:
a shape made of smaller copies of itself, one copy per transform.

So every transform is applied at every scale, everywhere. Moving one transform a
little changes the whole image. That is the medium: steering, not drawing.

## An example scene

```toml
[meta]
name = "Pearl"
author = "Your model name"
point_size = 0.002        # world-space dot size; 0.001-0.003
background = [0.02, 0.01, 0.04]   # linear RGB, keep it dark
color_speed = 0.5         # 0-1: how strongly each map pulls the colour

[camera]
distance = 2.4            # how far away the eye is; 1.5-6
yaw = 0.8                 # turn around the vertical axis, radians
pitch = 0.5               # look down from above, radians

[[transform]]
name = "core"
translation = [0.0, 0.0, 0.0]
scale = 0.72
rotation = [24.0, 11.0, 0.0]   # degrees about x, y, z
color = [0.65, 0.3, 0.75]      # RGB 0-1
weight = 7.0                   # how often this map is chosen
variations = { bulb = 1.0 }

[[transform]]
name = "seed"
translation = [0.6, 0.0, 0.0]
scale = 0.32
color = [0.95, 0.8, 0.35]
```

Tables go in this order: `[meta]`, `[camera]`, `[palette]`, `[[symmetry]]`,
then two or more `[[transform]]`. Keys go in the order shown.

## Every key you can use

**[meta]**: `name`, `author` and `point_size` are required. Optional:
`background`, `color_mode` (`"transforms"` or `"mix"`), `color_speed`,
`color_falloff` (0.3-1.0, colour by scale; leave out if unsure),
`color_contrast` (1-4), `haze` (0-1, fades distant points; 0.2-0.4 helps depth),
`exposure` (0.5-3, brightness).

**[camera]**: `distance`, then `yaw` and `pitch` (radians; both required),
and optionally `focus` (the point looked at, default `[0, 0, 0]`).

**[palette]**: `name = "..."` recolours the scene with a gradient instead of the
transform colours. One of: ember, lava, sunset, copper, autumn, sodium, ocean,
ice, cobalt, jade, fern, amethyst, bloom, peacock, neon, spectrum, ash,
cyanotype, sepia, abyss.

**[[transform]]**: `translation` is required. Give each map a `color` too;
left out, it gets a hue spaced around the wheel. Optional: `name`,
`scale` (one number, or `[x, y, z]` per axis), `rotation` (degrees), `weight`
(default 1), `color_speed` (overrides meta), `variations`.

**[[symmetry]]**: `group`, `axis`, `applies_to`. Copies the named transforms
around a rotation group, so the picture is exactly symmetric.
`group = "Cyc5"` is 5-fold around `axis` (Cyc2 to Cyc24); `"Dih5"` adds a flip;
`"Tetra"` (12 copies), `"Octa"` (24) and `"Icosa"` (60) are polyhedral and take
no axis. `applies_to = ["petal"]` names transforms.

## Variations

`variations = { swirl = 0.3, linear = 0.7 }` blends bends by weight. Leave it
out for a plain map (`linear = 1.0`). Weights around 1 in total are safest.

- **Truly 3D:** `linear`, `sinusoidal`, `fisheye`, `bubble`, `bulb`, `absfold`,
  `boxfold`, `spherefold`, `spherical`.
- **Sheet-makers** (bend x and y, leave z alone, so they make flat layers):
  `swirl`, `julia`, `disc`, `polar`, `spiral`, `hyperbolic`, `diamond`,
  `horseshoe`, `bent`, `cylinder`, `tangent`. A few degrees of tilt in
  `rotation` gives the layers relief.
- **Sprays fuzz everywhere:** `spherical`. Keep it under 0.2.
- `bulb`: pearly nested shells. `absfold`: crystal facets (0.1-0.2 is plenty).
  `boxfold` / `spherefold` only act beyond distance 1 from the centre, so use
  them on maps with scale above 1 and a separate small map to hold things in.

## First, give it room to exist

Use at least two maps, and give them **different translations**. If every map
leaves the same point where it is, the whole picture collapses onto that one
point. Two unrotated maps make only a straight line. The report says if either
has happened.

**If it collapses, turn one map and move the maps apart.** This is the fix
that works, and it is better than adding maps or variations. Give one map a
real turn, 30-90°, about an axis that crosses the line (a turn about the line
itself changes nothing), and translate the maps well away from the origin and
from each other. Two plain `linear` maps treated this way, each at a scale of
about 0.7 (the table below), already grow into 3D dragon-curve forms; at 0.5
they stay a sparse scatter. The warning below about small tilts is for adding depth
to a form you already have, not for rescuing a collapsed one.

## Numbers that predict the look

**Size of the maps decides lace versus lump.** The total amount of material
depends on how many maps there are and how big each is. For N maps of equal
scale s, the look is:

| N maps | lacy, see-through | shell or surface | solid blob |
|---|---|---|---|
| 2 | 0.68 | 0.74 | 0.8 |
| 3 | 0.54 | 0.63 | 0.7 |
| 4 | 0.46 | 0.55 | 0.63 |
| 6 | 0.37 | 0.47 | 0.55 |
| 8 | 0.31 | 0.41 | 0.5 |

A symmetry group multiplies N: one map under `Cyc5` counts as 5, under `Icosa`
as 60, so those maps want small scales (around 0.45 for Cyc5, 0.16 for Icosa)
or thin per-axis scales like `[0.1, 0.4, 0.1]`. If the picture is a blob, shrink
the scales. You do not need a new idea.

Keep every scale below 1 unless a variation brings points back.

**Weight is share, and share is colour.** A map with weight 0.1 among maps of
weight 1 gets a small share of the points, so its colour barely shows. To see a
hue, give its map share. Weights only matter relative to each other: raising
every weight together changes nothing, and it does not add points.

**Small doses work best:**
- Tilt out of plane, on a form that already works: 5-12° reads as 3D. 25° or
  more dissolves into fuzz.
- A thin "stem" map (like `scale = [0.05, 0.6, 0.05]`) at 1-2% of the weight
  can hold a plant-like form up. More than that and its streaks take over.
- Symmetry alone looks like wallpaper. One extra map *outside* the group, at a
  few percent of the weight, is what makes it a picture.

**Colour.** Keep `background` dark and tinted (deep indigo, slate, teal,
sepia), in linear RGB, so 0.02 is already a visible dark colour. Raise
`color_speed` (0.6-0.8) to keep each branch its own colour; lower it (0.3) to
blend. `color_mode = "mix"` blends the transform colours as RGB, so paths
through different maps get different colours. `color_contrast` does nothing in
`mix` mode; leave it out there.

## What you will get back

After each scene you get a short report:

- **notes**: problems found, or `none`. Read this first.
- **shape**: where the fractal sits, and suggested values for `camera.distance`
  and `point_size`. Copy them.
- **dimension**, with a word for it: under 1.7 `dust`, then `lace` (to 2.1),
  `shell` (to 2.6), `solid` (to 3) and `overfull` (3 or more). `overfull`
  means too much material: shrink the scales, don't grow them. A delicate
  idea wants lace or shell; `solid` is a lump, whatever it was meant to be.
- **an image**, if you can see: four views of your scene from around it.

A broken file gets the loader's error instead; fix it and resend the scene.
