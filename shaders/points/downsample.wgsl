// Reconstruction filter + downsample: N x N accumulation -> output pixels.
//
// This is the pass that turns a raw histogram into a photograph. Every point
// used to be deposited into exactly one pixel with no kernel and no filtering,
// so the image was hard-edged and crunchy however many samples landed in it —
// past ~33 samples/pixel more sampling stopped helping, because what was left
// was not shot noise but pixel quantization. Rendering N x larger and filtering
// down fixes exactly that, and measurably beats native rendering at the *same*
// total sample count.
//
// **The filtering has to happen on linear values, before any log tonemap.**
// Both sources here satisfy that: the splat path hands over its additive
// (colour*weight, weight) accumulation, and the points path hands over an sRGB
// texture, which `textureLoad` decodes to linear for us and the sRGB render
// target re-encodes on the way out. Filtering after the log would blur in a
// perceptually compressed space and visibly muddy bright cores.

struct DownsampleParams {
    /// Supersample factor N: the source is N x the output in both axes.
    scale: u32,
    /// Which kernel — see `kernel_weight`.
    kernel: u32,
    /// Kernel half-width in *output* pixels. The tap radius in source pixels
    /// is `support * scale`, which is how the two compose in flam3.
    support: f32,
    /// 1.0 when the source holds **straight** alpha (the points renderer's
    /// colour target): premultiply before averaging and unpremultiply after,
    /// or a filter that mixes an opaque red pixel with a transparent one
    /// darkens the red instead of leaving it alone.
    ///
    /// 0.0 when the source is already additive (the splat accumulation, which
    /// is (colour*weight, weight) and averages correctly channel by channel —
    /// the tonemap's `rgb / a` recovers the density-weighted mean either way).
    straight_alpha: f32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: DownsampleParams;

const KERNEL_BOX: u32 = 0u;
const KERNEL_TRIANGLE: u32 = 1u;
const KERNEL_GAUSSIAN: u32 = 2u;
const KERNEL_MITCHELL: u32 = 3u;
const KERNEL_LANCZOS: u32 = 4u;

/// Value of exp(-4), subtracted from the gaussian so it reaches exactly zero at
/// the edge of its support instead of stepping down from 0.018. A truncated
/// gaussian's step is small but it is a step, and it lands on a hard ring
/// around every bright core.
const GAUSS_EDGE: f32 = 0.0183156389;

fn sinc(x: f32) -> f32 {
    if abs(x) < 1e-6 {
        return 1.0;
    }
    let p = 3.14159265 * x;
    return sin(p) / p;
}

/// One axis of the kernel, at `t` in units of the support (so the kernel is
/// zero for |t| >= 1 and the caller never has to know the kernel's own scale).
fn kernel_weight(t: f32) -> f32 {
    let a = abs(t);
    if a >= 1.0 {
        return 0.0;
    }
    switch params.kernel {
        case KERNEL_TRIANGLE: {
            return 1.0 - a;
        }
        case KERNEL_GAUSSIAN: {
            return exp(-4.0 * t * t) - GAUSS_EDGE;
        }
        case KERNEL_MITCHELL: {
            // Mitchell-Netravali with B = C = 1/3, the parameters Mitchell and
            // Netravali picked as the best blur/ringing compromise. Defined
            // over |x| <= 2, so the support maps to x = 2a.
            let x = 2.0 * a;
            let b = 1.0 / 3.0;
            let c = 1.0 / 3.0;
            if x < 1.0 {
                return ((12.0 - 9.0 * b - 6.0 * c) * x * x * x
                    + (-18.0 + 12.0 * b + 6.0 * c) * x * x
                    + (6.0 - 2.0 * b)) / 6.0;
            }
            return ((-b - 6.0 * c) * x * x * x
                + (6.0 * b + 30.0 * c) * x * x
                + (-12.0 * b - 48.0 * c) * x
                + (8.0 * b + 24.0 * c)) / 6.0;
        }
        case KERNEL_LANCZOS: {
            // a = 2. Sharpest of the five, and the one *not* to default to:
            // its negative lobes ring around small bright cores, which is what
            // a flame image is full of.
            let x = 2.0 * a;
            return sinc(x) * sinc(x * 0.5);
        }
        default: {
            // KERNEL_BOX. At support 0.5 this is exactly an N x N block
            // average — the arrangement the baseline measured.
            return 1.0;
        }
    }
}

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
}

@vertex
fn vs_downsample(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle, same trick as the tonemap pass
    var out: VsOut;
    let x = f32(i32(idx & 1u) * 4 - 1);
    let y = f32(i32(idx >> 1u) * 4 - 1);
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_downsample(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(src, 0));
    let n = f32(params.scale);
    let straight = params.straight_alpha > 0.5;

    // Centre of this output pixel, in source pixels.
    let centre = (floor(in.clip_position.xy) + 0.5) * n;
    let radius = params.support * n;

    let lo = vec2<i32>(floor(centre - radius));
    let hi = vec2<i32>(ceil(centre + radius));

    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var sy = lo.y; sy <= hi.y; sy = sy + 1) {
        // Distance from the output pixel's centre, expressed in *output*
        // pixels, then in units of the support.
        let ty = ((f32(sy) + 0.5) - centre.y) / n / params.support;
        let wy = kernel_weight(ty);
        if wy == 0.0 {
            continue;
        }
        for (var sx = lo.x; sx <= hi.x; sx = sx + 1) {
            let tx = ((f32(sx) + 0.5) - centre.x) / n / params.support;
            let w = wy * kernel_weight(tx);
            if w == 0.0 {
                continue;
            }
            // Clamp-to-edge: a kernel wider than half a source pixel reaches
            // past the border on the outermost output pixels.
            let p = clamp(vec2<i32>(sx, sy), vec2<i32>(0), dims - vec2<i32>(1));
            var v = textureLoad(src, p, 0);
            if straight {
                v = vec4<f32>(v.rgb * v.a, v.a);
            }
            acc = acc + v * w;
            wsum = wsum + w;
        }
    }

    if wsum == 0.0 {
        // Cannot happen for any kernel offered here — every one is non-zero at
        // t = 0 and the centre tap is always inside the support — but a
        // divide by zero would be a NaN in the output file rather than a
        // visible mistake, so it is guarded rather than assumed.
        return vec4<f32>(0.0);
    }
    var out = acc / wsum;

    // Lanczos and Mitchell have negative lobes, so a filtered density can come
    // out below zero next to a hot core. Negative density is not a darker
    // pixel, it is a NaN once the tonemap takes its log.
    out = max(out, vec4<f32>(0.0));

    if straight {
        if out.a <= 0.0 {
            return vec4<f32>(0.0);
        }
        return vec4<f32>(out.rgb / out.a, out.a);
    }
    return out;
}
