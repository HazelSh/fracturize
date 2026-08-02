# Notes on making an IFS infinite

*Claude Opus 5, 2026-08-02. Working notes from building `[zoom]` — the
reasoning that didn't fit in code comments, the things I got wrong first, and
what I think is worth doing next. `AGENTS.md` has the how; this is the why and
the residue.*

---

## The thing todo.txt was actually asking for

> fractals that are 'lines' across all scales, not rays. can zoom into
> mandelbrot forever, but not out.
> fixed-point 'structure' with no privileged scale? any point in/out
> self-similar to other points in/out

That's a precise request and it has a clean answer, which surprised me. The
answer is that **the object already exists** — every IFS with one invertible
contracting affine map in it generates one, and generating it costs a `round()`
per point. You do not have to design a special kind of fractal. You have to
stop truncating the one you have.

Here is the whole argument in five lines. `S` is the attractor, `f` one of the
maps, `p` its fixed point.

```
S ⊇ f(S)                              because S = ⋃ fᵢ(S)
f⁻¹(S) ⊇ S                            apply f⁻¹ to both sides
f⁻ᵐ(S) is increasing in m             induction
S∞ := ⋃ f⁻ᵐ(S)                        the limit
f⁻¹(S∞) = S∞                          shifting an increasing union by one
```

The last line is the whole feature. `S∞` is *exactly* invariant under a
similarity — not approximately, not statistically. Scale it by `s` about `p`,
rotate by `f`'s rotation, and it is not a similar set, it is **the same set**.

I want to be clear about what is and isn't surprising here. That an IFS
attractor is self-similar is the definition of one. What's less obvious is that
the *inverse* direction is available too and costs nothing: the same maps that
built the thing downward will build it upward, and the union is a genuinely
unbounded, genuinely scale-free object. It has no biggest feature. Asking "how
big is it" has no answer.

## The bit I expected to be hard and wasn't

I braced for the density problem. Zooming into a fractal is normally miserable
because the chaos game samples the natural measure, so a window a thousand
times smaller gets a thousandth of the points and you either accept a fading
picture or start doing rejection sampling, which is worse.

Renormalization dissolves it. Given a chaos point at radius `r` from `p`, the
integer

```
m = round( log(R/r) / log(1/s) )
```

is exactly how many times to apply `f⁻¹` to move it onto the band you're
looking at. Not "approximately" — `f⁻ᵐ(x) ∈ S∞` for every integer `m` and every
`x ∈ S`, so *every* point is a legal point of the object at *every* scale. There
is no rejection because there is nothing to reject. A point that fell a
thousand levels deep isn't a wasted sample, it's a sample of a smaller copy,
and one `round()` says which one.

I keep turning this over because it feels like it shouldn't be free. I think
the honest account is: the difficulty was never sampling, it was that the
object was the wrong object. A bounded attractor genuinely doesn't have
material at every scale in every window. `S∞` does, and it costs the same to
draw.

## The bit I didn't expect to be hard and was

**Precision.** Zoom forty octaves and f32 has nothing left; the points near the
fixed point are quantization noise. I spent a while thinking about f64 point
buffers, or a per-frame recentre, and both are bad — the first doubles memory
on a renderer whose whole constraint is memory, the second reintroduces the
regeneration cost the renormalization just removed.

The answer came from taking the invariance seriously. If `A(S∞) = S∞`, then
zoom is a **symmetry**, so it's *periodic*. The camera never has to leave one
period. When the eye drops through the inner edge of the band, step the whole
camera — eye, focus and up together — by `A⁻¹`, and it lands at the outer edge
looking at a pixel-identical picture. Increment a counter. Nothing else moves.

Nothing gets small, so no precision is spent, and the point buffer never needs
regenerating. **The wrap is the level-of-detail system.** That's my favourite
part of this: the thing that makes it correct is the same thing that makes it
cheap, and both are the same one-line fact about an increasing union.

The test I care most about is `wrapping_the_camera_does_not_move_the_picture`,
which asserts the seamlessness rather than asserting some proxy for it: after a
wrap, every point projects to the pixel its image under `A` occupied before.

## Four things I got wrong

**Levels meant periods.** I first wrote `levels` as "how many zoom periods of
scale to render". A period is however big the chosen map's contraction happens
to be — 0.07 octaves for a 0.95 spiral, 3.3 for a 0.1 collapse. So the same
`levels = 12` covered a hundredth of the frame's dynamic range in one scene and
forty times it in the next, and half the scenes I tried came out as dust. It
now means octaves, converted per map. **Any parameter whose meaning depends on
the data is a bug wearing a number.**

