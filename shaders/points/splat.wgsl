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
//
// VOLUME MODE adds absorption to that, with two more entry points over the
// same vertex stage. `fs_abs` is pass A: it splats each point's *absorbance*
// into fp32 targets as a total and four power moments of warped depth. Then
// `fs_splat_volume` is the ordinary accumulation with each fragment's weight
// multiplied by the transmittance reconstructed from its pixel's moments
// (`moment_transmittance`, after Munstermann et al. 2018). With opacity 0 none
// of this runs: the host draws with `fs_splat` exactly as before.

struct Point {
    position: vec3<f32>,
    color_idx: u32,
}

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    screen_height: f32,
    // The scene's point size, already scaled for the field of view on the CPU
    // (`Fov::point_scale`). Equal to the authored size at the default 45
    // degree FoV; smaller as the lens widens, because a wider lens shrinks the
    // whole world on screen and a point has to shrink with it.
    projected_point_size: f32,
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
    // 1 when this pass writes an image with an alpha channel to keep, 2 when
    // it is a --transparent-window swapchain (premultiplied; see Alpha).
    transparent: f32,
    // 1 when point colour is packed RGB rather than a colormap index
    color_rgb_mode: f32,
    // Infinite-zoom edge guard; see guard_weight() below and renorm.rs.
    guard_x: f32,
    guard_y: f32,
    guard_z: f32,
    guard_ln_near: f32,
    guard_inv_ln_width: f32,
    max_point_pixels: f32,
    /// This tile's window onto the frame: xy scale, zw offset, applied in NDC.
    /// (1, 1, 0, 0) is the whole frame and is what every untiled render uses.
    ///
    /// It has to reach the shader rather than being folded into view_proj,
    /// because a splat's position is projected but its *size* is added
    /// afterwards as an NDC quad offset — see below. Scaling one without the
    /// other gives every tile points of the wrong size.
    subfrustum: vec4<f32>,
}

struct SplatParams {
    background: vec4<f32>,
    /// exposure * K * screen_height^2 / point_capacity — scales raw density
    /// so brightness is invariant to resolution and point count
    exposure_scale: f32,
    /// Output gain applied after the log
    gain: f32,
    /// 1.0 = write straight-alpha coverage instead of compositing over
    /// `background` (transparent PNG output); 2.0 = premultiplied, for a
    /// --transparent-window swapchain
    transparent: f32,
    /// **1/gamma**, pre-inverted on the CPU so the shader never divides.
    /// 1.0 is off and reproduces every render made before this existed.
    gamma_inv: f32,
    /// Coverage below which the gamma curve is blended toward a straight line.
    /// 0 disables the toe. See `gamma_curve`.
    gamma_threshold: f32,
    /// 1 = gamma reaches the picture through coverage alone (hue untouched),
    /// 0 = through each colour channel (highlights roll off toward white).
    vibrancy: f32,
    /// 1 LSB of triangular dither before the hardware quantizes to 8 bits.
    /// 0 is off, and off is exact — see `dither_srgb`.
    dither: f32,
    /// Volume mode's tonemap only: the supersample factor, so it can average
    /// the absorbance field over an output pixel's block. 0 for the plain
    /// tonemap, which never reads it.
    volume_n: f32,
}

/// Volume mode's per-frame numbers (16 bytes; `VolumeParams` in volume.rs).
struct VolumeParams {
    /// 1 when the host is drawing the volume passes, else 0. The vertex stage
    /// skips the depth and absorbance work entirely when it is 0.
    enabled: f32,
    /// A point's absorbing cross-section in world area: opacity * K * L^2 /
    /// samples, with L the scene's reference length. The vertex stage divides
    /// it by the world area of a pixel at the point's depth (`volume_deposit`),
    /// so optical depth is a property of the material and not of where the
    /// camera stands, nor of the point count.
    abs_scale: f32,
    /// Pixels per world unit at unit depth: a pixel's world area at depth z is
    /// (z / ppu)^2.
    ppu: f32,
    /// The source function, in splat density units: how much emission a unit of
    /// absorbance brings with it (`volume::SOURCE`, scaled like exposure is by
    /// samples and height). See `fs_splat_volume`.
    source: f32,
}

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;
@group(0) @binding(2) var<storage, read> colormap: array<vec4<f32>, 256>;
@group(0) @binding(3) var<uniform> volume: VolumeParams;

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

