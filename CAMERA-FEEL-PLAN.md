# Plan: view controls that don't care where the camera has been

*Claude Fable 5, 2026-08-05. Prerequisites landed in `d82a0df`: the spline no
longer lets a neighbouring winding roll a segment, multi-key zoom loops fly
their keys, and the roll field can't scramble a pole framing.*

> **Status: §§1–7 implemented (Claude Opus 5, 2026-08-05).** `OrbitStyle` is a
> pref defaulting to `Trackball`, `OrbitCamera::orbit` branches on it, and the
> Camera window has the radio. `Route::Exact` carries a per-segment `route`
> rotvec from scene files, checked on load. What remains is step 3 below —
> living with it before adding any further affordance.

---

## 0. The goal, in Hazel's words

Drop into an arbitrary scene, or step off the camera path at an arbitrary
point, and not have to care what the camera was doing beforehand in order to
use the view controls. Looking around with the mouse should feel the same no
matter what's been going on.

## 1. Why today's orbit can't deliver that, no matter how it's tuned

Horizontal drag currently yaws about **world Y** — the turntable. That is a
deliberate anchor: it keeps the horizon level while you orbit. But it means
the on-screen effect of a horizontal drag depends on the angle between world
Y and wherever history has pointed your view axis:

- Looking out level, world-Y yaw pans the scene sideways. Feels like yaw.
- Looking near straight down (or up), world Y nearly *is* the view axis, so
  the same drag spins the image in its plane. Feels like roll. This is the
  "pitch a few turns, then yawing also rolls" report: after multi-turn
  pitching you're near a pole without knowing it, because a fractal has no
  horizon to tell you.
- Upside down (pitch residual between 90° and 270°), the drag works mirrored.

None of that is numerical — the composition was verified exact to microradians
over thousands of drag events. It is the chosen geometry: **a turntable's feel
is a function of your orientation relative to its privileged axis**, and
history decides that orientation. The goal above is precisely "feel must not
be a function of history", so the privileged axis has to go.

It was already a poor fit here anyway. World Y means nothing to these scenes:
there is no ground plane, and every zoom wrap composes the map's twist — an
arbitrary-axis rotation — into the framing. The one axis the turntable
protects is one the app itself doesn't respect.

## 2. The design: screen-space orbit

Define every look control in the **camera's own frame** — the screen's frame:

| control          | today                    | planned                  |
|------------------|--------------------------|--------------------------|
| horizontal drag  | world-Y yaw (turntable)  | **body-Y yaw**           |
| vertical drag    | body-X pitch             | body-X pitch (unchanged) |
| right-drag       | body-Z roll              | body-Z roll (unchanged)  |
| pan / zoom       | body axes / distance     | unchanged                |

Body-frame operations compose on the right: the drag's effect relative to the
screen is *literally the same rotation for every starting orientation*.
History-independence isn't approximated, it's structural — which is why this
is the whole design and not a heuristic (see §5 for the rejected ones).

Concretely, `OrbitCamera::orbit` becomes, in trackball style:

```rust
self.orientation = self
    .orientation
    .then_body(Turn::about(Vec3::Y, -dx * DRAG_RATE))
    .then_body(Turn::about(Vec3::X, dy * DRAG_RATE));
```

(the yaw line is the only change: `then_world` → `then_body`).

## 3. The price, stated before it's paid

**No horizon invariant.** Rotations about different axes don't commute, so a
closed loop of drags (right, up, left, down) returns you looking the same way
but slightly rolled. Every trackball does this; it is a theorem, not a bug:
*"drag effect is always screen-relative" and "horizon stays level" cannot both
hold.* The turntable chose the second; this chooses the first, because the
first is the stated goal — and because "level" already means little in scenes
whose own symmetry tilts the world every wrap.

What keeps the price payable, all of it already in the tree:

- the **roll readout and field** in the Camera window show what you've
  accumulated, exactly;
- the **`level` button** removes it in one click, without moving the eye;
- **right-drag roll** lets you set any horizon deliberately.

No auto-leveling, ever: easing roll back after a drag would make the controls
fight the user and would reintroduce history-dependence through the back door.

