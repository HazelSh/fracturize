// Chaos game compute shader with flame-style variations
// Writes points directly to a circular buffer - no hash grid, no view-space projection
//
// Each transform applies its affine matrix first, then blends the result
// through a weighted sum of nonlinear "variations" (Apophysis/flam3 style,
// generalized to 3D). A pure { linear = 1 } transform reproduces the classic
// affine IFS behavior exactly.

struct Point {
    position: vec3<f32>,
    color_idx: u32,
}

// Must match GpuTransform in buffers.rs (256 bytes)
struct Transform {
    matrix: mat4x4<f32>,       // pre-affine (before variations)
    post_affine: mat4x4<f32>,  // post-affine (after variations)
    color_value: f32,
    weight: f32,
    cumulative_weight: f32,
    color_speed: f32,
    // Variation weights; slot order matches scene::VARIATION_NAMES
    var_weights: array<vec4<f32>, 5>,
    // This transform's own colour, linear RGB (ColorMode::Mix only). vec3 is
    // 16-aligned, so it lands at offset 224 and the three scalars after it run
    // 236..248, putting the struct at 256.
    color_rgb: vec3<f32>,
    // This transform's symmetry group: where it starts in `group_elements` and
    // how many elements it has. Zero count = no symmetry.
    group_start: u32,
    group_count: u32,
    // 1 when the colormap index is offset by the drawn group element.
    orbit_color: f32,
}

// The walker carries colour two ways at once, and they cost the same because
// both fit in space that was already padding:
//
//   current_color   a scalar position in the colormap (transforms / palette)
//   current_rgb     a 3-vector that genuinely mixes (mix mode)
//
// The scalar can order structure but never label it: two walkers that arrived
// by completely different routes collide on the same index whenever their
// EMAs happen to land together. Mixing three channels instead of one makes a
// walker that came via a red map and then a blue one *purple*, and
// distinguishable from one that came via two magenta maps — distinct
// transform *combinations* get distinct colours, which is the thing 1-D
// indexing structurally cannot do.
//
// `current_rgb` occupies what were `_pad1..3`, so this costs no memory at all
// on a renderer whose entire constraint is VRAM.
struct WalkerState {
    current_pos: vec3<f32>,
    max_point_pixels: f32,
    current_color: f32,
    current_rgb_r: f32,
    current_rgb_g: f32,
    current_rgb_b: f32,
    rng_state: vec4<u32>,
    // How far this walker's current point would move if the transform under
    // the cursor were nudged — as a *vector*, because it has to be carried
    // through the maps that come after, and only a vector can be. See the
    // emphasis block in `main`. Zero, and untouched, unless the emphasis is on.
    tangent: vec3<f32>,
    _pad_tangent: f32,
}

// Must match PointComputeParams in buffers.rs (208 bytes)
struct ComputeParams {
    num_transforms: u32,
    num_walkers: u32,
    iterations_per_walker: u32,
    write_offset: u32,
    buffer_capacity: u32,
    // Infinite-zoom renormalization; see renorm.rs. zoom_enabled = 0 for
    // ordinary scenes, and then none of the rest is read.
    zoom_enabled: u32,
    zoom_levels: f32,
    zoom_log_scale: f32,
    zoom_octave_q: f32,
    zoom_similar: u32,
    zoom_scale: f32,
    // Periods to carry the buffer through, read only by `rewrap` (0 for the
    // chaos pass, which must never move a point that is already placed).
    zoom_rewrap: f32,
    // Threads per row of the `rewrap` dispatch, so it can be 2D. A 100M point
    // buffer is 390,625 workgroups of 256, and a dispatch dimension stops at
    // 65,535 — see `rewrap_in`. Read by nothing else.
    rewrap_stride: u32,
    // Point emphasis: 1 while the render is lighting up the material that
    // `emphasis_target` moves, and the circular run of the point buffer that
    // has been written since that target was chosen. Outside the run the
    // sensitivity values belong to a different transform, so nothing — the
    // stats pass or the glow pass — may read them. See src/gpu/points/emphasis.rs.
    emphasis_enabled: u32,
    emphasis_target: u32,
    emphasis_run_start: u32,
    emphasis_run_len: u32,
    // Seventeen scalars, so three words of padding before the vec4s. The band's
    // outer edge is faded at *render* time now (see guard_weight() in
    // points/splat.wgsl); it cannot be done here, because this buffer is
    // filled over ~800 frames and a camera-dependent deal would mix that many
    // camera positions into one image.
    // Two of the three padding words carry the layer table (subscenes; see
    // src/subscene.rs): how many layers, and whether the scene composes them.
    num_layers: u32,
    composite: u32,
    // `--isolate`: 0 is off; k + 1 keeps only points whose last map was slot k.
    // The rest go to ISOLATE_DROPPED, which every point shader clips.
    isolate: u32,
    zoom_fixed: vec4<f32>,      // xyz = fixed point, w = target radius
    zoom_axis_angle: vec4<f32>, // xyz = rotation axis of A, w = its angle
    zoom_a: mat3x3<f32>,
    zoom_a_inv: mat3x3<f32>,
}

