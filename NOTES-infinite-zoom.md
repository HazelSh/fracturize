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

## Four things I got wrong before anyone looked at it

**Levels meant periods.** I first wrote `levels` as "how many zoom periods of
scale to render". A period is however big the chosen map's contraction happens
to be — 0.07 octaves for a 0.95 spiral, 3.3 for a 0.1 collapse. So the same
`levels = 12` covered a hundredth of the frame's dynamic range in one scene and
forty times it in the next, and half the scenes I tried came out as dust. It
now means octaves, converted per map. **Any parameter whose meaning depends on
the data is a bug wearing a number.**

**Equal points per octave — which I "fixed", wrongly.** Octave *k* is a copy at
scale `sᵏ` covering `sᵏ` of the frame, so a flat deal looked like it was
spending most of the buffer on specks around the fixed point, and I gave it an
`s^(2k)` geometric falloff. That reasoning optimises an even-looking *still*,
and this feature's entire purpose is the flight. A wrap moves the octave
filling the screen along by one, so unequal octaves make the density jump every
period. Flat is forced. Measured, and reverted, in the section below — I record
it here in the order I believed it, because the mistake wasn't the arithmetic,
it was optimising the wrong picture.

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

## The fifth thing I got wrong, found by someone looking at the output

Hazel watched the first zoom animation and said sections of the fractal were
visibly cutting out while still on screen. They were. It is worth writing down
how I found it, because I was wrong twice on the way and the second time I was
wrong *while holding the right answer*.

The first guess was the band's outer edge. I tested it by rendering one still
at a fixed framing with three different `radius` values and diffing them — and
the extra radius added nothing, so I dropped the idea. **That test could not
have detected the bug.** At a fixed framing the edge is either in the picture
or it isn't; the failure only exists at the *moment of a wrap*, where the
required extent jumps. Testing a dynamic fault with a static probe.

Then I chased three innocents. Near-plane clipping: no, tested by rebuilding
with `Z_NEAR` at 0.002, output identical to the byte. Point size popping across
the wrap: a real-sounding argument that was simply void, because at these
scales the renderer is on the 1px point-primitive path and `point_size` does
not affect screen size at all. The AV1 encoder crushing a soft region: no, the
fault reproduces in the raw renders.

What did find it was measuring instead of arguing: render two stills 0.14%
apart in distance straddling the wrap threshold, and the same 0.14% move
somewhere else in the band as a control. Straddle 8.41/255, control 2.64. A
3.2× excess, reproducible, and something to bisect against.

That let me sweep parameters honestly. `levels` changed nothing at all — the
inner end was innocent too. `octave_falloff` moved it monotonically (1.9× at 0,
3.2× at 2, 4.8× at 3), which is a genuine second bug: a wrap moves the octave
filling the screen along by one, so if neighbouring octaves hold different
numbers of points the density jumps every period. Flat weighting is *forced*,
not preferred, and I had reasoned my way into a falloff by optimising the wrong
thing — an even-looking still, in a feature whose entire purpose is the flight.

But flat weighting didn't fix the blinking. The radius did — the thing I had
already guessed and cleared on bad evidence. A wrap multiplies the eye's
distance from the fixed point by `1/s`, so the distance at which the frustum
wants material multiplies by `1/s` too, while the outer edge stays where it is.
Everything in that shell is dropped in a single frame. It doesn't read as an
edge sweeping past, which is what I was looking for; it reads as a region
switching off.

And the bound is derivable rather than tunable, which is the part that annoys
me most about having shipped a guess. The eye is at most `band` from the fixed
point and haze has finished hiding things by `FAR_FRAC · band`, so

```
    radius ≥ (1 + FAR_FRAC) · band = 2.42 · band
```

I shipped 1.2 for `wellspiral` and a 1.5 default. Both under. The lesson isn't
"test more", it's narrower and more useful: **a static test cannot falsify a
dynamic hypothesis, and clearing a hypothesis with the wrong instrument is
worse than never having had it** — I spent the next three experiments not
looking at the answer because I believed I had already ruled it out.

`MIN_RADIUS` is now derived from `haze::FAR_FRAC` in code, three tests pin it,
and a band below it is reported by name everywhere a zoom is described.

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

## Closing the loop (Hazel asked, and it was already true)

> Would it be possible to link camera paths up to themselves for infinite zoom,
> and to be able to render an animation that loops for an infinite zoom?

Yes, and — this is the pleasant part — *exactly*, not approximately. A looping
video of a zoom is normally a cheat: you cross-fade, or you find a near-repeat
and hope. Here the repeat is a theorem. `S∞` is invariant under `A`, so a path
whose last key is its first key carried forward by `A` ends on a frame that is
not similar to the first frame but **is** it.

So a loop needs no extra machinery, only a different way of closing. A normal
closed path returns to where it started; a zoom loop keeps going and *arrives*
where it started, one period down. `path_zoom_loop = N` in the scene, and the
whole implementation is one branch in `CameraPath::key`: for out-of-range
spline indices, return the in-range key carried by the symmetry rather than
clamped or wrapped.