**Equal points per octave.** Also wrong, for a related reason: octave *k* is a
copy at scale `sᵏ` covering `sᵏ` of the frame, so a flat deal spends most of
the buffer on specks around the fixed point. It wants `s^(2k)`, which is one
inverted geometric CDF and lets twelve octaves cost about what three did.

**Iterating the matrix.** I clamped `Aᵏ` at 48 multiplies. Fine for `s = 0.5`,
useless for `s = 0.95`, which needs hundreds — and the gentle maps are exactly
the ones that look best, because a small period reads as continuous motion
rather than a series of steps. A similarity has a closed-form power (`s⁻ᵏ`, and
`k` times the rotation angle), so it's O(1) and the clamp is gone. Only the
anisotropic fallback iterates.

**A counter that ran away.** The status bar read `zoom +6882` in the first live
test. A camera path key is deliberately unwrapped — that's how it descends nine
periods in one interpolation — so the sample arrives outside the band *every
frame* and the wrap's return value is the absolute depth of that sample, not a
step. I was adding it. This is the one bug of the four I'd call a real bug, and
it only showed up because I ran the actual window instead of trusting the
headless renders. Worth remembering.

## What it can't do, said plainly

The zoom is infinite **toward `p`**. Fly off sideways and you leave the band
and get the ordinary bounded attractor back. Every self-similar zoom has a
centre, and this one's is the fixed point of the map you nominated — there is
no version of this where you can zoom forever toward an arbitrary point,
because an arbitrary point isn't a symmetry of anything.

Anisotropic maps still generate an exactly invariant set — self-*affine*
rather than self-similar — but the camera wrap can't reproduce a non-uniform
scale, so those show a seam. `Renorm::defect` measures it and everything that
can say so does. I considered refusing them and decided against it: the
rendered object is correct and often interesting, and the only broken part is
one interaction. Better to ship it flagged than to withhold it.

And not every scene makes a good one. A map with a gentle contraction (0.4-0.8)
gives a band with many visible octaves. A 0.1 collapse puts a factor of ten
between neighbouring shells and reads as dust — correctly, but nobody wants
that. `--info` now lists every eligible map with its period, which is the fix:
you can see before rendering whether a scene has a good symmetry in it.

## Where I'd go next

**Zoom toward a point that isn't the fixed point.** The fixed points of `f`
conjugated by any word in the other maps — `g f g⁻¹` for `g` a composition —
are also exact symmetry centres, and there are countably many of them densely
spread through the attractor. That would turn "zoom into *this* bit" into a
real operation: click a feature, find the nearest conjugate fixed point, zoom
there forever. The maths is already sitting in `renorm.rs`; what's missing is
the search, and a way to pick `g` so the conjugated map is still contracting
and still roughly a similarity.

**Two symmetries at once.** `scenes/bicameral.toml` has two eligible maps and
you pick one at a time. If `f` and `g` are both symmetries of the same set, the
group they generate acts on it, and there's presumably a two-parameter family
of zooms — a *surface* of self-similarity rather than a line. I don't know what
that looks like and I'd like to.

**Zoom for the nonlinear variations.** The current requirement is pure affine,
because `f⁻¹` has to exist in closed form. `spherical` is its own inverse.
`bulb` is radius-preserving and its angle map inverts to within a branch
choice. `boxfold` inverts piecewise. There's a real question about which of the
twenty are invertible enough to renormalize with, and a scale symmetry that
inverts through a sphere would be a genuinely new kind of picture — Droste in
an inversive geometry rather than a similarity one.

**Occlusion, still.** todo.txt is right that the splats need to show structure
and haze only half does it. But under infinite zoom this gets *worse* and more
interesting at once: the object extends toward the camera without bound, so
"what's in front" stops being a question about the fractal and becomes a
question about the band. I don't have a proposal. I mention it because the
renormalization changes the shape of that problem and I'd rather that be on the
record than rediscovered.

## On the pictures

`scenes/ammonite.toml` is the one I'd keep. A logarithmic spiral is the only
curve that looks the same at every scale, which is why a nautilus grows by
adding chambers instead of inflating — each chamber is the last one scaled and
turned, so the animal never redesigns itself. Real shells stop, because animals
stop. It was a good feeling to render one that doesn't.

`scenes/pythagoras.toml` is Bosman's 1942 figure, drawn by hand under
occupation, with fourteen degrees of yaw added and the trunk taken away. Both
of its branch maps are similarities, so `--zoom left` gives a Pythagoras tree
with no first square and no last one. I'd like to think he'd have wanted to see
that.
