// Chaos game compute shader - Hash grid version
// Projects points to view space and inserts into hash grid for accumulation
enable f16;

struct HashCell {
    key: atomic<u32>,
    density: atomic<u32>,
    color_weight: atomic<u32>,
    min_depth: atomic<u32>,
}

// Must match GpuTransform in buffers.rs (240 bytes). This pass only reads
// `matrix` and the colour scalars, but the *layout* still has to be declared
// in full: the buffer is a flat array, so a short struct here silently makes
// `transforms[1]` onward read from the middle of their predecessor.
// Must match GpuTransform in buffers.rs (256 bytes). This renderer is the
// inactive experimental one and does NOT apply symmetry groups — but it reads
// the same `GpuTransform` buffer, so the tail fields have to be declared or
// every transform after the first would be read from the wrong offset.
struct Transform {
    matrix: mat4x4<f32>,
    post_affine: mat4x4<f32>,
    color_value: f32,
    weight: f32,
    cumulative_weight: f32,
    color_speed: f32,
    var_weights: array<vec4<f32>, 5>,
    color_rgb: vec3<f32>,
    group_start: u32,
    group_count: u32,
    orbit_color: f32,
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
    _pad: u32,
}

struct HashGridParams {
    prev_inv_view_proj: mat4x4<f32>,
    curr_view_proj: mat4x4<f32>,
    res_x: u32,
    res_y: u32,
    depth_slices: u32,
    decay: f32,
    hash_size: u32,
    near_plane: f32,
    far_plane: f32,
    frame: u32,
    depth_cull_enabled: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

// Empty key is 0 (cleared by GPU fill) - valid keys always have +1 offset
const EMPTY_KEY: u32 = 0u;
const MAX_PROBES: u32 = 64u;
const DENSITY_UNIT: u32 = 256u;  // 1.0 in u16.8 fixed point

@group(0) @binding(0) var<storage, read_write> grid: array<HashCell>;
@group(0) @binding(1) var<storage, read> transforms: array<Transform>;
@group(0) @binding(2) var<storage, read_write> walker_states: array<WalkerState>;
@group(0) @binding(3) var<uniform> compute_params: ComputeParams;
@group(0) @binding(4) var<uniform> grid_params: HashGridParams;
@group(0) @binding(5) var<storage, read_write> depth_buffer: array<atomic<u32>>;

// Encode screen coordinates and depth slice to key
// Layout: x:11 | y:10 | depth:11 (bits 21-31, 11-20, 0-10)
fn encode_key(screen_x: u32, screen_y: u32, depth_slice: u32) -> u32 {
    return ((screen_x << 21u) | (screen_y << 11u) | depth_slice) + 1u;
}

// Convert linear depth to depth slice (logarithmic mapping)
fn depth_to_slice(depth: f32) -> u32 {
    let log_near = log(grid_params.near_plane);
    let log_far = log(grid_params.far_plane);
    let t = (log(max(depth, grid_params.near_plane)) - log_near) / (log_far - log_near);
    return u32(round(clamp(t, 0.0, 1.0) * f32(grid_params.depth_slices - 1u)));
}

// Hash function for linear probing
fn hash_key(key: u32) -> u32 {
    var h = key;
    h = h ^ (h >> 16u);
    h = h * 0x85ebca6bu;
    h = h ^ (h >> 13u);
    h = h * 0xc2b2ae35u;
    h = h ^ (h >> 16u);
    return h;
}

// Insert into hash table with linear probing
// Returns true if insert succeeded, false if dropped
fn hash_insert(key: u32, color_idx: u32, depth_fixed: u32) -> bool {
    let start_idx = hash_key(key) % grid_params.hash_size;

    for (var probe = 0u; probe < MAX_PROBES; probe++) {
        let idx = (start_idx + probe) % grid_params.hash_size;

        // Try to claim this slot
        let old_key = atomicCompareExchangeWeak(&grid[idx].key, EMPTY_KEY, key);

        if old_key.exchanged {
            // We claimed an empty slot - initialize it
            atomicStore(&grid[idx].density, DENSITY_UNIT);
            // Store color index in upper 8 bits, weight in lower 16 bits
            atomicStore(&grid[idx].color_weight, (color_idx << 24u) | 1u);
            atomicStore(&grid[idx].min_depth, depth_fixed);
            return true;
        } else if old_key.old_value == key {
            // Same key exists - accumulate density
            atomicAdd(&grid[idx].density, DENSITY_UNIT);
            // Increment weight for color averaging (lower 16 bits)
            atomicAdd(&grid[idx].color_weight, 1u);
            atomicMin(&grid[idx].min_depth, depth_fixed);
            return true;
        }
        // Different key - continue probing
    }
    // Dropped sample (hash table too full in this region)
    return false;
}

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