## 4. Turntable stays available as a preference

`Prefs` gains `orbit_style: OrbitStyle` (`Trackball | Turntable`), persisted
and defaulted to `Trackball`, same pattern as `invert_pitch`. One radio in the
Camera window's controls row (use `ui/radio.rs` — this is a one-of-n).
`OrbitCamera::orbit` takes the style as a parameter; the app passes it from
prefs at the call site (`app.rs`, `Drag::Orbit`).

Existing scenes and paths are untouched — this is a *control* change only. The
default turntable **path** (`CameraPath::full_orbit`) keeps flying world-Y
circles; a path is a shot, not a feel.

## 5. Rejected alternatives

- **Hemisphere flip** (negate yaw when `up·Y < 0`): fixes mirrored-when-
  inverted, does nothing about roll-feel at the poles, keeps the history
  dependence. A patch on the wrong geometry.
- **Elevation blend** (turntable when level, trackball near poles): the feel
  now *varies with state*, which is the complaint itself, plus a tuning
  parameter nobody can name a right value for.
- **Auto-relevel on release**: see §3.
- **Clamp pitch again**: the framings past the pole are the point; some shots
  need them.

## 6. Folded in: routes a winding cannot say (review bug #3)

Screen-space controls make "any framing at all" normal, so authored segments
between arbitrary framings get more common, and the route model needs one
extension. Today a segment's route is `turns: i32` — whole extra turns about
the axis the endpoints already imply. Two things it cannot say:

- **A loop in place about a chosen axis.** Equal endpoints force the axis to
  be guessed (world Y), so "pitch three full turns and return" silently
  becomes yaw turns, or nothing.
- **A corkscrew about any axis the endpoints don't imply.** The winding's
  axis is always the short way's axis.

Plan: scene files may optionally give a segment `route = [x, y, z]` — a
rotation vector (same form `rotvec` already uses for framings), taking
priority over `turns` on that segment. **Validated at load**: `exp(route)`
must land on the segment's far key within tolerance, else a loader error
naming the key and the miss in degrees. That keeps the property the `i32` was
chosen for — a stored route that disagrees with its endpoints is *detectable*
— while lifting the expressiveness ceiling. `turns` remains the spelling for
everything people write by hand today; no UI for `route` initially (it is a
file-format door, and the panel keeps showing turns).

Loader: `scene.rs` (`PathKeyDef` gains `route`; windings resolution prefers
it). Spline: `segment_turn` returns the authored `route` directly where
present — `sample()` already handles arbitrary-magnitude turns correctly
since the winding/principal split landed.

## 7. Tests

- **Feel-invariance, the theorem as a test**: for a dozen random orientations
  `q`, `q⁻¹ ∘ orbit(dx,dy)(q)` is the *same* turn — the drag's screen-space
  effect doesn't depend on the framing. (This is the spec; it fails for the
  turntable at any nonzero pitch.)
- **Exact inverse**: drag right then equally left returns bit-close to start,
  from any framing — including upside down and at the poles.
- **Turntable pref pins the old arithmetic**: the existing
  `orbiting_at_zero_roll_is_the_old_angle_arithmetic` test runs against
  `Turntable` style; a sibling documents that `Trackball` at zero
  pitch-and-roll agrees with it (they coincide exactly when level).
- **Route validation**: a `route` that misses its endpoint is a load error
  that names the segment; a pitch-loop-in-place `route` flies pitch, not yaw.
- Optional end-to-end: drive a drag sequence through the X11 XTest harness
  and assert the readouts, per the existing input-testing setup.

## 8. Order of work

1. ~~`OrbitStyle` pref + `orbit()` branch + radio + tests (§2, §4, §7).~~ Done.
2. ~~`route` in scene files + loader validation + tests (§6).~~ Done. Landed as
   `path::Route`, an enum over the two ways a segment's route can be named,
   replacing the bare `windings: Vec<i32>` — one field, so a winding and a
   stored displacement can't both claim the same segment.
3. Live with the trackball default for a few sessions before considering any
   further affordance (e.g. a keybind for `level`) — not before.
