// Compaction compute shader
// Scans hash grid, extracts non-empty cells to dense render buffer
enable f16;

struct HashCell {
    key: atomic<u32>,
    density: atomic<u32>,
    color_weight: atomic<u32>,
    min_depth: atomic<u32>,
}

struct RenderVoxel {
    ndc_xyzw: vec4<f16>,
    color_rgb: vec3<f16>,
    density: f16,
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

struct VoxelCounter {
    count: atomic<u32>,
}

// Empty key is 0 (cleared by GPU fill) - valid keys always have +1 offset
const EMPTY_KEY: u32 = 0u;
const MAX_RENDER_VOXELS: u32 = 2u * 1024u * 1024u;

// Tolerance for depth comparison - now using slice indices directly
// Larger tolerance needed because multiple depth slices may legitimately
// contribute to the same screen pixel (fractal surfaces have depth variation)
const DEPTH_SLICE_EPSILON: u32 = 2u;

@group(0) @binding(0) var<storage, read_write> grid: array<HashCell>;
@group(0) @binding(1) var<storage, read_write> render_voxels: array<RenderVoxel>;
@group(0) @binding(2) var<storage, read_write> counter: VoxelCounter;
@group(0) @binding(3) var<uniform> params: HashGridParams;
@group(0) @binding(4) var<storage, read> colormap: array<vec4<f32>, 256>;
@group(0) @binding(5) var<storage, read> depth_buffer: array<u32>;

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

// Simple noise function for dithering
fn dither_noise(seed: u32) -> f32 {
    var h = seed;
    h = h ^ (h >> 17u);
    h = h * 0xed5ad4bbu;
    h = h ^ (h >> 11u);
    h = h * 0xac4c1b51u;
    h = h ^ (h >> 15u);
    return f32(h) / 4294967296.0;  // [0, 1)
}

// Convert depth slice to linear depth
fn slice_to_depth(slice: u32) -> f32 {
    let t = f32(slice) / f32(params.depth_slices - 1u);
    let log_near = log(params.near_plane);
    let log_far = log(params.far_plane);
    return exp(log_near + t * (log_far - log_near));
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if idx >= params.hash_size {
        return;
    }

    // Read atomically from the grid
    let cell_key = atomicLoad(&grid[idx].key);
    if cell_key == EMPTY_KEY {
        return;
    }

    let cell_density = atomicLoad(&grid[idx].density);
    let cell_color_weight = atomicLoad(&grid[idx].color_weight);

    // Skip cells with very low density (noise)
    if cell_density < 256u {  // Less than 1.0 in u16.8 fixed point
        return;
    }

    // Decode screen coordinates
    let coords = decode_key(cell_key);
    let screen_x = coords.x;
    let screen_y = coords.y;
    let depth_slice = coords.z;

    // Get linear depth
    let depth = slice_to_depth(depth_slice);

    // Depth culling (optional): check if this cell is at the closest depth for this pixel
    if params.depth_cull_enabled != 0u {
        // We use inverted slice index (larger = closer), matching chaos.wgsl and reproject.wgsl
        let pixel_idx = screen_y * params.res_x + screen_x;
        let max_inverted_slice = depth_buffer[pixel_idx];

        // Calculate this cell's inverted slice (same formula as chaos/reproject)
        let cell_inverted_slice = params.depth_slices - 1u - depth_slice;

        // Skip if this cell is behind the closest surface (with small tolerance)
        if cell_inverted_slice + DEPTH_SLICE_EPSILON < max_inverted_slice {
            return;  // Occluded by closer voxel
        }
    }

    // Convert to NDC (pixel centers, no dithering)
    let res_x = f32(params.res_x);
    let res_y = f32(params.res_y);
    let ndc_x = (f32(screen_x) + 0.5) / res_x * 2.0 - 1.0;
    let ndc_y = (f32(screen_y) + 0.5) / res_y * 2.0 - 1.0;

    // NDC z for wgpu perspective_rh with [0,1] depth range:
    // ndc_z = far * (depth - near) / (depth * (far - near))
    let near = params.near_plane;
    let far = params.far_plane;
    let ndc_z = far * (depth - near) / (depth * (far - near));

    // Decode color from color_weight
    // Format: colormap_idx:8 | reserved:8 | weight:16
    let color_idx = (cell_color_weight >> 24u) & 0xFFu;
    let color = colormap[color_idx];

    // Normalize density for rendering (convert from fixed point)
    // u16.8 format means divide by 256 to get the count, then scale for visualization
    let density_f = f32(cell_density) / 256.0;
    // Log scale for better visual range, clamped to reasonable values
    let density_normalized = clamp(log(density_f + 1.0) / 10.0, 0.0, 1.0);

    // Atomically claim a slot in the output buffer
    let output_idx = atomicAdd(&counter.count, 1u);
    if output_idx >= MAX_RENDER_VOXELS {
        return;  // Buffer full
    }

    // Write render voxel
    render_voxels[output_idx] = RenderVoxel(
        vec4<f16>(f16(ndc_x), f16(ndc_y), f16(ndc_z), f16(depth)),
        vec3<f16>(f16(color.r), f16(color.g), f16(color.b)),
        f16(density_normalized)
    );
}
