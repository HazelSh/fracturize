// Chaos-game trace renderer: colored line segments with per-vertex alpha

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    screen_height: f32,
    point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
    haze_near: f32,
    haze_far: f32,
    haze_transmittance: f32,
    haze_saturation: f32,
    color_contrast: f32,
    // Linear RGB, in what used to be tail padding — haze fades toward it.
    bg_r: f32,
    bg_g: f32,
    bg_b: f32,
    // 1 when this pass writes an image with an alpha channel to keep.
    transparent: f32,
    // Tail of `CameraUniforms` in gpu/buffers.rs: the colour mode and the
    // infinite-zoom edge guard, none of which this pass reads. Every pass
    // shares one buffer, so the struct still has to be the same size.
    max_point_pixels: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
    _pad6: f32,
}

@group(0) @binding(0) var<uniform> camera: CameraUniforms;

// A depth fade toward a fixed grey, over [near, far] along the view
// direction (`gpu::lines::Fade`). Strength 0 for every user but the warp grid.
struct Fade {
    eye: vec3<f32>,
    near: f32,
    forward: vec3<f32>,
    far: f32,
    grey: vec3<f32>,
    strength: f32,
}

@group(0) @binding(1) var<uniform> fade: Fade;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    out.depth = dot(in.position - fade.eye, fade.forward);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = clamp((in.depth - fade.near) / max(fade.far - fade.near, 1e-6), 0.0, 1.0);
    return vec4<f32>(mix(in.color.rgb, fade.grey, t * fade.strength), in.color.a);
}