// Where `--isolate` sends a point it drops. Far past anything real — a walker
// past 1e6 is re-seeded — and tested for by `isolate_dropped` in render.wgsl
// and splat.wgsl, which must agree with this.
const ISOLATE_DROPPED: f32 = 1.0e37;

@group(0) @binding(0) var<storage, read_write> points: array<Point>;
@group(0) @binding(1) var<storage, read> transforms: array<Transform>;
@group(0) @binding(2) var<storage, read_write> walker_states: array<WalkerState>;
@group(0) @binding(3) var<uniform> params: ComputeParams;
// Symmetry group elements: every distinct group in the scene, concatenated.
// A transform's own run is `group_start .. group_start + group_count`, and
// element 0 of any run is the identity. See src/symmetry.rs.
@group(0) @binding(4) var<storage, read> group_elements: array<mat4x4<f32>>;
// Per-point emphasis: |tangent| at the moment the point was written, parallel
// to `points`. A sixteen-byte stand-in is bound here while the emphasis is
// off, so every write to it is behind `params.emphasis_enabled`.
@group(0) @binding(5) var<storage, read_write> sensitivity: array<f32>;
// [0] = EMA of the mean sensitivity over the run, [1] = 1 once that mean has
// been seeded. Written by `emphasis_stats`, read by the glow pass, which needs
// a reference to normalize against and cannot have one until the points exist.
@group(0) @binding(6) var<storage, read_write> stats: array<f32>;

// One layer of the scene: its own maps, or one subscene's. Must match GpuLayer
// in src/gpu/points/compute.rs (96 bytes). A walker belongs to the layer whose
// `walker_end` first exceeds its id, and only ever draws that layer's maps —
// which is what makes a composite the union of its subscenes' attractors
// rather than one IFS whose maps mix. An ordinary scene is one layer holding
// every map and every walker.
struct Layer {
    placement: mat4x4<f32>,
    t_start: u32,
    t_count: u32,
    walker_end: u32,
    mix: u32,
    contrast: f32,
    scale: f32,
    _pad0: f32,
    _pad1: f32,
}
const MAX_LAYERS: u32 = 16u;
@group(0) @binding(7) var<uniform> layers: array<Layer, 16>;
// 256 entries per layer, layer k's at k * 256. Read only in a composite.
@group(0) @binding(8) var<storage, read> layer_colormaps: array<vec4<f32>>;

const PI: f32 = 3.14159265358979;

// xorshift128 random number generator
fn xorshift128(s: ptr<function, vec4<u32>>) -> u32 {
    var t = (*s).w;
    let x = (*s).x;
    (*s).w = (*s).z;
    (*s).z = (*s).y;
    (*s).y = x;
    t ^= t << 11u;
    t ^= t >> 8u;
    (*s).x = t ^ x ^ (x >> 19u);
    return (*s).x;
}

fn rand_float(s: ptr<function, vec4<u32>>) -> f32 {
    return f32(xorshift128(s)) / 4294967296.0;
}

// Binary search over the cumulative weights: first i with r < cum_weight[i].
// Scenes are typically <10 transforms, but generated ones (L-system ports)
// reach thousands, where a linear scan dominates the whole chaos pass.
// Searches one layer's run of the buffer: its cumulative weights are its own.
fn select_transform(r: f32, start: u32, count: u32) -> u32 {
    var lo = start;
    var hi = start + count - 1u;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if r < transforms[mid].cumulative_weight {
            hi = mid;
        } else {
            lo = mid + 1u;
        }
    }
    return lo;
}

// Fetch a variation weight by slot index from the packed vec4 array
fn var_weight(t_idx: u32, slot: u32) -> f32 {
    return transforms[t_idx].var_weights[slot / 4u][slot % 4u];
}

