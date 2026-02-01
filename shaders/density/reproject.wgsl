// Reprojection compute shader
// Reads voxels from previous frame's grid, reprojects to current view, inserts into current grid

struct HashCell {
    key: atomic<u32>,
    density: atomic<u32>,
    color_weight: atomic<u32>,
    min_depth: atomic<u32>,
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

// Empty key is 0 (cleared by GPU fill) - valid keys always have bit 0 set
const EMPTY_KEY: u32 = 0u;
const MAX_PROBES: u32 = 64u;

@group(0) @binding(0) var<storage, read_write> grid_prev: array<HashCell>;
@group(0) @binding(1) var<storage, read_write> grid_curr: array<HashCell>;
@group(0) @binding(2) var<uniform> params: HashGridParams;
@group(0) @binding(3) var<storage, read_write> depth_buffer: array<atomic<u32>>;

// Decode key to screen coordinates and depth slice
// Layout: x:11 | y:10 | depth:11 (bits 21-31, 11-20, 0-10)
fn decode_key(key: u32) -> vec3<u32> {
    let k = key - 1u;
    let depth_mask = (1u << 11u) - 1u;  // 11 bits for depth
    let y_mask = (1u << 10u) - 1u;       // 10 bits for y
    let x_mask = (1u << 11u) - 1u;       // 11 bits for x

    let depth_slice = k & depth_mask;
    let screen_y = (k >> 11u) & y_mask;
    let screen_x = (k >> 21u) & x_mask;

    return vec3<u32>(screen_x, screen_y, depth_slice);
}

// Encode screen coordinates and depth slice to key
// Add 1 to ensure the key is never 0 (which is EMPTY_KEY)
fn encode_key(screen_x: u32, screen_y: u32, depth_slice: u32) -> u32 {
    return ((screen_x << 21u) | (screen_y << 11u) | depth_slice) + 1u;
}

// Convert depth slice to linear depth (logarithmic mapping)
fn slice_to_depth(slice: u32) -> f32 {
    let t = f32(slice) / f32(params.depth_slices - 1u);
    // Logarithmic depth: more resolution near camera
    let log_near = log(params.near_plane);
    let log_far = log(params.far_plane);
    return exp(log_near + t * (log_far - log_near));
}

// Convert linear depth to depth slice
fn depth_to_slice(depth: f32) -> u32 {
    let log_near = log(params.near_plane);
    let log_far = log(params.far_plane);
    let t = (log(depth) - log_near) / (log_far - log_near);
    let slice = u32(round(clamp(t, 0.0, 1.0) * f32(params.depth_slices - 1u)));
    return slice;
}

// Simple noise function for dithering (using cell index + frame as seed)
fn dither_noise(seed: u32) -> f32 {
    var h = seed;
    h = h ^ (h >> 17u);
    h = h * 0xed5ad4bbu;
    h = h ^ (h >> 11u);
    h = h * 0xac4c1b51u;
    h = h ^ (h >> 15u);
    return f32(h) / 4294967296.0;  // [0, 1)
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
fn hash_insert(key: u32, density: u32, color_weight: u32, min_depth: u32) -> bool {
    let start_idx = hash_key(key) % params.hash_size;

    for (var probe = 0u; probe < MAX_PROBES; probe++) {
        let idx = (start_idx + probe) % params.hash_size;

        // Try to claim this slot
        let old_key = atomicCompareExchangeWeak(&grid_curr[idx].key, EMPTY_KEY, key);

        if old_key.exchanged {
            // We claimed an empty slot - initialize it
            atomicStore(&grid_curr[idx].density, density);
            atomicStore(&grid_curr[idx].color_weight, color_weight);
            atomicMin(&grid_curr[idx].min_depth, min_depth);
            return true;
        } else if old_key.old_value == key {
            // Same key exists - accumulate
            atomicAdd(&grid_curr[idx].density, density);
            // For color: we keep the existing color (first writer wins for simplicity)
            atomicMin(&grid_curr[idx].min_depth, min_depth);
            return true;
        }
        // Different key - continue probing
    }
    // Dropped sample (hash table too full in this region)
    return false;
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if idx >= params.hash_size {
        return;
    }

    // Read from previous frame's grid using atomicLoad
    let cell_key = atomicLoad(&grid_prev[idx].key);
    if cell_key == EMPTY_KEY {
        return;
    }
    let cell_density = atomicLoad(&grid_prev[idx].density);
    let cell_color_weight = atomicLoad(&grid_prev[idx].color_weight);

    // Decode screen coordinates from key
    let coords = decode_key(cell_key);
    let screen_x = coords.x;
    let screen_y = coords.y;
    let depth_slice = coords.z;

    // Convert to NDC
    let res_x = f32(params.res_x);
    let res_y = f32(params.res_y);
    let ndc_x = (f32(screen_x) + 0.5) / res_x * 2.0 - 1.0;
    let ndc_y = (f32(screen_y) + 0.5) / res_y * 2.0 - 1.0;

    // Get linear depth (view-space Z, i.e., clip.w) from slice
    let w = slice_to_depth(depth_slice);

    // Unproject to world space
    // Clip space: (ndc_x * w, ndc_y * w, ndc_z * w, w)
    // For wgpu perspective_rh with [0,1] depth range:
    // ndc_z = far * (w - near) / (w * (far - near))
    let near = params.near_plane;
    let far = params.far_plane;
    let ndc_z = far * (w - near) / (w * (far - near));
    let clip_pos = vec4<f32>(ndc_x * w, ndc_y * w, ndc_z * w, w);

    // Unproject to world space and normalize homogeneous coordinate
    let world_h = params.prev_inv_view_proj * clip_pos;
    let world_pos = world_h.xyz / world_h.w;

    // Reproject to current view
    let new_clip = params.curr_view_proj * vec4<f32>(world_pos, 1.0);

    // Frustum culling
    if new_clip.w <= params.near_plane * 0.5 {
        return;
    }

    let new_ndc = new_clip.xyz / new_clip.w;

    // Check if still on screen
    if any(abs(new_ndc.xy) > vec2<f32>(1.0, 1.0)) {
        return;
    }

    // Convert NDC to screen coords (no dithering for temporal stability)
    let new_screen_x = u32(clamp(round((new_ndc.x * 0.5 + 0.5) * res_x - 0.5), 0.0, res_x - 1.0));
    let new_screen_y = u32(clamp(round((new_ndc.y * 0.5 + 0.5) * res_y - 0.5), 0.0, res_y - 1.0));
    let new_depth_slice = depth_to_slice(new_clip.w);

    // Encode new key
    let new_key = encode_key(new_screen_x, new_screen_y, new_depth_slice);

    // Apply decay to density
    let decayed_density = u32(f32(cell_density) * params.decay);
    if decayed_density == 0u {
        return;  // Faded out completely
    }

    // Convert depth to u32 for atomicMin (smaller = closer)
    // Use new_clip.w for the reprojected position
    let depth_fixed = u32(clamp(new_clip.w / params.far_plane, 0.0, 1.0) * f32(0xFFFFFFFFu));

    // Insert into current grid - only update depth buffer if successful
    if hash_insert(new_key, decayed_density, cell_color_weight, depth_fixed) {
        // Update screen-space depth buffer using depth_slice directly (not clip.w)
        // This ensures exact match with compact.wgsl's reconstruction
        let pixel_idx = new_screen_y * params.res_x + new_screen_x;
        let inverted_slice = params.depth_slices - 1u - new_depth_slice;
        atomicMax(&depth_buffer[pixel_idx], inverted_slice);
    }
}
