// Additive log-density splat renderer ("flame" accumulation)
//
// Two passes over the same point buffer the plain point renderer uses:
//
// 1. Accumulate: every point is splatted into an rgba16float HDR target
//    with additive blending. rgb accumulates color*weight, alpha
//    accumulates weight (= density). A gaussian kernel spreads each
//    point's unit energy over its projected footprint, so total deposited
//    energy per point is ~1 regardless of splat size or zoom.
// 2. Tonemap: fullscreen pass mapping density through log2, flame-style.
//    Sparse points stay visible grit; dense cores get smooth log-density
//    gradients instead of flat saturation.
//
// There is no depth or occlusion: the fractal is treated as pure emission
// (fog still darkens far points, applied per-point before accumulation).

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
    color_contrast: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

struct SplatParams {
    background: vec4<f32>,
    /// exposure * K * screen_height^2 / point_capacity — scales raw density
    /// so brightness is invariant to resolution and point count
    exposure_scale: f32,
    /// Output gain applied after the log
    gain: f32,
    /// 1.0 = write straight-alpha coverage instead of compositing over
    /// `background` (transparent PNG output)
    transparent: f32,
    _pad0: f32,
}

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;
@group(0) @binding(2) var<storage, read> colormap: array<vec4<f32>, 256>;

// Same cyclic contrast stretch as render.wgsl
fn lookup_color(color_idx: u32) -> vec3<f32> {
    let f = (f32(color_idx & 0xFFu) + 0.5) / 256.0;
    let stretched = fract(0.5 + (f - 0.5) * camera.color_contrast);
    return colormap[u32(stretched * 256.0) & 0xFFu].rgb;
}

// Fog is applied per point before accumulation (there is no per-pixel
// depth in an additive framebuffer). Mirrors fs_main in render.wgsl.
fn fog_color(color: vec3<f32>, depth: f32) -> vec3<f32> {
    let fog_range = camera.fog_far - camera.fog_near;
    let fog_factor = clamp((depth - camera.fog_near) / fog_range, 0.0, 1.0);
    let lum = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sat_factor = mix(1.0, camera.fog_saturation, fog_factor);
    let desaturated = mix(vec3<f32>(lum), color, sat_factor);
    return desaturated * mix(1.0, camera.fog_brightness, fog_factor);
}

struct SplatOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    // Position within the splat, in kernel units (-1..1 at the quad edge)
    @location(1) offset: vec2<f32>,
    // Per-fragment energy normalization (peak of the gaussian)
    @location(2) weight: f32,
}

// Gaussian sharpness: exp(-4 r^2) is ~0.02 at the quad edge (r = 1)
const KERNEL_SHARPNESS: f32 = 4.0;
// 1 / integral of exp(-4 r^2) over the unit quad: peak weight so the
// kernel sums to 1 over a splat of radius 1 px
const KERNEL_NORM: f32 = 1.29;

@vertex
fn vs_splat(
    @builtin(vertex_index) corner_index: u32,
    @builtin(instance_index) point_index: u32,
) -> SplatOutput {
    let point = points[point_index];
    let clip = camera.view_proj * vec4<f32>(point.position, 1.0);
    let depth = clip.w;

    var out: SplatOutput;
    if depth <= 0.0 {
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        out.color = vec3<f32>(0.0);
        out.offset = vec2<f32>(0.0);
        out.weight = 0.0;
        return out;
    }

    let ndc = clip.xyz / clip.w;

    // Perspective splat radius in pixels, clamped so subpixel points still
    // deposit their energy and huge near-camera splats stay bounded. The
    // 12px cap keeps points brushing past the camera from smearing into
    // screen-sized gaussian blobs in volume-filling scenes (their unit of
    // energy still lands, just concentrated — a bright mote, not a wash).
    let base_radius = 0.5 * camera.point_size * camera.screen_height / depth;
    let radius_px = clamp(base_radius, 1.0, 12.0);

    let ndc_size_y = radius_px / camera.screen_height * 2.0;
    let ndc_size_x = ndc_size_y / camera.aspect_ratio;

    let offset = vec2<f32>(
        f32(corner_index & 1u) * 2.0 - 1.0,
        f32(corner_index >> 1u) * 2.0 - 1.0,
    );

    out.clip_position = vec4<f32>(
        ndc.x + offset.x * ndc_size_x,
        ndc.y + offset.y * ndc_size_y,
        0.0,
        1.0,
    );
    out.color = fog_color(lookup_color(point.color_idx), depth);
    out.offset = offset;
    // Total kernel energy is 1: peak scales inversely with splat area
    out.weight = KERNEL_NORM / (radius_px * radius_px);
    return out;
}