Two things fell out that I didn't plan.

**One keypoint is enough, and is better than two.** With the out-of-range keys
generated by the symmetry, the four keys the spline sees are `A⁻¹k, k, Ak, A²k`
— and log-distance and yaw across those are *arithmetic sequences*. Catmull-Rom
through equally spaced collinear points is exactly linear. So a one-key zoom
loop is a constant-rate descent with no ease, no wobble, and no velocity kink
at the seam, for free. A two-key path would have needed the ends clamped, which
is precisely what puts a speed discontinuity at a loop seam.

*Update, after the quaternion rework:* this got stronger, and is now exact in
orientation rather than only in yaw. The spline is cumulative Catmull-Rom over
displacements, and for a one-key loop every segment displacement is the same
element `q⁻¹·rot·q`. The cumulative weights sum to `1+u` identically, so the
whole product telescopes to `q(u) = rot^(1+u)·q`: the spline *is* the
continuous similarity flow, not an approximation of it.

That also fixed a real bug the old version hid. The yaw-space spline could only
carry the *vertical component* of the map's twist, which is exact for a map
that turns about the vertical and wrong for one that doesn't.
`pythagoras-zoomy` twists 47.02° about `(0.117, -0.282, -0.952)` — an axis that
is mostly Z — so the loop swept the camera −13.27° of yaw where 47.02° of turn
was needed to close it. It drifted a little every pass and nothing said so. It
now closes to 1.1e-4 on a radius of 4.35.

**The seam needs no special case in the renderer.** `sample` already wraps `t`
for looping paths, so t=1 lands on t=0 — a different *camera* from the one the
path was heading toward, but the identical *picture*, and `Renorm::wrap` was
already folding cameras between those two descriptions every period. The
feature and the precision fix turn out to be the same fact used twice.

Measured on `wellspiral`, 168 frames: the last-frame-to-first-frame step is
8.08/255 against a median adjacent step of 7.20 — **1.12×**. The excess is the
irreducible difference between two point samples of the same structure, the
same floor I hit measuring the wrap. There is no seam to find.

I made one real mistake building this, and it was the same one twice: I paired
the partner point with the wrong camera in the seam test, exactly as I had in
`wrapping_the_camera_does_not_move_the_picture` a few hours earlier. The rule
is `screen(S(x), S(C)) == screen(x, C)` — the transformed point goes with the
transformed camera. Writing it down here in the hope of not doing it a third
time.

## The floor wasn't a floor

*Added 2026-08-07, after Hazel watched `astral_lattice` loop and said something
jumped at the seam without being able to say what.*

Two sections up I measured a zoom loop's last-frame-to-first-frame step at
1.12× the median adjacent step and wrote: **"The excess is the irreducible
difference between two point samples of the same structure... There is no seam
to find."** The first half is a correct description of what the number is. The
second half does not follow from it, and I want to be precise about the gap,
because I made the same move twice: `tools/zoom_seam.py` opens by explaining
that per-pixel noise across a wrap is ~40× the signal and must be averaged out
to see anything. Both times I identified the resample, and both times I filed
it as the instrument's problem rather than the picture's.

It is the picture's. A wrap leaves `S∞` alone and moves the camera by `A⁻¹`.
That is exact for the *set* and not for a **sample** of it: the dots do not
move, so every one of them lands on a different pixel. Nothing enters, nothing
leaves, no light moves — mean frame brightness is flat across the seam, which
is why the brightness instrument passes — and the entire dot field is
nonetheless replaced in a single frame. Whether you can see that is a question
about the scene, not about the wrap. On a dense attractor the dots are not
individually resolvable and there is nothing to notice. On a sparse one they
are, and it reads as a twitch: no direction, no edge, no region switching off,
just everything being very slightly *different* for one frame. Hazel's
description was "hard to pin down even for me", and that is exactly the
signature of a change with no direction in it.

The fix is one line of algebra and about forty of shader. `screen(A⁻¹x, A⁻¹C)
== screen(x, C)`, so carry the point buffer by the same `A⁻¹` the camera took
and every dot keeps its pixel. What that cannot do alone is stay bounded —
carrying camera *and* points is the wrap undone, and the precision it exists to
save goes with it. So the deal is re-folded: a point carried off the outer end
of the band comes back at the inner end, which costs the octave assignment
rotating by one and moves only the outermost octave — the one `edge_guard` has
already taken to nothing. `rewrap` in `points/chaos.wgsl`. Written as a single
power rather than a shift plus a correction, which bounds it inside one band's
worth however far the wrap jumped:

```
m = idx − ((idx − turns) mod n)
```

Measured with `tools/zoom_twitch.py`, which is `zoom_seam.py` with the
averaging taken out and two references added — an ordinary frame of motion, and
one camera drawn from two independent fills of the buffer, which is what a pure
resample costs:

```
                 seam   ordinary   floor    before → after
astral_lattice   4.22 → 2.26   2.29   4.20     1.85× → 0.99× ordinary
wellspiral      15.10 → 9.91   9.96  13.83     1.52× → 1.00× ordinary
```