// Apply the weighted variation blend to an affine-transformed point.
// Slot order must match scene::VARIATION_NAMES.
// The 2D-classic variations act on xy (polar angle theta measured there)
// and carry z through, while spherical/fisheye/bubble/swirl are fully 3D.
fn apply_variations(t_idx: u32, p: vec3<f32>, rng: ptr<function, vec4<u32>>) -> vec3<f32> {
    let r2 = dot(p, p);
    let r = sqrt(r2);
    let theta = atan2(p.y, p.x);

    var out = vec3<f32>(0.0);

    // 0: linear
    var w = var_weight(t_idx, 0u);
    if w != 0.0 { out += w * p; }

    // 1: sinusoidal
    w = var_weight(t_idx, 1u);
    if w != 0.0 { out += w * sin(p); }

    // 2: spherical (3D inversion)
    w = var_weight(t_idx, 2u);
    if w != 0.0 { out += w * p / max(r2, 1e-9); }

    // 3: swirl (rotate xy by r^2, z through)
    w = var_weight(t_idx, 3u);
    if w != 0.0 {
        let sr = sin(r2);
        let cr = cos(r2);
        out += w * vec3<f32>(p.x * sr - p.y * cr, p.x * cr + p.y * sr, p.z);
    }

    // 4: horseshoe
    w = var_weight(t_idx, 4u);
    if w != 0.0 {
        let inv_r = 1.0 / max(r, 1e-6);
        out += w * vec3<f32>(inv_r * (p.x - p.y) * (p.x + p.y), inv_r * 2.0 * p.x * p.y, p.z);
    }

    // 5: polar
    w = var_weight(t_idx, 5u);
    if w != 0.0 { out += w * vec3<f32>(theta / PI, r - 1.0, p.z); }

    // 6: disc
    w = var_weight(t_idx, 6u);
    if w != 0.0 {
        let f = theta / PI;
        out += w * vec3<f32>(f * sin(PI * r), f * cos(PI * r), p.z);
    }

    // 7: spiral
    w = var_weight(t_idx, 7u);
    if w != 0.0 {
        let inv_r = 1.0 / max(r, 1e-6);
        out += w * vec3<f32>(inv_r * (cos(theta) + sin(r)), inv_r * (sin(theta) - cos(r)), p.z);
    }

    // 8: hyperbolic
    w = var_weight(t_idx, 8u);
    if w != 0.0 {
        out += w * vec3<f32>(sin(theta) / max(r, 1e-6), r * cos(theta), p.z);
    }

    // 9: diamond
    w = var_weight(t_idx, 9u);
    if w != 0.0 {
        out += w * vec3<f32>(sin(theta) * cos(r), cos(theta) * sin(r), p.z);
    }

    // 10: julia (half-angle with random branch)
    w = var_weight(t_idx, 10u);
    if w != 0.0 {
        let omega = f32(xorshift128(rng) & 1u) * PI;
        let a = theta * 0.5 + omega;
        let sr = sqrt(max(r, 0.0));
        out += w * vec3<f32>(sr * cos(a), sr * sin(a), p.z);
    }

    // 11: bent
    w = var_weight(t_idx, 11u);
    if w != 0.0 {
        var b = p;
        if b.x < 0.0 { b.x *= 2.0; }
        if b.y < 0.0 { b.y *= 0.5; }
        out += w * b;
    }

    // 12: fisheye (eyefish, 3D)
    w = var_weight(t_idx, 12u);
    if w != 0.0 { out += w * (2.0 / (r + 1.0)) * p; }

    // 13: bubble (3D)
    w = var_weight(t_idx, 13u);
    if w != 0.0 { out += w * (4.0 / (r2 + 4.0)) * p; }

    // 14: cylinder
    w = var_weight(t_idx, 14u);
    if w != 0.0 { out += w * vec3<f32>(sin(p.x), p.y, p.z); }

    // 15: tangent
    w = var_weight(t_idx, 15u);
    if w != 0.0 {
        out += w * vec3<f32>(sin(p.x) / max(abs(cos(p.y)), 1e-3) * sign(cos(p.y)), tan(p.y), p.z);
    }

    // 16: absfold — KIFS kaleidoscope fold: reflect into the positive octant.
    // Pair with affine rotations (before/after via transform order) for
    // kaleidoscopic symmetry (McGraw 2015).
    w = var_weight(t_idx, 16u);
    if w != 0.0 { out += w * abs(p); }

    // 17: boxfold — Mandelbox fold: reflect components off the ±1 walls
    w = var_weight(t_idx, 17u);
    if w != 0.0 { out += w * (2.0 * clamp(p, vec3<f32>(-1.0), vec3<f32>(1.0)) - p); }

    // 18: spherefold — Mandelbox sphere fold (minR2 = 0.25, fixR2 = 1):
    // inner points blow outward, mid-range points invert, far points pass
    w = var_weight(t_idx, 18u);
    if w != 0.0 {
        var f = 1.0;
        if r2 < 0.25 { f = 4.0; } else if r2 < 1.0 { f = 1.0 / r2; }
        out += w * f * p;
    }

    // 19: bulb — power-8 mandelbulb angle map, radius-preserving: octuple
    // both spherical angles but keep r (so it never diverges/collapses)
    w = var_weight(t_idx, 19u);
    if w != 0.0 {
        let rr = max(r, 1e-9);
        let tb = 8.0 * acos(clamp(p.z / rr, -1.0, 1.0));
        let pb = 8.0 * atan2(p.y, p.x);
        out += w * r * vec3<f32>(sin(tb) * cos(pb), sin(tb) * sin(pb), cos(tb));
    }

    return out;
}

