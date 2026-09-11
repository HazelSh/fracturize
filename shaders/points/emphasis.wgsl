// The emphasis pass: light up the material that the selected transform moves.
//
// An overlay, deliberately. The obvious place for this is inside the point and
// splat shaders — modulate the colour there and be done — but that is two
// shaders to change, a wider CameraUniforms, and an offline renderer that has
// to be taught to ignore all of it. Added on top instead, it is one thing that
// composites the same way over either render mode and costs nothing at all
// when nothing is selected.
//
// What it draws is `sensitivity[i]`: how far point i travels per unit nudge of
// the transform under the cursor, measured by the chaos pass as it wrote the
// point (see the emphasis block in chaos.wgsl). Normalized against the mean
// over the same population, because a distance in world units means nothing on
// its own — a scene is as big as its author made it.
//
// # Why this is two passes and not one
//
// A pixel of a fractal is not one point, it is tens of them, and the first two
// attempts at drawing this both foundered on that:
//
//   added      the glow came out proportional to how many points landed on the
//              pixel, so it reported density — which is what the picture
//              already showed — and saturated to white wherever the attractor
//              was thick, sensitive or not.
//   max        the brightest point in the pixel won, and since a quarter of
//              all points are freshly touched by any given transform, almost
//              every pixel contained one. The whole figure lit up.
//
// Neither is a summary of "does the material here move"; the summary that is,
// is the **mean over the points that landed on the pixel**. So the first pass
// accumulates sensitivity and weight together, exactly as the splat renderer
// accumulates colour and density, and the second divides them. Unlit points
// have to be drawn too — they are the denominator, and dropping them is what
// turns one lit point in twenty-five into a lit pixel.

struct Point {
    position: vec3<f32>,
    color_idx: u32,
}

// Field for field with CameraUniforms in buffers.rs; only the projection and
// the point sizing are read here.
struct CameraUniforms {
    view_proj: mat4x4<f32>,
    screen_height: f32,
    projected_point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
    haze_near: f32,
    haze_far: f32,
    haze_transmittance: f32,
    haze_saturation: f32,
    color_contrast: f32,
    bg_r: f32,
    bg_g: f32,
    bg_b: f32,
    transparent: f32,
    color_rgb_mode: f32,
    guard_x: f32,
    guard_y: f32,
    guard_z: f32,
    guard_ln_near: f32,
    guard_inv_ln_width: f32,
    max_point_pixels: f32,
    subfrustum: vec4<f32>,
}

struct EmphasisParams {
    // The run of the ring that belongs to the current target. Instance i is
    // point run_start + i, so the pass draws only points whose sensitivity is
    // about the transform now selected, and the glow grows in over a lap of
    // the ring instead of showing a moment of the last one.
    run_start: u32,
    run_len: u32,
    capacity: u32,
    /// 1 when the frame under this was drawn by the splat renderer.
    ///
    /// The footprint and the weighting below are then the splat renderer's
    /// own, rather than the point renderer's — so the glow is a summary of
    /// *the picture on screen*, sampled the way that picture was, instead of
    /// a second, differently-drawn cloud laid over it.
    splat_mode: u32,
    /// Multiple of the mean sensitivity that counts as "this moves": the knee
    /// of the response curve in `lit`.
    knee: f32,
    /// Gain on the composited glow.
    strength: f32,
    /// How far the unlit material is taken down toward the background, 0..1.
    dim: f32,
    /// How much brighter fully-lit material is made: the *multiplier* is
    /// 1 + glow_gain, not an amount of white added. See `fs_brighten`.
    glow_gain: f32,
    /// Tint of that brightening, linear RGB. Near-white, so it lifts the
    /// material rather than recolouring it.
    color: vec3<f32>,
    /// The background, linear RGB: what the unlit material recedes toward.
    background: vec3<f32>,
    _pad1: f32,
}

// The splat renderer's kernel, to the digit — see `shaders/points/splat.wgsl`,
// which is where these are explained. Sharing the numbers is the point: a
// summary of a picture has to be gathered the way the picture was.
const KERNEL_SHARPNESS: f32 = 4.0;
const KERNEL_NORM: f32 = 1.29;

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;
@group(0) @binding(2) var<storage, read> sensitivity: array<f32>;
// [0] = mean sensitivity over the run, [1] = 1 once that mean is real.
@group(0) @binding(3) var<storage, read> stats: array<f32>;
@group(0) @binding(4) var<uniform> params: EmphasisParams;

struct GlowOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) offset: vec2<f32>,
    @location(1) intensity: f32,
    // The weight this point deposits at the centre of its footprint: 1 for the
    // point renderer, which draws every dot the same, and the splat renderer's
    // own energy-conserving peak under `splat_mode`.
    @location(2) energy: f32,
}

fn offscreen() -> GlowOutput {
    var out: GlowOutput;
    out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
    out.offset = vec2<f32>(0.0);
    out.intensity = 0.0;
    out.energy = 0.0;
    return out;
}

// How lit one point is, from its sensitivity.
//
// x is the sensitivity in units of `knee` mean sensitivities, and the curve is
// x⁴/(1+x⁴): zero slope at the origin, half at the knee, saturating above it.
// The fourth power rather than a square because the populations being
// separated are one application of the transform apart, and so differ by that
// transform's own contraction — a factor of two, typically. A gentler curve
// paints most of the attractor a dim grey and says nothing.
fn lit(s: f32) -> f32 {
    if stats[1] < 0.5 {
        return 0.0;
    }
    let reference = max(stats[0], 1e-20) * max(params.knee, 1e-6);
    let x2 = (s / reference) * (s / reference);
    return (x2 * x2) / (1.0 + x2 * x2);
}

