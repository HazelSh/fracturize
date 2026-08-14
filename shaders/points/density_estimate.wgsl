// Density estimation: a variable-width blur, wide where the histogram is
// sparse and narrow where it is dense.
//
// This is the thing that most distinguishes an Apophysis render, and it is
// **not substitutable by more samples**. More samples improve the whole image
// proportionally; DE targets exactly the regions still noisy at a finite
// budget. A fixed reconstruction filter has to trade detail for smoothness
// uniformly — crank it enough to kill grain in a void and you have softened the
// filament beside it. DE is what lets both coexist.
//
// ## Why a mip pyramid and not a summed-area table
//
// RENDER-QUALITY-PLAN.md proposes a SAT: 4 taps for any radius, O(pixels)
// instead of O(pixels x r²). The arithmetic is right and the precision is not.
// A SAT accumulates the whole image into its far corner, and a box query is the
// *difference* of two large nearly-equal sums. On a plausible accumulating
// render — 1600x1200 texels, mean density ~2e4 — the corner reaches ~4e10,
// where f32's 24-bit mantissa resolves steps of ~2.3e3. The faint tail texels
// DE exists to smooth are worth ~3. The error is **715x the signal**, and it is
// worst exactly where the feature matters.
//
// A 64-bit fixed-point SAT would work, at another full-size buffer on top of a
// histogram already measured in gigabytes, plus a two-pass prefix sum.
//
// A mip pyramid avoids the problem rather than paying for it: every level is an
// *average*, so magnitudes stay bounded and f32 keeps full relative precision
// at every level. Cost is 4/3 of the base texture and one pass per level. The
// blur it produces is a box of radius 2^level rather than a gaussian, which is
// softened by sampling bilinearly within a level and lerping between adjacent
// levels — the standard variable-radius blur, and the part the plan said to
// validate before relying on the SAT trick.

struct DeParams {
    /// Largest blur radius, in accumulation texels. A cap, not a target.
    max_radius: f32,
    /// Samples per texel considered "enough". A texel with this many is left
    /// alone; below it, the radius grows to borrow neighbours until the area
    /// covers about this many between them. See `fs_estimate`.
    target_density: f32,
    /// Highest pyramid level that exists, so the lerp cannot read past it.
    max_level: f32,
    _pad: f32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: DeParams;

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) idx: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(idx & 1u) * 4 - 1);
    let y = f32(i32(idx >> 1u) * 4 - 1);
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// Level 0 of the pyramid is the source, unchanged.
//
// A pass rather than a texture copy because the source is somebody else's
// texture — the histogram's resolve — and requiring COPY_SRC on it to save one
// fullscreen blit would push a usage flag up through the whole accumulator for
// no gain.
@fragment
fn fs_copy(in: VsOut) -> @location(0) vec4<f32> {
    return textureLoad(src, vec2<i32>(floor(in.clip_position.xy)), 0);
}

// === Pyramid construction ===
//
// One level from the one above it: a plain 2x2 box average. `src` is bound as a
// single-level view, so `textureDimensions` and `textureLoad(.., 0)` refer to
// the source level and this needs no uniform telling it which level it is on.
//
// Averaging rather than summing is what keeps f32 exact here: a level's values
// stay in the same range as level 0's however deep the pyramid goes.

@fragment
fn fs_reduce(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(src, 0));
    let p = vec2<i32>(floor(in.clip_position.xy)) * 2;

    // A 4x4 **tent**, not a 2x2 box. The box was the obvious thing and it looks
    // wrong: repeated box reduction keeps the kernel square, and at level 3 the
    // blur is a visible 8-texel block. Whole flowers came out as rectangles.
    //
    // Binomial 1-3-3-1 on each axis is one step of a gaussian approximation, so
    // stacking levels converges toward a gaussian instead of staying square,
    // for 16 taps on a target that is a quarter the size of its source.
    var acc = vec4<f32>(0.0);
    let w = array<f32, 4>(1.0, 3.0, 3.0, 1.0);
    for (var j = 0; j < 4; j = j + 1) {
        // Clamp rather than wrap: an odd-sized level would otherwise fold the
        // far edge of the image into the near one.
        let y = clamp(p.y + j - 1, 0, dims.y - 1);
        for (var i = 0; i < 4; i = i + 1) {
            let x = clamp(p.x + i - 1, 0, dims.x - 1);
            acc = acc + textureLoad(src, vec2<i32>(x, y), 0) * (w[i] * w[j]);
        }
    }
    return acc / 64.0;
}

// === The estimate ===