// Infinite zoom: move a chaos point onto the scale being looked at.
//
// The renormalizing map f is a contraction with fixed point p and ratio s, and
// the set we actually want to draw is the union of f^-m(S) over all integers m
// — unbounded, exactly invariant under f, self-similar at every scale (see the
// derivation in renorm.rs). Sampling it costs one round(): a chaos point at
// radius r from p belongs on the target shell R after
//
//     m = round(log(R/r) / log(1/s))
//
// applications of f^-1. So no point is ever wasted for being too deep in the
// attractor or too far out of it — each one is recycled to where the camera is.
// A random 0..levels extra contractions spread the octaves below R so the band
// is filled rather than hollow.
//
// The walker's own state is NOT renormalized: the chaos game runs on the plain
// attractor, and only what gets written to the buffer is moved.
// Which octave below the outer radius this point is dealt into. k = 0 is the
// outermost shell (radius R), k = levels-1 the innermost.
//
// Mirrored on the CPU by `Renorm::octave_offset` in renorm.rs, which is where
// this is tested; keep the two arithmetically identical.
//
// Two deals:
//
//   flat      every octave the same share (q = 1). Correct for a scene that is
//             flown, because a wrap moves the octave filling any screen region
//             along by one and equal shares are what make that invisible.
//
//   q < 1     the stills-only geometric falloff. Octave k covers s^k of the
//             frame's width and so wants fewer points for the same on-screen
//             density; `zoom_octave_q = s^octave_falloff` is that ratio, and
//             its inverse CDF is closed form. It makes every octave differ
//             from its neighbour, which a wrap turns into a density step, so
//             it is for things that never move.
//
// And nothing else — in particular nothing about where the camera is. The
// point buffer is circular and turns over at 1/800th per frame, so a deal that
// looked at the camera would mix thirteen seconds of stale camera positions
// into every frame. Fading the band's outer edge is therefore a render-time
// weight (guard_weight() in points/splat.wgsl and points/render.wgsl), where
// every point is re-weighted every frame; there used to be a taper here and it
// could not be made to work.
//
// Sum of r^i for i in 0..n, with the removable singularity at r = 1 filled in.
fn geo_sum(r: f32, n: f32) -> f32 {
    if n <= 0.0 {
        return 0.0;
    }
    if abs(r - 1.0) < 1e-4 {
        return n;
    }
    return (1.0 - pow(r, n)) / (1.0 - r);
}

// Inverse of geo_sum: the largest i with sum_{j<i} r^j <= x. Works for r above
// and below 1. Capped at ceil(n)-1, not n-1: a fractional n means a partial top
// octave, which must stay reachable and must stay a whole offset.
fn geo_pick(x: f32, r: f32, n: f32) -> f32 {
    if n <= 0.0 {
        return 0.0;
    }
    var i = x;
    if abs(r - 1.0) >= 1e-4 {
        i = log(max(1.0 - x * (1.0 - r), 1e-30)) / log(r);
    }
    return clamp(floor(i), 0.0, ceil(n) - 1.0);
}

// Share of octave k is q^k: the falloff envelope, and nothing else. q = 1 is a
// flat deal, floor(u * levels) exactly.
fn octave_offset(rng: ptr<function, vec4<u32>>) -> f32 {
    let levels = params.zoom_levels;
    if levels <= 1.0 {
        return 0.0;
    }
    let q = params.zoom_octave_q;
    return geo_pick(rand_float(rng) * geo_sum(q, levels), q, levels);
}

// Rodrigues: rotate v about a unit axis by `ang`
fn rotate_axis(v: vec3<f32>, axis: vec3<f32>, ang: f32) -> vec3<f32> {
    let c = cos(ang);
    return v * c + cross(axis, v) * sin(ang) + axis * dot(axis, v) * (1.0 - c);
}

// `f^-m` applied to an offset from the fixed point: m periods further out.
//
// A similarity has a closed-form power — s^-m with the rotation angle
// multiplied by m — so gentle maps, which need hundreds of periods to cross
// the band and are the ones that look best, cost the same as steep ones. Only
// the anisotropic fallback iterates, and there the count is capped because
// each step is a matrix multiply.
fn carry(u_in: vec3<f32>, m: f32) -> vec3<f32> {
    if params.zoom_similar != 0u {
        // Bound m so s^-m alone can't overflow before it multiplies a tiny u.
        //
        // Floored, because `m` is a whole number of periods and the clamp has
        // to leave it one. A fractional bound makes `m` fractional exactly at
        // the extremes, and `A^m` for non-integer m is branch-dependent — the
        // axis/angle pair stops being the same rotation as any of the map's
        // honest powers. Integer powers can't see the branch; fractional ones
        // are nothing but branch. Same class of bug as the zoom loop's twist.
        let k_max = floor(69.0 / params.zoom_log_scale);
        let k = clamp(m, -k_max, k_max);
        let aa = params.zoom_axis_angle;
        return pow(params.zoom_scale, -k) * rotate_axis(u_in, aa.xyz, -k * aa.w);
    }

    var u = u_in;
    let k = i32(clamp(m, -48.0, 48.0));
    for (var i = 0; i < k; i++) {
        u = params.zoom_a_inv * u;
    }
    for (var i = 0; i > k; i--) {
        u = params.zoom_a * u;
    }
    return u;
}