Before the carry, both seams sat on the resample floor to within a few percent
— which is the diagnosis stated as a number, and is why I trust it: the seam
did not merely resemble a resample, it cost exactly one.

Three things I'd want a future me to take from this rather than the fix.

**"Every difference is signal" and "this difference is noise" cannot both be
true of the same instrument.** `zoom_seam.py` says the first in its docstring
and does the second in its code. The moment a tool has to discard a channel to
see its signal, the discarded channel is a hypothesis, not a nuisance — and
it should be measured, not averaged. The 40× was sitting in that file for
months with its size written down.

**The seam lands on the loop boundary for a structural reason, and I'd have
found it faster by asking why.** `band` is the scene's authored camera
distance, and a one-key zoom loop puts its only key at exactly that distance.
So the fold fires on the first frame after `t` rolls over, on every scene
authored the ordinary way — `astral_lattice`, `wellspiral` and `rimefall` all
report the wrap at frame 1. It is not a coincidence that the twitch is where
the loop closes; it is the same number twice.

**A still renderer that folds the camera has to carry the buffer too.** Not for
the still, where it is invisible — it permutes which point sits on which octave
of a band that is unchanged either way — but because two stills either side of
a fold are otherwise not two frames of one flight, and anything measuring a
seam is comparing exactly those. `--path-t` exists so a frame of a flight can
be named at all (the descent twists as it scales, so stepping `--distance`
walks off the path), and `effective_camera_folded` is why a run of them is a
run of frames.

## Three ways not to spread the carry

*Added 2026-08-07. Hazel asked whether the carry pass could be done ahead of
time — "keeping some points in the new coords, some in the old, update with the
fizz, then swap them, double-buffer style". It is the right instinct and I
talked myself into agreeing before I checked it. Writing down what the check
said, because two of these are attractive enough to be proposed again.*

The pass costs **0.93 ms on an 8M buffer** (0.242 ms at 2M — linear, and at the
memory floor: 256 MB of streaming read and write). Writing 12 bytes instead of
the full 16-byte point measured 0.934 against 0.925, i.e. nothing; it is the
same cache line. So the only lever is *when* it happens, not how fast it is.

**Doing it at draw time instead.** This one nearly worked, and the fact behind
it is worth keeping even though the design isn't: the per-point carry looks
per-octave but collapses to **two similarities split at one radius**. With
`t = turns mod n` the power is `t` for every octave at or inside `t` and `t − n`
for the rest, two values however deep the zoom has gone. So the renderer could
do the whole carry with a `length`, a compare and a `mat3` — on a point it is
already reading, with no extra memory traffic and no buffer write at all.
Measured: **0.85 ms per frame**, against 0.93 ms per *period*. About 960× the
total work for a flat profile. Branchless `select` was slightly worse (0.365 s
vs 0.350 s over 48 frames), so it is the arithmetic, not divergence. Reverted.

**Migrating early, uncompensated — the "hide it in the fizz" one.** This is the
one I agreed to before checking, and it does not work at all. The requirement
is not "the point ends up in the right place"; it is that a point's screen
position is unchanged *across the wrap frame boundary*, and that is measured
from wherever the point is **at the wrap instant**. Move it early and you have
moved the goalposts with it: at the wrap it still needs another `A⁻¹`. So a
pre-migrated point twitches when it migrates *and* again at the wrap, the
wrap's own twitch is undiminished, and the scheme is strictly worse than doing
nothing. Three lines of numeric check would have caught it:

```
carried at the wrap:            screen before [0.174, 0.087]  after [0.174, 0.087]
pre-migrated, not carried:      screen before [0.425, 0.172]  after [0.174, 0.087]
```

**Migrating early, compensated.** Fixes the above — the renderer undoes the
early move for display until the swap — but the compensation is the draw-time
carry above, charged on whatever fraction has already moved. That fraction
grows to the whole buffer, so the cost *peaks at the full 0.85 ms* just before
the swap, which is what it was trying to avoid. Short windows don't help: the
peak is set by the split reaching 100%, not by how long it takes to get there.
A genuine second buffer does avoid it — that is what "double-buffer" properly
means here — at the price of doubling the largest allocation in the renderer
(128 MB at 8M points), on the one thing this renderer is actually short of.

And then the measurement that should have come first. On the reference desktop
the pass is **not detectable in frame times at all**: 129 wraps, vsynced at
120 Hz, 3.9% of wrap frames missed a refresh against 2.7% of ordinary ones —
and the same comparison with the pass taking its own `queue.submit` instead of
riding the frame's encoder gave 3.1% against 2.6%, i.e. the two are the same
and both are the ambient rate. Ordinary frames on this renderer have a p95 of
11.5 ms against an 8.3 ms refresh; 0.93 ms is well inside that. **A cost you
cannot measure is not a cost you can optimise**, and I went three designs deep
before checking whether the thing I was removing showed up anywhere.

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