/// The share that survives the infinite-zoom edge guard: 1 everywhere until
/// the band's outer octave, then smoothly to 0 before its edge. 1 always when
/// the scene has no zoom (or asked for a hard edge), which is the zero width.
///
/// The ramp is taken in **log radius**, and against a near plane that already
/// carries this frame's eye distance (`ln_near = ln(rho_start * d)`, filled in
/// by `CameraUniforms::with_zoom_guard`). Both of those are load-bearing:
/// dividing by the eye distance is what makes this survive a zoom wrap
/// unchanged — the wrap scales the point's radius and the eye's by the same
/// factor — and taking the ramp in log is what makes material leave the
/// picture at a constant rate per octave of zoom rather than in a rush at one
/// end. See `renorm::DEFAULT_EDGE_GUARD` for why the alternative (fading the
/// point deal instead) cannot work.
fn guard_weight(pos: vec3<f32>) -> f32 {
    if camera.guard_inv_ln_width == 0.0 {
        return 1.0;
    }
    let centre = vec3<f32>(camera.guard_x, camera.guard_y, camera.guard_z);
    let r = length(pos - centre);
    let t = (log(max(r, 1e-20)) - camera.guard_ln_near) * camera.guard_inv_ln_width;
    return 1.0 - smoothstep(0.0, 1.0, t);
}

struct SplatOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    // Position within the splat, in kernel units (-1..1 at the quad edge)
    @location(1) offset: vec2<f32>,
    // Per-fragment energy normalization (peak of the gaussian)
    @location(2) weight: f32,
    // Volume mode only (zero otherwise): the absorbance this point deposits,
    // before the kernel's falloff, and its view depth. Flat, because the depth
    // is the point's and not the fragment's.
    @location(3) abs_weight: f32,
    @location(4) @interpolate(flat) depth: f32,
}

/// A point's whole absorbance deposit, in pixels' worth of optical depth: its
/// world cross-section over the world area of a pixel at its depth.
fn volume_deposit(depth: f32) -> f32 {
    let s = volume.ppu / depth;
    return volume.abs_scale * s * s;
}

// Gaussian sharpness: exp(-4 r^2) is ~0.02 at the quad edge (r = 1)
const KERNEL_SHARPNESS: f32 = 4.0;
// 1 / integral of exp(-4 r^2) over the unit quad: peak weight so the
// kernel sums to 1 over a splat of radius 1 px
const KERNEL_NORM: f32 = 1.29;

// A point `--isolate` dropped (see ISOLATE_DROPPED in chaos.wgsl, which this
// must agree with). Reported as behind the camera, which every vertex shader
// here already clips. Greater-than rather than equal, so a rewrap that scales
// the sentinel still counts.
fn isolate_dropped(pos: vec3<f32>) -> bool {
    return abs(pos.x) > 1.0e36;
}

// Mute/solo, a view (src/gpu/points/mask.rs): which map placed each point,
// one up (0 = not recorded yet), and the mask — word 0 is "on", then one bit
// a GPU slot, set to hide. Sixteen-byte stand-ins with word 0 = 0 in every
// renderer that isn't the live window's, so `point_maps` is never read there.
@group(0) @binding(4) var<storage, read> point_maps: array<u32>;
@group(0) @binding(5) var<storage, read> map_mask: array<u32>;

// Whether a point is drawn at all: `--isolate` dropped it, or the mute/solo
// mask hides the map that placed it. A point whose map isn't recorded yet is
// hidden while the mask is on, so a solo fills in rather than flashing the
// whole picture first.
fn hidden(i: u32, pos: vec3<f32>) -> bool {
    if isolate_dropped(pos) {
        return true;
    }
    if map_mask[0] == 0u {
        return false;
    }
    let m = point_maps[i];
    if m == 0u {
        return true;
    }
    let s = m - 1u;
    return ((map_mask[1u + s / 32u] >> (s % 32u)) & 1u) != 0u;
}

