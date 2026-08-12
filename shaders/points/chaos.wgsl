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
}

// Must match PointComputeParams in buffers.rs (192 bytes)
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
    // Twelve scalars, so four words of padding before the vec4s. The band's
    // outer edge is faded at *render* time now (see guard_weight() in
    // points/splat.wgsl); it cannot be done here, because this buffer is
    // filled over ~800 frames and a camera-dependent deal would mix that many
    // camera positions into one image.
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
    zoom_fixed: vec4<f32>,      // xyz = fixed point, w = target radius
    zoom_axis_angle: vec4<f32>, // xyz = rotation axis of A, w = its angle
    zoom_a: mat3x3<f32>,
    zoom_a_inv: mat3x3<f32>,
}

@group(0) @binding(0) var<storage, read_write> points: array<Point>;
@group(0) @binding(1) var<storage, read> transforms: array<Transform>;
@group(0) @binding(2) var<storage, read_write> walker_states: array<WalkerState>;
@group(0) @binding(3) var<uniform> params: ComputeParams;
// Symmetry group elements: every distinct group in the scene, concatenated.
// A transform's own run is `group_start .. group_start + group_count`, and
// element 0 of any run is the identity. See src/symmetry.rs.
@group(0) @binding(4) var<storage, read> group_elements: array<mat4x4<f32>>;

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
fn select_transform(r: f32, num_transforms: u32) -> u32 {
    var lo = 0u;
    var hi = num_transforms - 1u;
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
    let idx = global_id.x;
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

    var rng = walker_states[walker_id].rng_state;
    var pos = walker_states[walker_id].current_pos;
    var color_val = walker_states[walker_id].current_color;
    var color_rgb = vec3<f32>(
        walker_states[walker_id].current_rgb_r,
        walker_states[walker_id].current_rgb_g,
        walker_states[walker_id].current_rgb_b,
    );

    for (var i = 0u; i < params.iterations_per_walker; i++) {
        // Select random transform based on weights
        let r = rand_float(&rng);
        let transform_idx = select_transform(r, params.num_transforms);

        // Pre-affine, then weighted variation blend, then post-affine
        let affine = (transforms[transform_idx].matrix * vec4<f32>(pos, 1.0)).xyz;
        let varied = apply_variations(transform_idx, affine, &rng);
        pos = (transforms[transform_idx].post_affine * vec4<f32>(varied, 1.0)).xyz;

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
        var group_pick = 0u;
        let group_count = transforms[transform_idx].group_count;
        if group_count > 0u {
            // min() rather than a modulo: rand_float can return values that
            // round up to exactly 1.0 in f32, and one walker in a few billion
            // reading element `count` is a real out-of-bounds.
            group_pick = min(u32(rand_float(&rng) * f32(group_count)), group_count - 1u);
            let g = group_elements[transforms[transform_idx].group_start + group_pick];
            pos = (g * vec4<f32>(pos, 1.0)).xyz;
        }

        // Nonlinear variations can diverge or produce NaN; re-seed dead walkers
        // (NaN fails all comparisons, so check for "not in bounds")
        if !(dot(pos, pos) < 1e12) {
            pos = vec3<f32>(
                rand_float(&rng) * 2.0 - 1.0,
                rand_float(&rng) * 2.0 - 1.0,
                rand_float(&rng) * 2.0 - 1.0,
            );
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
        if params.zoom_enabled != 0u {
            out_pos = renormalize(pos, &rng);
        }

        // Both encodings ride in the one word: the colormap index in the low
        // 8 bits (every consumer already masks with 0xFF), the mixed RGB in
        // the 24 that were free. So switching colour mode is a uniform flip
        // that recolours the existing buffer — no refill, no stall.
        let color_idx = u32(clamp(color_val, 0.0, 1.0) * 255.0);
        points[output_idx] = Point(out_pos, color_idx | (pack_rgb(color_rgb) << 8u));
    }

    // Save walker state for next frame
    walker_states[walker_id].current_pos = pos;
    walker_states[walker_id].current_color = color_val;
    walker_states[walker_id].current_rgb_r = color_rgb.r;
    walker_states[walker_id].current_rgb_g = color_rgb.g;
    walker_states[walker_id].current_rgb_b = color_rgb.b;
    walker_states[walker_id].rng_state = rng;
}