// Fast path for subpixel points: native 1px point primitives depositing
// their full unit energy into a single pixel
@vertex
fn vs_splat_point(@builtin(vertex_index) point_index: u32) -> SplatOutput {
    let point = points[point_index];
    let clip = camera.view_proj * vec4<f32>(point.position, 1.0);
    let depth = clip.w;

    var out: SplatOutput;
    if depth <= 0.0 {
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        out.color = vec3<f32>(0.0);
        out.offset = vec2<f32>(0.0);
        out.weight = 0.0;
        return out;
    }

    out.clip_position = vec4<f32>(clip.xy / clip.w, 0.0, 1.0);
    out.color = fog_color(lookup_color(point.color_idx), depth);
    out.offset = vec2<f32>(0.0);
    out.weight = 1.0;
    return out;
}

@fragment
fn fs_splat(in: SplatOutput) -> @location(0) vec4<f32> {
    let w = in.weight * exp(-KERNEL_SHARPNESS * dot(in.offset, in.offset));
    return vec4<f32>(in.color * w, w);
}

// === Tonemap pass ===

@group(0) @binding(0) var accum: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: SplatParams;

struct TonemapOutput {
    @builtin(position) clip_position: vec4<f32>,
}

@vertex
fn vs_tonemap(@builtin(vertex_index) idx: u32) -> TonemapOutput {
    // Fullscreen triangle
    var out: TonemapOutput;
    let x = f32(i32(idx & 1u) * 4 - 1);
    let y = f32(i32(idx >> 1u) * 4 - 1);
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_tonemap(in: TonemapOutput) -> @location(0) vec4<f32> {
    let transparent = params.transparent > 0.5;
    let acc = textureLoad(accum, vec2<i32>(in.clip_position.xy), 0);
    let density = acc.a;
    if density <= 0.0 {
        if transparent {
            return vec4<f32>(0.0);
        }
        return vec4<f32>(params.background.rgb, 1.0);
    }
    // Density-weighted mean color, brightness from log density
    let mean_color = acc.rgb / density;
    let brightness = log2(1.0 + density * params.exposure_scale) * params.gain;
    // Coverage, not gain. This used to be `background + mean_color *
    // brightness` — pure emission, which is right only if the background is
    // black. Once the background became a scene parameter it stopped being:
    // add a fractal to a light background and every pixel clips to white,
    // which is what happened the first time this was tried at 0.9 grey.
    //
    // Treating log-density as *coverage* and compositing fixes that and
    // unifies the two output modes — the opaque render is now exactly the
    // transparent one composited over the background, one model instead of
    // two. On the near-black default background the difference from the old
    // formula is bounded by the background itself (~2% of full scale), so
    // existing scenes are visually unchanged.
    let coverage = clamp(brightness, 0.0, 1.0);
    if transparent {
        // Straight (non-premultiplied) alpha, which is what PNG stores:
        // colour is the palette hue, coverage is the log density, so the
        // dusty edges stay dusty instead of turning into a cutout.
        return vec4<f32>(mean_color, coverage);
    }
    return vec4<f32>(mix(params.background.rgb, mean_color, coverage), 1.0);
}