@vertex
fn vs_splat(
    @builtin(vertex_index) corner_index: u32,
    @builtin(instance_index) point_index: u32,
) -> SplatOutput {
    let point = points[point_index];
    let clip = camera.view_proj * vec4<f32>(point.position, 1.0);
    let depth = select(clip.w, -1.0, hidden(point_index, point.position));

    var out: SplatOutput;
    if depth <= 0.0 {
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        out.color = vec3<f32>(0.0);
        out.offset = vec2<f32>(0.0);
        out.weight = 0.0;
        out.abs_weight = 0.0;
        out.depth = 0.0;
        return out;
    }

    // Project, then crop to this tile's window. The identity window leaves
    // this exactly as it was.
    let ndc_full = clip.xyz / clip.w;
    let ndc = vec3<f32>(
        ndc_full.x * camera.subfrustum.x + camera.subfrustum.z,
        ndc_full.y * camera.subfrustum.y + camera.subfrustum.w,
        ndc_full.z,
    );

    // Perspective splat radius in pixels of the *render target*, clamped so
    // subpixel points still deposit their energy and huge near-camera splats
    // stay bounded. The cap keeps points brushing past the camera from
    // smearing into screen-sized gaussian blobs in volume-filling scenes
    // (their unit of energy still lands, just concentrated — a bright mote,
    // not a wash).
    //
    // The cap arrives already scaled for supersampling (`max_point_pixels`,
    // 12 x N) because it means 12 *output* pixels; a literal 12 here would
    // tighten to 12/N output pixels and shrink close motes as quality went up.
    //
    // The floor stays at one target pixel, and deliberately does **not**
    // scale. One accumulation texel is the finest thing the target can
    // represent, and letting a splat be that small is exactly what
    // supersampling buys: at 4x a point a quarter of an output pixel across
    // now stays a quarter of an output pixel instead of being inflated to a
    // whole one. Scaling this bound with N would undo the feature for the
    // smallest material in the picture.
    let base_radius = 0.5 * camera.projected_point_size * camera.screen_height / depth;
    let radius_px = clamp(base_radius, 1.0, camera.max_point_pixels);

    // The same window scales the splat's size, not just its position. A tile
    // showing half the frame in the same number of texels magnifies by two,
    // and a point has to magnify with it or the tile comes out finer-grained
    // than its neighbours.
    let ndc_size_y = radius_px / camera.screen_height * 2.0 * camera.subfrustum.y;
    let ndc_size_x = (radius_px / camera.screen_height * 2.0 / camera.aspect_ratio)
        * camera.subfrustum.x;

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
    // the haze and the zoom's edge guard take their shares of it.
    let guard = guard_weight(point.position);
    out.weight = haze_weight(depth) * guard
        * KERNEL_NORM / (radius_px * radius_px);
    // Absorbance is a property of the material, so haze does not thin it; the
    // edge guard removes material, so it does.
    out.abs_weight = 0.0;
    out.depth = 0.0;
    if volume.enabled > 0.5 {
        // A point's deposit is its world cross-section over a pixel's world
        // area, which grows as 1/z^2 as the point nears the eye, and so does the
        // area its footprint is spread over — until the footprint is capped at
        // `max_point_pixels`. Past the cap the footprint stops growing and the
        // peak would climb as 1/z^2: a mote the eye brushes past would turn
        // optically solid. So beyond the cap the deposit is held at what it was
        // at the cap, in proportion to the area it covers.
        let cap = camera.max_point_pixels;
        let cap_share = select(1.0, (cap * cap) / (base_radius * base_radius), base_radius > cap);
        out.abs_weight = volume_deposit(depth) * cap_share * guard * KERNEL_NORM / (radius_px * radius_px);
        out.depth = depth;
    }
    return out;
}

// Fast path for subpixel points: native 1px point primitives depositing
// their full unit energy into a single pixel
@vertex
fn vs_splat_point(@builtin(vertex_index) point_index: u32) -> SplatOutput {
    let point = points[point_index];
    let clip = camera.view_proj * vec4<f32>(point.position, 1.0);
    let depth = select(clip.w, -1.0, hidden(point_index, point.position));

    var out: SplatOutput;
    if depth <= 0.0 {
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        out.color = vec3<f32>(0.0);
        out.offset = vec2<f32>(0.0);
        out.weight = 0.0;
        out.abs_weight = 0.0;
        out.depth = 0.0;
        return out;
    }

    // The same tile window as vs_splat. Only the position needs it here: a
    // point primitive is one texel wide by definition, and one texel is one
    // texel whatever part of the frame this tile is showing.
    let ndc = clip.xy / clip.w;
    out.clip_position = vec4<f32>(
        ndc.x * camera.subfrustum.x + camera.subfrustum.z,
        ndc.y * camera.subfrustum.y + camera.subfrustum.w,
        0.0,
        1.0,
    );
    out.color = haze_color(lookup_color(point.color_idx), depth);
    out.offset = vec2<f32>(0.0);
    let guard = guard_weight(point.position);
    out.weight = haze_weight(depth) * guard;
    out.abs_weight = 0.0;
    out.depth = 0.0;
    if volume.enabled > 0.5 {
        out.abs_weight = volume_deposit(depth) * guard;
        out.depth = depth;
    }
    return out;
}

