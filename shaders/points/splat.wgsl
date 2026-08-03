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
// (haze still thins far points, applied per-point before accumulation).

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

// Haze is applied per point before accumulation — there is no per-pixel depth
// in an additive framebuffer, so it has to ride on the point.
//
// Two separate effects, and keeping them separate is the whole point (see
// `src/haze.rs`). Distance *desaturates* the colour, and it *removes
// contribution*: a far point deposits less density, so the tonemap's coverage
// falls and the pixel resolves toward the background — whatever colour that
// is. Scaling the colour toward black instead, which is what this did before,
// only looks like distance against a black background; against a pale one it
// makes far material darker and therefore higher-contrast, which reads as
// nearer.

/// 0 at the near plane, 1 at the far plane.
fn haze_at(depth: f32) -> f32 {
    let range = camera.haze_far - camera.haze_near;
    return clamp((depth - camera.haze_near) / range, 0.0, 1.0);
}

/// The palette colour, desaturated for distance.
fn haze_color(color: vec3<f32>, depth: f32) -> vec3<f32> {
    let lum = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sat = mix(1.0, camera.haze_saturation, haze_at(depth));
    return mix(vec3<f32>(lum), color, sat);
}

/// The share of this point's density that survives the distance.
fn haze_weight(depth: f32) -> f32 {
    return mix(1.0, camera.haze_transmittance, haze_at(depth));
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
    out.color = haze_color(lookup_color(point.color_idx), depth);
    out.offset = offset;
    // Total kernel energy is 1: peak scales inversely with splat area, then
    // the haze takes its share of it.
    out.weight = haze_weight(depth) * KERNEL_NORM / (radius_px * radius_px);
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
    out.color = haze_color(lookup_color(point.color_idx), depth);
    out.offset = vec2<f32>(0.0);
    out.weight = haze_weight(depth);
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
