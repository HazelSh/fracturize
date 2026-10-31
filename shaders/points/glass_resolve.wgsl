// Glass, the resolve (compiled after glass_common.wgsl). One fullscreen
// triangle per pane, viewport-clipped to it.
//
// The total opacity is exact, from the total absorbance; the weights only say
// how that opacity is shared among colours:
//
//     pixel = (sum colour*w / sum w) * (1 - T) + background * T,  T = exp(-tau)

@group(0) @binding(0) var<uniform> glass: Glass;
@group(0) @binding(1) var<storage, read> pixels: array<u32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
}

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn srgb_encode(c: f32) -> f32 {
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn srgb_decode(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

// A compositor blends the window's *encoded* bytes, so premultiply there.
fn premultiply_encoded(c: vec3<f32>, a: f32) -> vec3<f32> {
    return vec3<f32>(
        srgb_decode(srgb_encode(c.r) * a),
        srgb_decode(srgb_encode(c.g) * a),
        srgb_decode(srgb_encode(c.b) * a),
    );
}

@fragment
fn fs_resolve(in: VsOut) -> @location(0) vec4<f32> {
    let pix = u32(in.pos.y) * glass.fb_width + u32(in.pos.x);
    let base = pix * STRIDE;
    let tau = f32(pixels[base + TAU]) / TAU_SCALE;
    let a = 1.0 - exp(-tau);
    let w = f32(pixels[base + SUM_R + 3u]);
    var c = vec3<f32>(0.0);
    if w > 0.0 {
        c = vec3<f32>(f32(pixels[base + SUM_R]), f32(pixels[base + SUM_R + 1u]), f32(pixels[base + SUM_R + 2u])) / w;
    }
    if glass.alpha_mode > 1.5 {
        return vec4<f32>(premultiply_encoded(c, a), a);
    }
    if glass.alpha_mode > 0.5 {
        return vec4<f32>(c, a);
    }
    return vec4<f32>(mix(glass.background.rgb, c, a), 1.0);
}