@fragment
fn fs_splat(in: SplatOutput) -> @location(0) vec4<f32> {
    let w = in.weight * exp(-KERNEL_SHARPNESS * dot(in.offset, in.offset));
    return vec4<f32>(in.color * w, w);
}

// === Volume mode ===
//
// Pass A0 (`fs_front`) finds, for every pixel, the nearest and farthest depth any
// fragment lands at. Pass A (`fs_abs`) adds each fragment's absorbance to one of
// BINS equal slices of *that pixel's own span* (log-spaced between those two
// depths), as fixed point in a storage buffer. Pass B (`fs_splat_volume`) lights
// each fragment by the transmittance read off those slices: the absorbance in
// the slices in front of its own, plus the share of its own slice in front of
// it, own deposit excluded. Total absorbance is the sum of the slices.
//
// The pixel's own span is what matters. The same estimator over one depth range
// for the whole frame is 7x less accurate, because a pixel's material sits in a
// sliver of the attractor's depth; and it replaces a moment reconstruction that,
// scored against the exact transport (`volume_ref.rs`), was off by 0.2 in T and
// biased by its overestimation constant. See VOLUME-RENDERER-PLAN.md.

// Mirrored in `volume::BINS`; a test holds the two together.
const BINS: u32 = 32u;
// Absorbance is accumulated as u32 fixed point, 2^20 to the unit: a millionth
// of resolution, 4096 units of headroom a slice. After `fs_prefix` the same
// words hold f32 running totals instead.
const FIXED: f32 = 1048576.0;

// Per pixel, BINS words: the slices.
@group(1) @binding(0) var<storage, read_write> slices: array<atomic<u32>>;
// Per pixel, two words: the farthest depth's bits, and the *complement* of the
// nearest depth's bits, so that both are atomicMax and a cleared buffer means
// "nothing yet". Depths are positive floats, whose bits order like they do.
@group(1) @binding(1) var<storage, read_write> ranges: array<atomic<u32>>;

fn pixel_index(px: vec2<i32>) -> u32 {
    let w = u32(round(camera.screen_height * camera.aspect_ratio));
    return u32(px.y) * w + u32(px.x);
}

@fragment
fn fs_front(in: SplatOutput) {
    let base = pixel_index(vec2<i32>(in.clip_position.xy)) * 2u;
    let bits = bitcast<u32>(in.depth);
    // Read first: most fragments are neither the nearest nor the farthest, and
    // a plain load does not serialise like an atomic does.
    if bits > atomicLoad(&ranges[base + 1u]) {
        atomicMax(&ranges[base + 1u], bits);
    }
    if ~bits > atomicLoad(&ranges[base]) {
        atomicMax(&ranges[base], ~bits);
    }
}

// Between A0 and A, `fs_range` rewrites each pixel's span words as two floats:
// the log of its nearest depth and BINS over the log span (0 when the span is
// a single depth). Everything after reads those, so no fragment takes a log of
// anything but its own depth.
@fragment
fn fs_range(in: TonemapOutput) {
    let base = pixel_index(vec2<i32>(in.clip_position.xy)) * 2u;
    let far_bits = atomicLoad(&ranges[base + 1u]);
    if far_bits == 0u {
        return;
    }
    let ln_near = log(bitcast<f32>(~atomicLoad(&ranges[base])));
    let span = log(bitcast<f32>(far_bits)) - ln_near;
    // A pixel whose fragments all share one depth gets a vanishing scale, not
    // zero: a zero word means "no fragment here" to `fs_prefix`.
    var scale = 1.0e-30;
    if span > 1.0e-6 {
        scale = f32(BINS) / span;
    }
    atomicStore(&ranges[base], bitcast<u32>(ln_near));
    atomicStore(&ranges[base + 1u], bitcast<u32>(scale));
}

struct BinPos {
    bin: u32,
    frac: f32,
}

