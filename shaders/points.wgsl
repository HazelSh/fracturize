// Point rendering shader with vertex pulling
// Reads point data from storage buffer, renders as circular sprites

struct Point {
    pos: vec3<f32>,
    _pad0: f32,
    color: vec4<f32>,
}

struct CameraUniforms {
    mvp: mat4x4<f32>,
    screen_height: f32,
    point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
}

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) point_coord: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // Each point generates 6 vertices (2 triangles for a quad)
    let point_index = vertex_index / 6u;
    let corner_index = vertex_index % 6u;

    let point = points[point_index];

    // Transform point to clip space
    let clip_pos = camera.mvp * vec4<f32>(point.pos, 1.0);

    // Calculate point size in pixels based on world-space size and depth
    // FOV = 45 degrees, tan(45/2) = 0.41421356
    // scale_factor = 1.0 / (2.0 * 0.41421) = 1.2071
    let scale_factor = 1.2071;
    let base_size_pixels = (camera.point_size * camera.screen_height * scale_factor) / clip_pos.w;

    // Clamp to minimum size - this prevents distant points from disappearing
    // Gives the feathery/sandy Apophysis aesthetic
    let size_pixels = max(base_size_pixels, camera.min_point_pixels);

    // Convert pixel size to NDC (normalized device coordinates)
    // NDC goes from -1 to 1, so 2 units = screen_height pixels in Y
    let ndc_size_y = size_pixels / camera.screen_height * 2.0;
    // Adjust X for aspect ratio to keep circles circular
    let ndc_size_x = ndc_size_y / camera.aspect_ratio;

    // Corner offsets for quad (2 triangles: 0,1,2 and 2,3,0)
    var corner_offsets: array<vec2<f32>, 6>;
    corner_offsets[0] = vec2<f32>(-1.0, -1.0);
    corner_offsets[1] = vec2<f32>( 1.0, -1.0);
    corner_offsets[2] = vec2<f32>( 1.0,  1.0);
    corner_offsets[3] = vec2<f32>( 1.0,  1.0);
    corner_offsets[4] = vec2<f32>(-1.0,  1.0);
    corner_offsets[5] = vec2<f32>(-1.0, -1.0);

    let offset = corner_offsets[corner_index];

    // Calculate clip-space offset
    // To get NDC offset after perspective divide, multiply by w
    let clip_offset = vec2<f32>(
        offset.x * ndc_size_x * 0.5 * clip_pos.w,
        offset.y * ndc_size_y * 0.5 * clip_pos.w
    );

    var out: VertexOutput;
    out.clip_position = clip_pos + vec4<f32>(clip_offset, 0.0, 0.0);
    out.color = point.color;
    out.point_coord = offset; // -1 to 1 across the quad

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Circular mask - sharp hard edge
    let dist = length(in.point_coord);

    // Hard circular cutoff
    if dist > 1.0 {
        discard;
    }

    // Full color, no transparency, no dimming
    return vec4<f32>(in.color.rgb, 1.0);
}
