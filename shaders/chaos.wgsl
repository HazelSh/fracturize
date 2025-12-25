// Chaos game compute shader
// Runs IFS iterations on the GPU using xorshift128 RNG

struct Point {
    pos: vec3<f32>,
    _pad0: f32,
    color: vec4<f32>,
}

struct Transform {
    matrix: mat4x4<f32>,     // 64 bytes
    color: vec4<f32>,        // 16 bytes
    weight: f32,             // 4 bytes
    cumulative_weight: f32,  // 4 bytes
    _pad: vec2<f32>,         // 8 bytes
}

struct IterationState {
    current_pos: vec3<f32>,
    _pad0: f32,
    current_color: vec4<f32>,
    point_write_idx: u32,
    total_iterations: u32,
    _align_pad: vec2<u32>,      // padding to align rng_state to 16 bytes
    rng_state: vec4<u32>,       // xorshift128 state
    _struct_pad: vec4<u32>,     // padding for struct alignment
}

struct ComputeParams {
    num_transforms: u32,
    max_points: u32,
    iterations_per_dispatch: u32,
    _pad: u32,
}

@group(0) @binding(0) var<storage, read_write> points: array<Point>;
@group(0) @binding(1) var<storage, read> transforms: array<Transform>;
@group(0) @binding(2) var<storage, read_write> state: IterationState;
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

// Generate random float in [0, 1)
fn rand_float(s: ptr<function, vec4<u32>>) -> f32 {
    return f32(xorshift128(s)) / 4294967296.0;
}

// Select transform based on cumulative weights
fn select_transform(r: f32, num_transforms: u32) -> u32 {
    for (var i = 0u; i < num_transforms; i++) {
        if r < transforms[i].cumulative_weight {
            return i;
        }
    }
    return num_transforms - 1u;
}

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // Load RNG state into local variable
    var rng = state.rng_state;

    // Load current position and color
    var pos = state.current_pos;
    var color = state.current_color;
    var write_idx = state.point_write_idx;
    let total_iters = state.total_iterations;

    // Check if buffer is already full (determines write strategy)
    let buffer_full = total_iters >= params.max_points;

    // Run iterations
    for (var i = 0u; i < params.iterations_per_dispatch; i++) {
        // Select transform
        let r = rand_float(&rng);
        let transform_idx = select_transform(r, params.num_transforms);
        let transform = transforms[transform_idx];

        // Apply transform
        let pos4 = transform.matrix * vec4<f32>(pos, 1.0);
        pos = pos4.xyz;

        // Blend colors (Apophysis-style: 80% old, 20% new)
        color = color * 0.8 + transform.color * 0.2;

        // Choose write index based on whether buffer is full
        var actual_write_idx: u32;
        if buffer_full {
            // Buffer full: random overwrite (like old CPU version)
            // This keeps most points stable, only updating scattered ones
            actual_write_idx = xorshift128(&rng) % params.max_points;
        } else {
            // Buffer not full: sequential fill
            actual_write_idx = write_idx;
            write_idx = (write_idx + 1u) % params.max_points;
        }

        // Store point
        points[actual_write_idx] = Point(pos, 0.0, color);
    }

    // Store state back
    state.current_pos = pos;
    state.current_color = color;
    state.point_write_idx = write_idx;
    state.total_iterations = total_iters + params.iterations_per_dispatch;
    state.rng_state = rng;
}
