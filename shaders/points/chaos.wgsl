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

// Must match GpuTransform in buffers.rs (144 bytes)
struct Transform {
    matrix: mat4x4<f32>,
    color_value: f32,
    weight: f32,
    cumulative_weight: f32,
    color_speed: f32,
    // Variation weights; slot order matches scene::VARIATION_NAMES
    var_weights: array<vec4<f32>, 4>,
}

struct WalkerState {
    current_pos: vec3<f32>,
    _pad0: f32,
    current_color: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
    rng_state: vec4<u32>,
}

struct ComputeParams {
    num_transforms: u32,
    num_walkers: u32,
    iterations_per_walker: u32,
    write_offset: u32,
    buffer_capacity: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
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

fn select_transform(r: f32, num_transforms: u32) -> u32 {
    for (var i = 0u; i < num_transforms; i++) {
        if r < transforms[i].cumulative_weight {
            return i;
        }
    }
    return num_transforms - 1u;
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

    return out;
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

        // Blend color toward transform's color
        let speed = transforms[transform_idx].color_speed;
        color_val = color_val * (1.0 - speed) + transforms[transform_idx].color_value * speed;

        // Calculate output index with circular wrapping
        let local_idx = walker_id * params.iterations_per_walker + i;
        let output_idx = (params.write_offset + local_idx) % params.buffer_capacity;

        // Write point to buffer
        let color_idx = u32(clamp(color_val, 0.0, 1.0) * 255.0);
        points[output_idx] = Point(pos, color_idx);
    }

    // Save walker state for next frame
    walker_states[walker_id].current_pos = pos;
    walker_states[walker_id].current_color = color_val;
    walker_states[walker_id].rng_state = rng;
}
