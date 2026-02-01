// Simple chaos game compute shader
// Writes points directly to a circular buffer - no hash grid, no view-space projection

struct Point {
    position: vec3<f32>,
    color_idx: u32,
}

struct Transform {
    matrix: mat4x4<f32>,
    color_value: f32,
    weight: f32,
    cumulative_weight: f32,
    color_speed: f32,
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
        let transform = transforms[transform_idx];

        // Apply transform
        let pos4 = transform.matrix * vec4<f32>(pos, 1.0);
        pos = pos4.xyz;

        // Blend color toward transform's color
        let speed = transform.color_speed;
        color_val = color_val * (1.0 - speed) + transform.color_value * speed;

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