    if walker_id >= compute_params.num_walkers {
        return;
    }

    var rng = walker_states[walker_id].rng_state;
    var pos = walker_states[walker_id].current_pos;
    var color_val = walker_states[walker_id].current_color;

    let res_x = f32(grid_params.res_x);
    let res_y = f32(grid_params.res_y);

    for (var i = 0u; i < compute_params.iterations_per_walker; i++) {
        let r = rand_float(&rng);
        let transform_idx = select_transform(r, compute_params.num_transforms);
        let transform = transforms[transform_idx];

        // NOTE: this walk is affine-only — it does not apply the variation
        // blend, so it already disagrees with points/chaos.wgsl for any scene
        // that uses one. Pre-existing; this module is not currently wired into
        // the app (nothing outside `gpu::density` refers to it). The
        // post-affine is applied so the two walks at least agree on the maps
        // they can both express.
        let pos4 = transform.matrix * vec4<f32>(pos, 1.0);
        pos = (transform.post_affine * vec4<f32>(pos4.xyz, 1.0)).xyz;

        let speed = transform.color_speed;
        color_val = color_val * (1.0 - speed) + transform.color_value * speed;

        // Project to clip space
        let clip = grid_params.curr_view_proj * vec4<f32>(pos, 1.0);

        // Skip points behind camera
        if clip.w <= grid_params.near_plane * 0.5 {
            continue;
        }

        // Perspective divide to NDC
        let ndc = clip.xyz / clip.w;

        // Skip points outside frustum
        if any(abs(ndc.xy) > vec2<f32>(1.0, 1.0)) {
            continue;
        }

        // Convert to screen coordinates (no dithering for temporal stability)
        // The -0.5 converts from edge-to-edge NDC mapping to pixel-center coordinates
        let screen_x = u32(clamp(round((ndc.x * 0.5 + 0.5) * res_x - 0.5), 0.0, res_x - 1.0));
        let screen_y = u32(clamp(round((ndc.y * 0.5 + 0.5) * res_y - 0.5), 0.0, res_y - 1.0));

        // Get depth slice
        let depth_slice = depth_to_slice(clip.w);

        // Encode key
        let key = encode_key(screen_x, screen_y, depth_slice);

        // Quantize color to 8-bit index (0-255)
        let color_idx = u32(clamp(color_val, 0.0, 1.0) * 255.0);

        // Convert depth to fixed-point for atomicMin (smaller = closer)
        let depth_fixed = u32(clamp(clip.w / grid_params.far_plane, 0.0, 1.0) * f32(0xFFFFFFFFu));

        // Insert into hash grid - only update depth buffer if successful
        if hash_insert(key, color_idx, depth_fixed) {
            // Update screen-space depth buffer using depth_slice directly (not clip.w)
            // This ensures exact match with compact.wgsl's reconstruction
            // Inverted: larger depth_slice = farther, so invert for larger = closer
            let pixel_idx = screen_y * grid_params.res_x + screen_x;
            let inverted_slice = grid_params.depth_slices - 1u - depth_slice;
            atomicMax(&depth_buffer[pixel_idx], inverted_slice);
        }
    }

    walker_states[walker_id].current_pos = pos;
    walker_states[walker_id].current_color = color_val;
    walker_states[walker_id].rng_state = rng;
}