/// Which slice of its pixel's span a depth falls in, and how far through it.
fn bin_of(depth: f32, pixel: u32) -> BinPos {
    let ln_near = bitcast<f32>(atomicLoad(&ranges[pixel * 2u]));
    let scale = bitcast<f32>(atomicLoad(&ranges[pixel * 2u + 1u]));
    let u = clamp((log(depth) - ln_near) * scale, 0.0, f32(BINS) - 1.0e-3);
    let bin = u32(u);
    return BinPos(bin, u - f32(bin));
}

// Pass A: absorbance into slices. No colour output: the host binds a one-byte
// target nothing is written to, because a pass needs one.
@fragment
fn fs_abs(in: SplatOutput) {
    let a = in.abs_weight * exp(-KERNEL_SHARPNESS * dot(in.offset, in.offset));
    let pixel = pixel_index(vec2<i32>(in.clip_position.xy));
    let pos = bin_of(in.depth, pixel);
    atomicAdd(&slices[pixel * BINS + pos.bin], u32(a * FIXED + 0.5));
}

// After A, `fs_prefix` turns each pixel's slices into running totals, as floats
// (the words change meaning here: from fixed-point masses to f32 cumulative
// absorbance), so that pass B and the tonemap read one or two words instead of
// walking the slices: a fragment used to pay half the slices in loads.
@fragment
fn fs_prefix(in: TonemapOutput) {
    let pixel = pixel_index(vec2<i32>(in.clip_position.xy));
    if atomicLoad(&ranges[pixel * 2u + 1u]) == 0u {
        return;
    }
    var total = 0.0;
    for (var i = 0u; i < BINS; i += 1u) {
        total += f32(atomicLoad(&slices[pixel * BINS + i])) / FIXED;
        atomicStore(&slices[pixel * BINS + i], bitcast<u32>(total));
    }
}

/// The transmittance of what is in front of a fragment at `depth`, whose own
/// deposit `own` is in its slice and does not count. Reads the running totals
/// `fs_prefix` left.
fn bins_transmittance(px: vec2<i32>, depth: f32, own: f32) -> f32 {
    let pixel = pixel_index(px);
    let pos = bin_of(depth, pixel);
    var before = 0.0;
    if pos.bin > 0u {
        before = bitcast<f32>(atomicLoad(&slices[pixel * BINS + pos.bin - 1u]));
    }
    let inside = max(bitcast<f32>(atomicLoad(&slices[pixel * BINS + pos.bin])) - before - own, 0.0);
    return exp(-(before + pos.frac * inside));
}

// === The exact reference ===
//
// `fs_dump` replaces pass A and pass B with a list: every fragment the volume
// passes would have drawn, as one record in a storage buffer, so the CPU can sort
// each pixel's records by depth and composite them front to back with no
// estimation at all (`volume_ref.rs`). Same vertex stage, same kernel, same
// world-space absorbance and emission weights as the real thing; the record also
// carries the transmittance the GPU path *would* have given that fragment, so the
// two can be compared fragment by fragment. Group 2, because group 1 is the
// slices.
//
// 8 words a record: x | y << 16, depth, absorbance, emission weight, r, g, b, and
// the GPU path's T.
@group(2) @binding(0) var<storage, read_write> dump: array<u32>;
@group(2) @binding(1) var<storage, read_write> dump_count: atomic<u32>;

@fragment
fn fs_dump(in: SplatOutput) {
    let falloff = exp(-KERNEL_SHARPNESS * dot(in.offset, in.offset));
    let px = vec2<i32>(in.clip_position.xy);
    let t = bins_transmittance(px, in.depth, in.abs_weight * falloff);
    let idx = atomicAdd(&dump_count, 1u);
    let base = idx * 8u;
    if base + 8u <= arrayLength(&dump) {
        dump[base] = u32(px.x) | (u32(px.y) << 16u);
        dump[base + 1u] = bitcast<u32>(in.depth);
        dump[base + 2u] = bitcast<u32>(in.abs_weight * falloff);
        dump[base + 3u] = bitcast<u32>(emission_weight(in, falloff));
        dump[base + 4u] = bitcast<u32>(in.color.r);
        dump[base + 5u] = bitcast<u32>(in.color.g);
        dump[base + 6u] = bitcast<u32>(in.color.b);
        dump[base + 7u] = bitcast<u32>(t);
    }
}

