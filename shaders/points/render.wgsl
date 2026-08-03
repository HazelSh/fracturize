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
    // 1 when point colour is packed RGB rather than a colormap index
    color_rgb_mode: f32,
    _pad0: f32,
    _pad1: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) depth: f32,
}

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;
@group(0) @binding(2) var<storage, read> colormap: array<vec4<f32>, 256>;

// Colormap lookup with a cyclic contrast stretch: deviations from the
// palette center are amplified and wrap around (the colormap is cyclic),
// restoring color amplitude when scale-aware accumulation compresses the
// index range toward the mean. contrast = 1 is an exact passthrough.
// The mixed-RGB path: 8/8/8 out of the top 24 bits, sqrt-companded (see
// `pack_rgb` in chaos.wgsl). No colormap, and no contrast stretch — that is a
// cyclic rescale of a 1-D *index*, and there is no index here to rescale.
fn unpack_rgb(packed: u32) -> vec3<f32> {
    let v = vec3<f32>(
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
        f32((packed >> 24u) & 0xFFu),
    ) / 255.0;
    return v * v;
}

fn lookup_color(color_idx: u32) -> vec3<f32> {
    if camera.color_rgb_mode > 0.5 {
        return unpack_rgb(color_idx);
    }
    let f = (f32(color_idx & 0xFFu) + 0.5) / 256.0;
    let stretched = fract(0.5 + (f - 0.5) * camera.color_contrast);
    return colormap[u32(stretched * 256.0) & 0xFFu].rgb;
}

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

    // Point size decreases with depth (perspective). The upper clamp stops
    // points right next to the camera from ballooning into giant squares
    // that occlude the scene (volume-filling scenes put points arbitrarily
    // close to the eye when zoomed in) — near-field dots stay dots.
    let base_size = camera.point_size * camera.screen_height / depth;
    let size_pixels = clamp(base_size, camera.min_point_pixels, 12.0);

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
    out.color = lookup_color(point.color_idx);
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
    out.color = lookup_color(point.color_idx);
    out.depth = depth;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Atmospheric haze: distant material dissolves *into the background*, and
    // loses saturation on the way. It used to multiply the colour toward black
    // instead, which is only the same thing when the background happens to be
    // black — put a fractal on a pale background that way and the far material
    // gains contrast with distance, which reads as nearer, not further. See
    // `src/haze.rs`.
    let range = camera.haze_far - camera.haze_near;
    let t = clamp((in.depth - camera.haze_near) / range, 0.0, 1.0);

    let lum = dot(in.color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sat = mix(1.0, camera.haze_saturation, t);
    let desaturated = mix(vec3<f32>(lum), in.color, sat);

    let transmittance = mix(1.0, camera.haze_transmittance, t);
    let background = vec3<f32>(camera.bg_r, camera.bg_g, camera.bg_b);

    if camera.transparent > 0.5 {
        // Writing a file with an alpha channel: the point's own colour, and
        // the haze spent as *transparency* rather than as a fade toward a
        // background that isn't going to be there. This is what makes a
        // transparent render composite correctly — fading to the background
        // colour at alpha 1 would bake the background into the far material,
        // which is exactly what you asked for transparency to avoid. The pass
        // clears to alpha 0, so pixels with no point in them stay empty.
        return vec4<f32>(desaturated, transmittance);
    }
    return vec4<f32>(mix(background, desaturated, transmittance), 1.0);
}
