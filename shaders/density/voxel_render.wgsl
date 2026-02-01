// Voxel rendering shader
// Renders billboard quads from compacted voxels
enable f16;

struct RenderVoxel {
    ndc_xyzw: vec4<f16>,  // x, y, z in NDC, w = linear depth
    color_rgb: vec3<f16>,
    density: f16,
}

struct CameraUniforms {
    mvp: mat4x4<f32>,  // Not used directly, but kept for compatibility
    screen_height: f32,
    point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
    fog_near: f32,
    fog_far: f32,
    fog_brightness: f32,
    fog_saturation: f32,
}

@group(0) @binding(0) var<storage, read> voxels: array<RenderVoxel>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) point_coord: vec2<f32>,
    @location(2) view_depth: f32,
    @location(3) density: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // 6 vertices per voxel (2 triangles for quad)
    let voxel_index = vertex_index / 6u;
    let corner_index = vertex_index % 6u;

    let voxel = voxels[voxel_index];

    // Extract position in NDC
    let ndc_x = f32(voxel.ndc_xyzw.x);
    let ndc_y = f32(voxel.ndc_xyzw.y);
    let ndc_z = f32(voxel.ndc_xyzw.z);
    let depth = f32(voxel.ndc_xyzw.w);

    // Extract color
    let color = vec4<f32>(
        f32(voxel.color_rgb.x),
        f32(voxel.color_rgb.y),
        f32(voxel.color_rgb.z),
        1.0
    );

    let density = f32(voxel.density);

    // Calculate point size in NDC
    // Scale with depth like the original renderer
    let scale_factor = 1.2071;
    let base_size = (camera.point_size * camera.screen_height * scale_factor) / depth;
    // Modulate size by density - higher density = slightly larger points
    let density_scale = 1.0 + density * 0.5;
    let size_pixels = max(base_size * density_scale, camera.min_point_pixels);

    // Convert to NDC offset
    let ndc_size_y = size_pixels / camera.screen_height * 2.0;
    let ndc_size_x = ndc_size_y / camera.aspect_ratio;

    // Corner offsets for quad (2 triangles)
    var corner_offsets: array<vec2<f32>, 6>;
    corner_offsets[0] = vec2<f32>(-1.0, -1.0);
    corner_offsets[1] = vec2<f32>( 1.0, -1.0);
    corner_offsets[2] = vec2<f32>( 1.0,  1.0);
    corner_offsets[3] = vec2<f32>( 1.0,  1.0);
    corner_offsets[4] = vec2<f32>(-1.0,  1.0);
    corner_offsets[5] = vec2<f32>(-1.0, -1.0);

    let offset = corner_offsets[corner_index];

    // NDC position with offset (no need to multiply by w since we're already in NDC)
    let final_ndc = vec4<f32>(
        ndc_x + offset.x * ndc_size_x * 0.5,
        ndc_y + offset.y * ndc_size_y * 0.5,
        ndc_z,
        1.0
    );

    var out: VertexOutput;
    out.clip_position = final_ndc;
    out.color = color;
    out.point_coord = offset;
    out.view_depth = depth;
    out.density = density;

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Calculate fog factor (0 = no fog, 1 = full fog)
    let fog_factor = clamp(
        (in.view_depth - camera.fog_near) / (camera.fog_far - camera.fog_near),
        0.0, 1.0
    );

    // Calculate luminance for desaturation
    let lum = dot(in.color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let gray = vec3<f32>(lum);

    // Apply saturation reduction (lerp toward gray)
    let sat_factor = mix(1.0, camera.fog_saturation, fog_factor);
    let desaturated = mix(gray, in.color.rgb, sat_factor);

    // Apply brightness reduction
    let bright_factor = mix(1.0, camera.fog_brightness, fog_factor);
    var final_color = desaturated * bright_factor;

    // Modulate by density for accumulation effect
    // Higher density = brighter (but with diminishing returns)
    let density_boost = 1.0 + in.density * 0.5;
    final_color = final_color * density_boost;

    return vec4<f32>(final_color, 1.0);
}