/// A fragment's emission weight: what splat gives it, plus the source function
/// times its absorbance. A medium that absorbs also glows (Kirchhoff), and what
/// a *thick* region glows at is then the ratio of the two, the same wherever the
/// camera stands and whatever the opacity. Splat's weight alone makes that ratio
/// `weight / absorbance`, which falls as opacity rises: the 1/opacity dimming of
/// every earlier slice, and the reason a thick pixel was dim. The extra term
/// is proportional to the absorbance, so it vanishes with it and opacity 0 is
/// still splat.
fn emission_weight(in: SplatOutput, falloff: f32) -> f32 {
    return (in.weight + volume.source * in.abs_weight) * falloff;
}

/// The ordinary splat fragment, its weight multiplied by the transmittance of
/// everything in front of the point (its own deposit excluded).
@fragment
fn fs_splat_volume(in: SplatOutput) -> @location(0) vec4<f32> {
    let falloff = exp(-KERNEL_SHARPNESS * dot(in.offset, in.offset));
    let t = bins_transmittance(vec2<i32>(in.clip_position.xy), in.depth, in.abs_weight * falloff);
    let w = emission_weight(in, falloff) * t;
    return vec4<f32>(in.color * w, w);
}

/// sRGB transfer function, both directions.
///
/// Needed here because the dither has to be **one step of the output code**,
/// and the output is sRGB-encoded by the hardware on the way to 8 bits. A
/// constant offset in linear light is a wildly varying offset in code space —
/// near black one 8-bit step spans about 0.0003 of linear and near white about
/// 0.006, a factor of twenty — so dithering in linear would be far too strong
/// in the shadows and far too weak in the highlights, which is where the
/// banding actually is.
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

/// Premultiply a straight colour the way the desktop will un-premultiply it.
///
/// An X11 compositor blends a 32-bit window's *code values*: what it shows is
/// `encoded + desktop * (1 - a)`. Multiplying in linear light and letting the
/// sRGB target encode the product gives `encode(c * a)`, which is far darker
/// than `encode(c) * a` at low alpha — the dusty edges would go to soot. So
/// multiply after encoding, and decode again for the hardware to undo.
fn premultiply_encoded(c: vec3<f32>, a: f32) -> vec3<f32> {
    return vec3<f32>(
        srgb_decode(srgb_encode(c.r) * a),
        srgb_decode(srgb_encode(c.g) * a),
        srgb_decode(srgb_encode(c.b) * a),
    );
}