// ---------------------------------------------------------------- accumulate

@vertex
fn vs_glow(
    @builtin(vertex_index) corner_index: u32,
    @builtin(instance_index) instance: u32,
) -> GlowOutput {
    if instance >= params.run_len {
        return offscreen();
    }
    let idx = (params.run_start + instance) % params.capacity;

    let point = points[idx];
    let clip = camera.view_proj * vec4<f32>(point.position, 1.0);
    let depth = clip.w;
    if depth <= 0.0 {
        return offscreen();
    }
    let ndc = clip.xyz / clip.w;

    // Whatever the renderer under this would have drawn for the same point,
    // to the same clamps — see `vs_main` in points/render.wgsl and `vs_splat`
    // in points/splat.wgsl. `half_px` is the footprint's half-extent.
    let base = camera.projected_point_size * camera.screen_height / depth;
    var half_px = 0.0;
    var energy = 1.0;
    if params.splat_mode != 0u {
        let radius_px = clamp(0.5 * base, 1.0, camera.max_point_pixels);
        half_px = radius_px;
        // Total kernel energy 1, as the splat pass does it: a near point
        // spread over many pixels weighs the same in total as a far one on a
        // single pixel, so the ratio in `fs_composite` is a *density*-weighted
        // mean and matches what the tonemap made of the same material.
        energy = KERNEL_NORM / (radius_px * radius_px);
    } else {
        let size_pixels = clamp(base, camera.min_point_pixels, camera.max_point_pixels);
        half_px = size_pixels * 0.5;
    }

    let ndc_size_y = half_px / camera.screen_height * 2.0;
    let ndc_size_x = ndc_size_y / camera.aspect_ratio;

    let offset = vec2<f32>(
        f32(corner_index & 1u) * 2.0 - 1.0,
        f32(corner_index >> 1u) * 2.0 - 1.0,
    );

    // Same subfrustum window as every other pass: position and size both, or
    // a tile would glow at the wrong size. See CameraUniforms in buffers.rs.
    var out: GlowOutput;
    out.clip_position = vec4<f32>(
        ndc.x * camera.subfrustum.x + camera.subfrustum.z
            + offset.x * ndc_size_x * camera.subfrustum.x,
        ndc.y * camera.subfrustum.y + camera.subfrustum.w
            + offset.y * ndc_size_y * camera.subfrustum.y,
        ndc.z,
        1.0,
    );
    out.offset = offset;
    out.intensity = lit(sensitivity[idx]);
    out.energy = energy;
    return out;
}

// r = Σ lit·w, g = Σ w. Additive, into a two-channel float target.
@fragment
fn fs_accumulate(in: GlowOutput) -> @location(0) vec4<f32> {
    let w = in.energy * exp(-KERNEL_SHARPNESS * dot(in.offset, in.offset));
    return vec4<f32>(in.intensity * w, w, 0.0, 0.0);
}

// ----------------------------------------------------------------- composite

@group(1) @binding(0) var accum: texture_2d<f32>;
@group(1) @binding(1) var accum_sampler: sampler;

struct FullscreenOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One oversized triangle rather than a quad: no diagonal seam, three vertices
// instead of four, and the clip does the rest.
@vertex
fn vs_composite(@builtin(vertex_index) idx: u32) -> FullscreenOut {
    let x = f32((idx << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(idx & 2u) * 2.0 - 1.0;
    var out: FullscreenOut;
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// How lit this *pixel* is: the mean over the material under it, which is what
// the accumulator's two channels are for.
fn pixel_glow(uv: vec2<f32>) -> f32 {
    let acc = textureSampleLevel(accum, accum_sampler, uv, 0.0);
    if acc.g <= 0.0 {
        return 0.0;
    }
    return clamp(acc.r / acc.g * params.strength, 0.0, 1.0);
}

// Step one: take the unlit material down toward the background.
//
// Premultiplied, for a One / OneMinusSrcAlpha blend, so this is exactly
// `mix(scene, background, dim·(1-g))`. It costs nothing where there is no
// material to dim, because the background blended toward the background is
// the background — so the pass needs no coverage threshold and has no edge to
// alias along. A threshold is what used to make this blocky.
@fragment
fn fs_dim(in: FullscreenOut) -> @location(0) vec4<f32> {
    let alpha = (1.0 - pixel_glow(in.uv)) * params.dim;
    return vec4<f32>(alpha * params.background, alpha);
}

// Step two: make the lit material *brighter*, rather than whiter.
//
// The first version of this replaced the lit material with the glow colour and
// the result was a flat white silhouette — it overwhelmed the picture and hid
// the structure inside the very region it was pointing at, which is the one
// place you most want to see it. Adding white instead of replacing had the
// same defect in miniature: a constant added to everything compresses the
// contrast it lands on and clips the highlights.
//
// Multiplying does not. Under a `Dst` / `One` blend the frame gets
//
//     out = dst·src + dst = dst·(1 + glow_gain·g·tint)
//
// so lit material is *lifted*, keeping every ratio inside it — the fractal's
// own structure survives being pointed at — and the near-white tint leaves its
// hue where the author put it.
@fragment
fn fs_brighten(in: FullscreenOut) -> @location(0) vec4<f32> {
    let lift = params.glow_gain * pixel_glow(in.uv);
    return vec4<f32>(lift * params.color, 0.0);
}