fn renormalize(pos: vec3<f32>, rng: ptr<function, vec4<u32>>) -> vec3<f32> {
    let u = pos - params.zoom_fixed.xyz;
    let r = length(u);
    // NaN-safe: an escaped walker falls through unchanged and is re-seeded
    if !(r > 1e-20) {
        return pos;
    }

    var m = round(log(params.zoom_fixed.w / r) / params.zoom_log_scale);
    m -= octave_offset(rng);
    return params.zoom_fixed.xyz + carry(u, m);
}

// Carry the whole buffer through `zoom_rewrap` periods, so that a camera wrap
// moves the points with it.
//
// A wrap leaves the invariant set alone and moves the camera by A⁻¹. That is
// exact for the *set* and not for a **sample** of it: the dots do not move, so
// every one of them lands on a different pixel, and the picture is redrawn
// from a completely different set of points in a single frame. Nothing enters,
// nothing leaves, no light changes — mean frame brightness does not move at
// all — but on a sparse scene, where the dots are individually resolvable, the
// whole field is resampled at once and it reads as a twitch. `tools/
// zoom_twitch.py` measures it, and measured it at exactly the cost of a fresh
// fill of the point buffer: the seam was the resample and nothing else.
//
// Carrying the buffer by the same A⁻¹ cancels it exactly, because
// `screen(A⁻¹x, A⁻¹C) == screen(x, C)`. Every point keeps its pixel.
//
// What that cannot do on its own is stay bounded — carrying camera *and*
// points is just the wrap undone, and the precision it was invented to save
// goes with it. So the deal is re-folded: the band holds `ceil(periods)`
// octaves, and a point carried out past the outermost one comes back at the
// innermost. Written as one power rather than a shift and a correction, which
// bounds `m` inside one band's worth however far the wrap jumped:
//
//     idx = round(log(R/r) / log s⁻¹)      which octave a point is in
//     m   = idx − ((idx − rewrap) mod n)   the power that lands it back
//
// So a wrap costs the octaves' assignment rotating by one, and only the points
// that fall off the outer end move at all — the outermost octave, which is
// the one the edge guard has already taken to nothing (see `guard_weight` in
// points/splat.wgsl). Everything the picture is actually made of holds still.
@compute @workgroup_size(256)
fn rewrap(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.y * params.rewrap_stride + global_id.x;
    if idx >= params.buffer_capacity || params.zoom_enabled == 0u {
        return;
    }
    let u = points[idx].position - params.zoom_fixed.xyz;
    let r = length(u);
    // Unwritten buffer sits on the fixed point; so does an escaped walker.
    if !(r > 1e-20) {
        return;
    }
    let n = max(1.0, ceil(params.zoom_levels));
    let octave = round(log(params.zoom_fixed.w / r) / params.zoom_log_scale);
    let shifted = octave - params.zoom_rewrap;
    let m = octave - (shifted - n * floor(shifted / n));
    // Position only — a carry does not touch the colour word. Measured at
    // 0.934 ms against 0.925 ms for writing the whole 16-byte point, i.e. no
    // difference: the write lands in the same cache line either way. Kept
    // because it says what the pass does, not because it is faster. The pass
    // is at the memory floor already — one streaming read and write of every
    // point, 256 MB at 8M points — so there is no version of it that is
    // meaningfully cheaper.
    points[idx].position = params.zoom_fixed.xyz + carry(u, m);
}

// One application of a transform: pre-affine, variation blend, post-affine,
// then the symmetry group element. Pulled out of `main` so that the emphasis
// probe can be pushed through the *identical* map — same transform, same
// variation branches, same group element — because it shares the walker's RNG
// state at the moment of the step and so makes every draw the same way.
//
// The draw order is exactly what it was when this was written inline, which is
// what keeps every render that existed before the emphasis unchanged — checked
// by rendering a scene from before and after and differencing: every pixel of
// every channel identical. (The PNG *files* differ; they carry a timestamp.)
struct MapResult {
    pos: vec3<f32>,
    group_pick: u32,
}

