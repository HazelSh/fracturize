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

// Must match GpuTransform in buffers.rs (176 bytes)
struct Transform {
    matrix: mat4x4<f32>,
    color_value: f32,
    weight: f32,
    cumulative_weight: f32,
    color_speed: f32,
    // Variation weights; slot order matches scene::VARIATION_NAMES
    var_weights: array<vec4<f32>, 5>,
    // This transform's own colour, linear RGB (ColorMode::Mix only). vec3 is
    // 16-aligned, so it lands at offset 160 and the struct is 176.
    color_rgb: vec3<f32>,
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
    _pad0: f32,
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
    zoom_octave_fade: f32,      // periods of tapered outer edge; 0 = hard cut
    zoom_fade_g: f32,           // per-period attenuation across that taper
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    zoom_fixed: vec4<f32>,      // xyz = fixed point, w = target radius
    zoom_axis_angle: vec4<f32>, // xyz = rotation axis of A, w = its angle
    zoom_a: mat3x3<f32>,
    zoom_a_inv: mat3x3<f32>,
}

@group(0) @binding(0) var<storage, read_write> points: array<Point>;
@group(0) @binding(1) var<storage, read> transforms: array<Transform>;
@group(0) @binding(2) var<storage, read_write> walker_states: array<WalkerState>;
@group(0) @binding(3) var<uniform> params: ComputeParams;

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
// Three deals, in the order branched on:
//
//   q < 1     the stills-only geometric falloff. Octave k covers s^k of the
//             frame's width and so wants fewer points for the same on-screen
//             density; `zoom_octave_q = s^octave_falloff` is that ratio, and
//             its inverse CDF is closed form. It makes every octave differ
//             from its neighbour, which a wrap turns into a density step, so
//             it is for things that never move. The taper does not compose
//             with it and is switched off on the CPU when it is in use.
//
//   flat      every octave the same share. Correct for a scene that is flown,
//             because a wrap moves the octave filling any screen region along
//             by one and equal shares are what make that invisible.
//
//   tapered   flat, except that the outermost `zoom_octave_fade` periods ramp
//             down to a sixteenth of a share at the edge. Without it the band
//             simply stops, and a wrap walks the picture off that cliff: the
//             outermost octave is replaced by one that isn't there, and a
//             whole slab of structure goes between two frames. Fading trades
//             that for a factor-of-g density step per wrap in the faded
//             region only — which is a real cost, but g is nearer 1 than
//             infinity is, and by the time material falls off the end there
//             is a sixteenth of it left.
//
// The share of octave k is g^(F-k) for k < F and 1 for k >= F. Both pieces are
// geometric, so this stays one sample per point with no rejection — the
// property that makes renormalization cost nothing in the first place.
fn octave_offset(rng: ptr<function, vec4<u32>>) -> f32 {
    let levels = params.zoom_levels;
    if levels <= 1.0 {
        return 0.0;
    }
    let u = rand_float(rng);
    let q = params.zoom_octave_q;
    if q < 0.9999 {
        // Inverse CDF of a geometric distribution truncated to 0..levels-1
        let tail = pow(q, levels);
        return min(floor(log(1.0 - u * (1.0 - tail)) / log(q)), levels - 1.0);
    }

    let f = params.zoom_octave_fade;
    let g = params.zoom_fade_g;
    if f < 1.0 || g > 0.9999 {
        return floor(u * levels);
    }

    let tail = pow(g, f);              // the depth of the fade, ~1/16
    let m1 = g * (1.0 - tail) / (1.0 - g);  // mass of the ramp: shares g^1..g^F
    let m2 = levels - f;                    // mass of the flat remainder
    let x = u * (m1 + m2);
    if x < m1 {
        // Geometric in j, counting inward from the outermost faded shell:
        // reindexing a rising ramp from its far end makes it a falling one, so
        // this is the same inversion as the falloff branch above. j is capped
        // because a u at the very top of the piece sends the log to exactly F.
        let v = x / m1;
        let j = min(floor(log(1.0 - v * (1.0 - tail)) / log(g)), f - 1.0);
        return max(f - 1.0 - j, 0.0);
    }
    // Flat remainder. x < m1 + m2 = levels, so this floors no higher than the
    // flat deal above does — partial top octave included.
    return floor(f + (x - m1));
}

// Rodrigues: rotate v about a unit axis by `ang`
fn rotate_axis(v: vec3<f32>, axis: vec3<f32>, ang: f32) -> vec3<f32> {
    let c = cos(ang);
    return v * c + cross(axis, v) * sin(ang) + axis * dot(axis, v) * (1.0 - c);
}

fn renormalize(pos: vec3<f32>, rng: ptr<function, vec4<u32>>) -> vec3<f32> {
    var u = pos - params.zoom_fixed.xyz;
    let r = length(u);
    // NaN-safe: an escaped walker falls through unchanged and is re-seeded
    if !(r > 1e-20) {
        return pos;
    }

    var m = round(log(params.zoom_fixed.w / r) / params.zoom_log_scale);
    m -= octave_offset(rng);
    // A similarity has a closed-form power — s^-k with the rotation angle
    // multiplied by k — so gentle maps, which need hundreds of periods to
    // cross the band and are the ones that look best, cost the same as steep
    // ones. Only the anisotropic fallback iterates, and there the count is
    // capped because each step is a matrix multiply.
    if params.zoom_similar != 0u {
        // Bound k so s^-k alone can't overflow before it multiplies a tiny u.
        //
        // Floored, because `m` is a whole number of periods and the clamp has
        // to leave it one. A fractional bound makes `k` fractional exactly at
        // the extremes, and `A^k` for non-integer k is branch-dependent — the
        // axis/angle pair stops being the same rotation as any of the map's
        // honest powers. Integer powers can't see the branch; fractional ones
        // are nothing but branch. Same class of bug as the zoom loop's twist.
        let k_max = floor(69.0 / params.zoom_log_scale);
        let k = clamp(m, -k_max, k_max);
        let aa = params.zoom_axis_angle;
        return params.zoom_fixed.xyz
            + pow(params.zoom_scale, -k) * rotate_axis(u, aa.xyz, -k * aa.w);
    }

    let k = i32(clamp(m, -48.0, 48.0));
    for (var i = 0; i < k; i++) {
        u = params.zoom_a_inv * u;
    }
    for (var i = 0; i > k; i--) {
        u = params.zoom_a * u;
    }
    return params.zoom_fixed.xyz + u;
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

        // Affine part, then weighted variation blend
        let affine = (transforms[transform_idx].matrix * vec4<f32>(pos, 1.0)).xyz;
        pos = apply_variations(transform_idx, affine, &rng);

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
        color_val = color_val * (1.0 - speed) + transforms[transform_idx].color_value * speed;
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