/// A cheap integer hash, for per-pixel noise that is the same every render.
///
/// Deterministic on purpose: two renders of the same scene should differ by
/// their sampling, not by their dither, and a re-render that shifts the grain
/// would make A/B comparison harder for no gain.
fn hash_u32(x: u32) -> u32 {
    var h = x;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

fn rand01(p: vec2<i32>, salt: u32) -> f32 {
    let h = hash_u32(u32(p.x) * 73856093u ^ u32(p.y) * 19349663u ^ salt * 83492791u);
    return f32(h) * (1.0 / 4294967296.0);
}

/// Triangular (TPDF) dither of one output code, applied in sRGB space.
///
/// # Why a converged render is what bands
///
/// Sampling noise is itself a dither. At low `--spp` the render's own grain
/// spans several quantization steps and randomizes every rounding decision for
/// free, so banding cannot appear. As `spp` climbs, that self-dither shrinks
/// below one step — around 30,000 spp at 8 bits — and the structure it was
/// hiding emerges. **The banding appears exactly when the render finally gets
/// good**, which is the regime the whole scaling effort exists to reach.
/// Measured on a 100,000-spp render: 1,134 distinct colours in a 300x200 patch
/// of smooth material at 8 bits, against 56,747 at 16.
///
/// # Triangular, not uniform
///
/// Uniform dither decorrelates the error from the signal but leaves the noise
/// *power* signal-dependent, which reads as the grain breathing as a gradient
/// passes through it. TPDF — the difference of two uniforms — removes that too,
/// and is the standard result from audio for the same reason.
///
/// # It makes the error bigger, and the picture better
///
/// Rounding has an RMS error of 0.289 of a step; TPDF raises that to 0.5. No
/// information is recovered and the 8-bit crossover does not move. What changes
/// is that the error stops being *structured*: a contour that the eye finds
/// instantly becomes grain that it does not. Past the crossover an 8-bit file
/// then degrades gracefully instead of into rings.
fn dither_srgb(c: vec3<f32>, p: vec2<i32>, amount: f32) -> vec3<f32> {
    if amount <= 0.0 {
        return c;
    }
    let encoded = vec3<f32>(srgb_encode(c.r), srgb_encode(c.g), srgb_encode(c.b));
    // Independent noise per channel: shared noise would move all three the same
    // way, which is a luminance wobble rather than the chroma-neutral grain we
    // want.
    let n = vec3<f32>(
        rand01(p, 0u) - rand01(p, 1u),
        rand01(p, 2u) - rand01(p, 3u),
        rand01(p, 4u) - rand01(p, 5u),
    );
    let d = clamp(encoded + n * (amount / 255.0), vec3<f32>(0.0), vec3<f32>(1.0));
    // Back to linear, because the hardware will encode again on the way out.
    // The round trip is exact to float precision, so `amount = 0` above is not
    // merely close to the old path — this function is skipped entirely.
    return vec3<f32>(srgb_decode(d.r), srgb_decode(d.g), srgb_decode(d.b));
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
    let raw = clamp(brightness, 0.0, 1.0);
    let gi = params.gamma_inv;
    let th = params.gamma_threshold;

    // Gamma reaches the picture by two routes, and `vibrancy` picks between
    // them. Through *coverage* the hue is untouched, so a bright core keeps
    // its colour and stays saturated. Through each *channel* of the emitted
    // colour, the dim channels of a bright pixel get lifted further than the
    // strong one — which is a highlight rolling off toward white, the way film
    // does it. flam3 calls the blend vibrancy and so does this.
    let coverage = gamma_curve(raw, gi, th);
    let vibrant = mean_color * coverage;
    let desaturated = vec3<f32>(
        gamma_curve(mean_color.r * raw, gi, th),
        gamma_curve(mean_color.g * raw, gi, th),
        gamma_curve(mean_color.b * raw, gi, th),
    );
    // Premultiplied by coverage, which is what makes the two branches below
    // one compositing model rather than two.
    let emitted = mix(desaturated, vibrant, params.vibrancy);

    // Dither the colour and never the alpha. Coverage is not a colour: it is
    // read as a mask, and noise in a mask is a ragged edge rather than a finer
    // gradient.
    let px = vec2<i32>(in.clip_position.xy);
    if transparent {
        // Straight (non-premultiplied) alpha, which is what PNG stores:
        // colour is the palette hue, coverage is the log density, so the
        // dusty edges stay dusty instead of turning into a cutout.
        let straight = emitted / max(coverage, 1e-6);
        if params.transparent > 1.5 {
            // A window the desktop shows through: same coverage, premultiplied.
            return vec4<f32>(premultiply_encoded(straight, coverage), coverage);
        }
        return vec4<f32>(dither_srgb(straight, px, params.dither), coverage);
    }
    let composited = params.background.rgb * (1.0 - coverage) + emitted;
    return vec4<f32>(dither_srgb(composited, px, params.dither), 1.0);
}

// Volume mode's tonemap: the same pass, with the background dimmed by the
// absorbance field. Keep in step with `fs_tonemap` above.
@group(0) @binding(2) var<storage, read> tonemap_slices: array<u32>;

@fragment
fn fs_tonemap_volume(in: TonemapOutput) -> @location(0) vec4<f32> {
    let transparent = params.transparent > 0.5;
    let acc = textureLoad(accum, vec2<i32>(in.clip_position.xy), 0);
    let density = acc.a;
    // The share of the background that survives the material in front of it:
    // mean of exp(-b0) over this output pixel's block of accumulation texels
    // (a transmittance averages as itself, not as its logarithm).
    let n = i32(params.volume_n);
    var t_total = 0.0;
    for (var dy = 0; dy < n; dy += 1) {
        for (var dx = 0; dx < n; dx += 1) {
            let q = vec2<i32>(in.clip_position.xy) * n + vec2<i32>(dx, dy);
            // The accumulation is `n` times as wide as the texture being toned.
            let pixel = u32(q.y) * (textureDimensions(accum).x * u32(n)) + u32(q.x);
            t_total += exp(-bitcast<f32>(tonemap_slices[pixel * BINS + BINS - 1u]));
        }
    }
    t_total = t_total / f32(n * n);
    if density <= 0.0 {
        // Nothing emitted here, but something may still absorb: a pixel under
        // material that lit nothing is not background. (This used to return the
        // background whatever `t_total` was.)
        if transparent {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0 - t_total);
        }
        return vec4<f32>(params.background.rgb * t_total, 1.0);
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
    let raw = clamp(brightness, 0.0, 1.0);
    let gi = params.gamma_inv;
    let th = params.gamma_threshold;

    // Gamma reaches the picture by two routes, and `vibrancy` picks between
    // them. Through *coverage* the hue is untouched, so a bright core keeps
    // its colour and stays saturated. Through each *channel* of the emitted
    // colour, the dim channels of a bright pixel get lifted further than the
    // strong one — which is a highlight rolling off toward white, the way film
    // does it. flam3 calls the blend vibrancy and so does this.
    let coverage = gamma_curve(raw, gi, th);
    let vibrant = mean_color * coverage;
    let desaturated = vec3<f32>(
        gamma_curve(mean_color.r * raw, gi, th),
        gamma_curve(mean_color.g * raw, gi, th),
        gamma_curve(mean_color.b * raw, gi, th),
    );
    // Premultiplied by coverage, which is what makes the two branches below
    // one compositing model rather than two.
    let emitted = mix(desaturated, vibrant, params.vibrancy);
    // The pixel is the material emits plus what the background lets
    // through. `density` here is the *visible* radiance, `sum e*T`, so a pixel
    // under a lot of material is as dim as the transport says it is, in its own
    // colour; and the background gets through twice over -- past the material's
    // coverage, as in splat, and past its absorbance, `t_total`. Where nothing
    // absorbs `t_total` is 1 and this is the splat tonemap exactly.
    //
    // There is deliberately no fill. Dimming only the background used to leave
    // a thick, dim pixel *darker than the ground it hid*, which was patched by
    // filling it with a constant share of its own colour. That patch was
    // compensating for a transport that did not know how much light a thick
    // pixel carries; with the exact one (see `volume_ref.rs`) the dim pixel is
    // correct, and a constant is just a constant.
    let alpha = 1.0 - (1.0 - coverage) * t_total;

    // Dither the colour and never the alpha. Coverage is not a colour: it is
    // read as a mask, and noise in a mask is a ragged edge rather than a finer
    // gradient.
    let px = vec2<i32>(in.clip_position.xy);
    if transparent {
        // Straight (non-premultiplied) alpha, which is what PNG stores:
        // colour is the palette hue, coverage is the log density, so the
        // dusty edges stay dusty instead of turning into a cutout.
        let straight = emitted / max(alpha, 1e-6);
        if params.transparent > 1.5 {
            return vec4<f32>(premultiply_encoded(straight, alpha), alpha);
        }
        return vec4<f32>(dither_srgb(straight, px, params.dither), alpha);
    }
    let composited = params.background.rgb * (1.0 - alpha) + emitted;
    return vec4<f32>(dither_srgb(composited, px, params.dither), 1.0);
}

/// Gamma with a linear toe: `x^(1/gamma)`, blended toward a straight line
/// below `threshold`.
///
/// This is flam3's `flam3_calc_alpha` applied to *coverage* rather than to raw
/// density, which is the same curve in the place this renderer keeps the
/// equivalent quantity. One consequence worth knowing before copying a number
/// out of Apophysis: the threshold is in **post-log coverage**, so its whole
/// scale is 0-1 and the useful range is 0.05-0.5, not flam3's density-unit
/// range. Gamma and vibrancy mean the same thing in both.
///
/// The toe is the whole reason `gamma_threshold` exists. Gamma > 1 lifts dim
/// values hard — that is what it is for — but the dimmest thing in a flame
/// image is not detail, it is the single-sample speckle scattered across the
/// background. Lifting *that* turns a black field into a grey veil with grain
/// in it. Below the threshold the curve becomes the straight line through the
/// origin that meets it at the threshold, so near-zero coverage stays
/// near-zero and everything above is graded normally.
fn gamma_curve(x: f32, gamma_inv: f32, threshold: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if threshold > 0.0 && x < threshold {
        let frac = x / threshold;
        // Slope of the line from the origin to the curve at `threshold`.
        let slope = pow(threshold, gamma_inv) / threshold;
        return (1.0 - frac) * x * slope + frac * pow(x, gamma_inv);
    }
    return pow(x, gamma_inv);
}