fn map_point(t_idx: u32, p: vec3<f32>, rng: ptr<function, vec4<u32>>) -> MapResult {
    // Pre-affine, then weighted variation blend, then post-affine
    let affine = (transforms[t_idx].matrix * vec4<f32>(p, 1.0)).xyz;
    let varied = apply_variations(t_idx, affine, rng);
    var out = (transforms[t_idx].post_affine * vec4<f32>(varied, 1.0)).xyz;

    // Then the symmetry group, composed on the OUTSIDE of the whole map.
    //
    // For a finite group G and maps {f}, the IFS {g∘f : g∈G} has an
    // attractor that is exactly G-symmetric, because hG = G for any h in G.
    // Drawing g uniformly here is not an approximation of that map set: it
    // *is* it. Picking f with weight w and then g uniformly samples the
    // |G|·N maps {g∘f} with weights w/|G|, so a motif keeps the share it
    // was written with and the chaos game converges exactly as before.
    //
    // The order matters and is the whole reason this sits after
    // `post_affine` rather than being folded into it: g must apply after
    // the variation blend, and a variation between two matrices cannot be
    // commuted past either.
    var pick = 0u;
    let group_count = transforms[t_idx].group_count;
    if group_count > 0u {
        // min() rather than a modulo: rand_float can return values that
        // round up to exactly 1.0 in f32, and one walker in a few billion
        // reading element `count` is a real out-of-bounds.
        pick = min(u32(rand_float(rng) * f32(group_count)), group_count - 1u);
        let g = group_elements[transforms[t_idx].group_start + pick];
        out = (g * vec4<f32>(out, 1.0)).xyz;
    }
    return MapResult(out, pick);
}

// ============================================================================
// Emphasis: how far does this point move if you nudge one transform?
// ============================================================================
//
// A point in the attractor is the image of a *word* — the sequence of maps the
// walker happened to draw to get there. Nudge one transform T and the point
// moves by the first-order sum over every occurrence of T in that word:
//
//     δ = Σ_k  J_k · E · [p_k; 1]
//
// where p_k is the point going into the k-th occurrence, E is the nudge (a
// 3×4 perturbation of T's affine part) and J_k is the Jacobian of everything
// the walker did *after* that occurrence. Two things follow, and they are the
// whole design:
//
//   * The most recent occurrence dominates, because every later map is a
//     contraction — but not overwhelmingly, and "not overwhelmingly" is
//     exactly the structure worth seeing. Material that T touched once, long
//     ago, still shifts; material it never touched does not move at all.
//   * There is no closed form for J_k with variations in the map, so it is
//     carried forward instead: one vector per walker, pushed through each
//     subsequent map by a finite difference along its own direction. That
//     costs one extra map application per iteration and is exact to first
//     order for every variation, symmetry group and post-affine in the scene.
//
// The nudge E is not something the walker can know — the user has not dragged
// anything yet — so each occurrence injects a *random* E of unit size, whose
// displacement has magnitude sqrt(|p|² + 1) (the 1 is the translation column,
// which acts whatever the point is). Averaged over occurrences this estimates
// the response to a generic nudge rather than to one particular handle, which
// is the honest thing to draw when nothing is being dragged yet: it answers
// "would this move at all", not "which way".
//
// The cross terms between occurrences drop out under that average, so what is
// drawn is E[|δ|²] over nudges — and a point dominated by one recent
// occurrence, which is most of them, has a magnitude that barely varies at
// all. The randomness shows up as direction, not as speckle.

// lowbias32 (Chris Wellons): a 32-bit integer hash good enough to draw a
// direction from, and stateless — the walker's own stream is left untouched so
// that turning the emphasis on does not change a single point of the picture.
fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return x;
}

// A direction, uniform on the sphere: z uniform in [-1, 1] and the azimuth
// uniform is the standard area-preserving parameterization.
fn hash_dir(seed: u32) -> vec3<f32> {
    let a = f32(hash_u32(seed)) / 4294967296.0 * 2.0 * PI;
    let z = f32(hash_u32(seed ^ 0x9e3779b9u)) / 4294967296.0 * 2.0 - 1.0;
    let r = sqrt(max(0.0, 1.0 - z * z));
    return vec3<f32>(r * cos(a), r * sin(a), z);
}

// Push the tangent through the map that was just applied.
//
// `rng_at_step` is the walker's RNG *before* the real step, so the probe draws
// the same variation branches and the same group element and differs from the
// real point in nothing but its position.
fn carry_tangent(
    t_idx: u32,
    p_in: vec3<f32>,
    p_out: vec3<f32>,
    d: vec3<f32>,
    rng_at_step: vec4<u32>,
) -> vec3<f32> {
    let dl = length(d);
    if !(dl > 1e-30) {
        return vec3<f32>(0.0);
    }
    // Along the tangent's own direction, so one probe suffices however
    // anisotropic the map is, at a step scaled to the point's own size —
    // f32 has about seven digits and the difference has to survive in them.
    let eps = 1e-3 * max(1.0, length(p_in));
    var rng_probe = rng_at_step;
    let probe = map_point(t_idx, p_in + d * (eps / dl), &rng_probe);
    let carried = (probe.pos - p_out) * (dl / eps);
    // A diverging variation can send the probe to infinity on its own; a
    // tangent that has gone bad would otherwise stay bad for the walker's life.
    if !(dot(carried, carried) < 1e24) {
        return vec3<f32>(0.0);
    }
    return carried;
}