/// Bilinear fetch from one pyramid level, in level-0 coordinates.
///
/// Manual rather than through a sampler because `Rgba32Float` is not filterable
/// without an optional feature, and the alternative — dropping to fp16 for the
/// pyramid — would reintroduce exactly the stall this renderer already had to
/// engineer around in the accumulator.
fn sample_level(coord: vec2<f32>, level: i32) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(src, level));
    let scale = exp2(f32(level));
    // Level-0 texel centre -> this level's texel space.
    let c = (coord + 0.5) / scale - 0.5;
    let base = vec2<i32>(floor(c));
    let f = fract(c);
    let x0 = clamp(base.x, 0, dims.x - 1);
    let y0 = clamp(base.y, 0, dims.y - 1);
    let x1 = clamp(base.x + 1, 0, dims.x - 1);
    let y1 = clamp(base.y + 1, 0, dims.y - 1);
    let a = textureLoad(src, vec2<i32>(x0, y0), level);
    let b = textureLoad(src, vec2<i32>(x1, y0), level);
    let c0 = textureLoad(src, vec2<i32>(x0, y1), level);
    let d = textureLoad(src, vec2<i32>(x1, y1), level);
    return mix(mix(a, b, f.x), mix(c0, d, f.x), f.y);
}

@fragment
fn fs_estimate(in: VsOut) -> @location(0) vec4<f32> {
    let coord = floor(in.clip_position.xy);
    let here = textureLoad(src, vec2<i32>(coord), 0);
    let density = here.a;

    // Radius from the *unblurred* density, which is the only stable choice:
    // picking it from the blurred value would make the blur feed itself.
    //
    // The law is `sqrt(target / density)`, and it is derived rather than tuned.
    // Sampling noise falls as 1/sqrt(N), so to bring a texel holding `density`
    // samples up to the quality of one holding `target`, it needs to average
    // over `target / density` texels — an area, so the radius is the square
    // root of it. Anything already at or past `target` gets a radius below 1
    // and is left strictly alone.
    //
    // flam3's `estimator` / `estimator_curve` pair is the same idea with the
    // exponent left free; fixing it at the value the noise model implies is
    // what lets this be one control instead of three. The first attempt here
    // used flam3's 0.4 with a radius scale, and it blurred filaments and voids
    // nearly equally — the exponent was doing the work and it was the wrong
    // exponent.
    //
    // Density is in accumulated units, so it grows with the sample count and
    // the radius therefore *shrinks* as a render runs longer. That is correct:
    // more samples means less noise means less blur needed, and it makes DE
    // fade out of a converged image rather than softening it permanently.
    //
    // ## The radius has to ask its neighbourhood too
    //
    // flam3's DE is a **scatter**: each bucket spreads outward with a width set
    // by *its own* density, so a dense filament has a narrow kernel and stays
    // where it is. This is a **gather**, which is the only affordable shape on
    // a GPU — and gathering inverts the asymmetry. An empty texel beside a
    // bright filament has near-zero density, so it takes the widest radius on
    // offer and reaches *in*, dragging the filament out into the void.
    //
    // That is what the first version did, and the symptom was unmistakable
    // once measured: the effect of DE did not diminish at all between 30 and
    // 3000 samples per pixel, because empty texels stay empty however long you
    // render, and they surround everything. The picture bloomed instead of
    // de-noising.
    //
    // The fix is one extra tap: take a second opinion on the density from the
    // neighbourhood *at the scale the first guess would have blurred over*, and
    // use whichever is denser. Beside a filament the neighbourhood is bright,
    // so the radius collapses and the void stays dark; in genuine sparse haze
    // both readings are low and the wide kernel survives.
    let first = min(sqrt(params.target_density / max(density, 1e-6)), params.max_radius);
    let probe = clamp(log2(max(first, 1.0)), 0.0, params.max_level);
    let neighbourhood = sample_level(coord, i32(round(probe))).a;
    let effective = max(density, neighbourhood);
    let radius = min(sqrt(params.target_density / max(effective, 1e-6)), params.max_radius);

    // Below one texel there is nothing to average, and returning early keeps
    // the dense, detailed parts of the image bit-identical to no DE at all.
    if radius <= 1.0 {
        return here;
    }

    // A level's box covers 2^level texels, so the level that matches a radius
    // is its log2. Lerping between adjacent levels is what turns a stack of
    // discrete box blurs into something that varies smoothly across the image.
    let level = clamp(log2(radius), 0.0, params.max_level);
    let lo = i32(floor(level));
    let hi = min(lo + 1, i32(params.max_level));
    let t = level - floor(level);
    let blurred = mix(sample_level(coord, lo), sample_level(coord, hi), t);

    // All four channels blur with the *same* radius, chosen from alpha. Colour
    // is stored premultiplied by density (`colour * weight, weight`), so
    // blurring the channels independently of the weight would drift the hue
    // wherever the radius varies.
    return blurred;
}
