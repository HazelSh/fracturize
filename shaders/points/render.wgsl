// Simple point billboard renderer
// Renders points as axis-aligned quads with depth-based sizing

struct Point {
    position: vec3<f32>,
    color_idx: u32,
}

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    screen_height: f32,
    point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
    fog_near: f32,
    fog_far: f32,
    fog_brightness: f32,
    fog_saturation: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) depth: f32,
}

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;
@group(0) @binding(2) var<storage, read> colormap: array<vec4<f32>, 256>;

@vertex
fn vs_main(
    @builtin(vertex_index) corner_index: u32,
    @builtin(instance_index) point_index: u32,
) -> VertexOutput {
    let point = points[point_index];

    // Transform to clip space
    let clip = camera.view_proj * vec4<f32>(point.position, 1.0);
    let depth = clip.w;

    // Handle points behind camera - move them off screen
    if depth <= 0.0 {
        var out: VertexOutput;
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        out.color = vec3<f32>(0.0);
        out.depth = 0.0;
        return out;
    }

    let ndc = clip.xyz / clip.w;

    // Point size decreases with depth (perspective)
    let base_size = camera.point_size * camera.screen_height / depth;
    let size_pixels = max(base_size, camera.min_point_pixels);

    // Convert to NDC size
    let ndc_size_y = size_pixels / camera.screen_height * 2.0;
    let ndc_size_x = ndc_size_y / camera.aspect_ratio;

    // Billboard quad corner from vertex index (4-vertex triangle strip):
    // 0=(-1,-1) 1=(1,-1) 2=(-1,1) 3=(1,1)
    let offset = vec2<f32>(
        f32(corner_index & 1u) * 2.0 - 1.0,
        f32(corner_index >> 1u) * 2.0 - 1.0,
    );

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        ndc.x + offset.x * ndc_size_x * 0.5,
        ndc.y + offset.y * ndc_size_y * 0.5,
        ndc.z,
        1.0
    );

    // Look up color from colormap
    let color_idx = point.color_idx & 0xFFu;
    out.color = colormap[color_idx].rgb;
    out.depth = depth;

    return out;
}

// Fast path: native 1px point primitives, used when the projected point size
// is subpixel anyway (~3x fewer vertices than the billboard quad path)
@vertex
fn vs_point(@builtin(vertex_index) point_index: u32) -> VertexOutput {
    let point = points[point_index];

    let clip = camera.view_proj * vec4<f32>(point.position, 1.0);
    let depth = clip.w;

    var out: VertexOutput;
    if depth <= 0.0 {
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        out.color = vec3<f32>(0.0);
        out.depth = 0.0;
        return out;
    }

    out.clip_position = vec4<f32>(clip.xyz / clip.w, 1.0);
    out.color = colormap[point.color_idx & 0xFFu].rgb;
    out.depth = depth;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Apply fog based on depth
    let fog_range = camera.fog_far - camera.fog_near;
    let fog_factor = clamp((in.depth - camera.fog_near) / fog_range, 0.0, 1.0);

    // Desaturate with distance
    let lum = dot(in.color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let gray = vec3<f32>(lum);

    let sat_factor = mix(1.0, camera.fog_saturation, fog_factor);
    let desaturated = mix(gray, in.color, sat_factor);

    // Reduce brightness with distance
    let bright_factor = mix(1.0, camera.fog_brightness, fog_factor);
    let final_color = desaturated * bright_factor;

    return vec4<f32>(final_color, 1.0);
}