// Pack a linear RGB triple into 24 bits, 8 per channel.
//
// Stored as sqrt(linear), not linear. Eight bits spread evenly over a linear
// ramp put almost all of their resolution in the highlights, and a fractal is
// mostly dark material — a straight linear quantisation bands visibly in the
// shadows, which is exactly where this renderer lives. One `sqrt` on write and
// one multiply on read buys that back. Must stay in step with `unpack_rgb` in
// render.wgsl and splat.wgsl.
fn pack_rgb(c: vec3<f32>) -> u32 {
    let q = vec3<u32>(round(sqrt(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))) * 255.0));
    return q.r | (q.g << 8u) | (q.b << 16u);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let walker_id = global_id.x;

    if walker_id >= params.num_walkers {
        return;
    }

    // This walker's layer. A handful at most, so a scan.
    var li = 0u;
    loop {
        if li + 1u >= params.num_layers || walker_id < layers[li].walker_end {
            break;
        }
        li += 1u;
    }
    let layer = layers[li];
    // A walker no visible layer claims (every layer hidden) writes nothing.
    if walker_id >= layer.walker_end || layer.t_count == 0u {
        return;
    }

    var rng = walker_states[walker_id].rng_state;
    var pos = walker_states[walker_id].current_pos;
    var color_val = walker_states[walker_id].current_color;
    var color_rgb = vec3<f32>(
        walker_states[walker_id].current_rgb_r,
        walker_states[walker_id].current_rgb_g,
        walker_states[walker_id].current_rgb_b,
    );
    var tangent = walker_states[walker_id].tangent;

    for (var i = 0u; i < params.iterations_per_walker; i++) {
        // Select random transform based on weights
        let r = rand_float(&rng);
        let transform_idx = select_transform(r, layer.t_start, layer.t_count);

        // The whole step — affine, variations, post-affine, symmetry — so
        // that the emphasis probe below can be pushed through the same one.
        let p_in = pos;
        let rng_at_step = rng;
        let step = map_point(transform_idx, pos, &rng);
        pos = step.pos;
        let group_pick = step.group_pick;
        let group_count = transforms[transform_idx].group_count;

        // Carry the tangent through the map that was just applied, and inject
        // a fresh nudge whenever the map applied *was* the one being watched.
        // See the block above `hash_u32` for what this measures.
        if params.emphasis_enabled != 0u {
            tangent = carry_tangent(transform_idx, p_in, pos, tangent, rng_at_step);
            if transform_idx == params.emphasis_target {
                // The seed mixes the walker's RNG state with the iteration, so
                // successive occurrences draw independent directions without
                // touching the stream the picture is made from.
                tangent += hash_dir(rng_at_step.x ^ (i * 0x2545f491u))
                    * sqrt(dot(p_in, p_in) + 1.0);
            }
        }

        // Nonlinear variations can diverge or produce NaN; re-seed dead walkers
        // (NaN fails all comparisons, so check for "not in bounds")
        if !(dot(pos, pos) < 1e12) {
            pos = vec3<f32>(
                rand_float(&rng) * 2.0 - 1.0,
                rand_float(&rng) * 2.0 - 1.0,
                rand_float(&rng) * 2.0 - 1.0,
            );
            // A re-seeded walker has no history, so it has no sensitivity
            // either: whatever the tangent said was about a point that is gone.
            tangent = vec3<f32>(0.0);
        }

        // Blend color toward transform's color. Both channels advance at the
        // same rate, so `color_speed` / `color_falloff` mean exactly what they
        // meant before in every mode — the accumulation stage is shared, and
        // only what it accumulates differs.
        let speed = transforms[transform_idx].color_speed;
        // Under `color = "orbit"` the color_target index is offset by which group
        // element was drawn, spreading the orbit around the colormap. Since g
        // is redrawn every iteration this tracks the walker's *most recent*
        // group element, not which copy a point ended up in — it reads as an
        // interference pattern over the whole form rather than as |G| solid
        // petals, which is why it is opt-in. See symmetry::OrbitColor.
        var color_target = transforms[transform_idx].color_value;
        if group_count > 0u {
            color_target = fract(
                color_target + transforms[transform_idx].orbit_color
                    * f32(group_pick) / f32(group_count)
            );
        }
        color_val = color_val * (1.0 - speed) + color_target * speed;
        color_rgb = mix(color_rgb, transforms[transform_idx].color_rgb, speed);

        // Calculate output index with circular wrapping
        let local_idx = walker_id * params.iterations_per_walker + i;
        let output_idx = (params.write_offset + local_idx) % params.buffer_capacity;

        // Write point to buffer, renormalized onto the visible scale band when
        // infinite zoom is on (this is the only thing zoom changes)
        var out_pos = pos;
        var out_sens = length(tangent);
        if params.zoom_enabled != 0u {
            out_pos = renormalize(pos, &rng);
            // The renormalizing map is a similarity about the fixed point, so
            // it carries a displacement by exactly the factor it carried the
            // offset by — and the emphasis has to be about where the point was
            // *drawn*, not where the chaos game happened to be running.
            let r_in = length(pos - params.zoom_fixed.xyz);
            let r_out = length(out_pos - params.zoom_fixed.xyz);
            if r_in > 1e-20 {
                out_sens *= r_out / r_in;
            }
        }

        // Both encodings ride in the one word: the colormap index in the low
        // 8 bits (every consumer already masks with 0xFF), the mixed RGB in
        // the 24 that were free. So switching colour mode is a uniform flip
        // that recolours the existing buffer — no refill, no stall.
        let color_idx = u32(clamp(color_val, 0.0, 1.0) * 255.0);
        var word = color_idx | (pack_rgb(color_rgb) << 8u);
        // A composite: carry the point into scene space by its layer's
        // placement, and resolve its colour here, through its layer's own
        // colormap and contrast — the renderers then draw packed RGB and never
        // need to know which layer a point came from. See src/subscene.rs.
        if params.composite != 0u {
            out_pos = (layer.placement * vec4<f32>(out_pos, 1.0)).xyz;
            out_sens *= layer.scale;
            if layer.mix == 0u {
                // The same cyclic contrast stretch render.wgsl applies.
                let f = (f32(color_idx) + 0.5) / 256.0;
                let stretched = fract(0.5 + (f - 0.5) * layer.contrast);
                let c = layer_colormaps[li * 256u + (u32(stretched * 256.0) & 0xFFu)].rgb;
                word = color_idx | (pack_rgb(c) << 8u);
            }
        }
        // Isolation drops a point at the write and nowhere else, so the walk —
        // and therefore every point that is kept — is exactly the full
        // picture's. Under symmetry `transform_idx` is the motif, so a motif's
        // tile holds its whole orbit.
        if params.isolate != 0u && transform_idx + 1u != params.isolate {
            out_pos = vec3<f32>(ISOLATE_DROPPED, 0.0, 0.0);
        }
        points[output_idx] = Point(out_pos, word);
        // Behind the flag because the buffer bound here is a sixteen-byte
        // stand-in when the emphasis is off — see `sensitivity` above.
        if params.emphasis_enabled != 0u {
            sensitivity[output_idx] = out_sens;
        }
    }

    // Save walker state for next frame
    walker_states[walker_id].current_pos = pos;
    walker_states[walker_id].current_color = color_val;
    walker_states[walker_id].current_rgb_r = color_rgb.r;
    walker_states[walker_id].current_rgb_g = color_rgb.g;
    walker_states[walker_id].current_rgb_b = color_rgb.b;
    walker_states[walker_id].rng_state = rng;
    walker_states[walker_id].tangent = tangent;
}

// The reference the glow normalizes against: the mean sensitivity over the
// points that belong to the current target.
//
// Sensitivity is a distance in world units, and the distances a scene is made
// of span whatever range the author chose — so there is no constant that could
// turn one into a brightness. The mean of the population itself is the only
// reference that holds across scenes, and it costs one strided pass over a
// sample of the ring: 65,536 points, one workgroup, once a frame.
//
// Smoothed, because the sample moves as the ring churns and an unsmoothed
// reference makes the whole image breathe. Seeded on the first frame that has
// anything to measure rather than eased up from zero, so the glow arrives at
// the right brightness instead of flaring.
const STATS_SAMPLES: u32 = 65536u;
var<workgroup> stat_sum: array<f32, 256>;
var<workgroup> stat_n: array<f32, 256>;

@compute @workgroup_size(256)
fn emphasis_stats(@builtin(local_invocation_id) local_id: vec3<u32>) {
    let lane = local_id.x;
    var sum = 0.0;
    var n = 0.0;

    let run = params.emphasis_run_len;
    if run > 0u {
        // Strided over the *run*, never over the whole ring: outside it the
        // values were written for a different transform.
        let stride = max(1u, run / STATS_SAMPLES);
        let taken = run / stride;
        var k = lane;
        while k < taken {
            let idx = (params.emphasis_run_start + k * stride) % params.buffer_capacity;
            let s = sensitivity[idx];
            if s > 0.0 {
                sum += s;
                n += 1.0;
            }
            k += 256u;
        }
    }

    stat_sum[lane] = sum;
    stat_n[lane] = n;
    workgroupBarrier();

    var half_width = 128u;
    while half_width > 0u {
        if lane < half_width {
            stat_sum[lane] += stat_sum[lane + half_width];
            stat_n[lane] += stat_n[lane + half_width];
        }
        workgroupBarrier();
        half_width >>= 1u;
    }

    if lane == 0u && stat_n[0] > 0.0 {
        let mean = stat_sum[0] / stat_n[0];
        if mean > 0.0 {
            if stats[1] < 0.5 {
                stats[0] = mean;
                stats[1] = 1.0;
            } else {
                stats[0] = mix(stats[0], mean, 0.1);
            }
        }
    }
}
