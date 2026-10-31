// Glass: shared by the compute passes (glass.wgsl) and the resolve
// (glass_resolve.wgsl), which are compiled as `common + pass` so neither has
// to restate this. See GLASS-RENDERER-PLAN.md and src/gpu/points/glass.rs.

// Mirrors `GlassUniforms` in src/gpu/points/glass.rs, field for field.
struct Glass {
    view_proj: mat4x4<f32>,
    // The pane, in framebuffer pixels: x, y, width, height.
    rect: vec4<f32>,
    // The framebuffer's width: the per-pixel buffers are indexed by it.
    fb_width: u32,
    // K, the length of each pixel's list of nearest fragments.
    k: u32,
    point_count: u32,
    // 1 when point colour is packed RGB rather than a colormap index.
    color_rgb_mode: u32,
    // Pixels per world unit at view depth 1: pane height / (2 tan(fov/2)).
    ppw_unit: f32,
    // A dot's radius, in world units.
    radius_world: f32,
    // A dot's absorbance: sigma * L^2 / N, a world area times an optical depth.
    tau_point: f32,
    // Nearer than this, a point is behind the eye.
    near: f32,
    // The largest disc drawn, in pixels; a dot grazing the camera is cropped.
    max_radius_px: f32,
    color_contrast: f32,
    // 0 opaque over the background, 1 straight alpha, 2 premultiplied.
    alpha_mode: f32,
    _pad0: f32,
    background: vec4<f32>,
    // Infinite-zoom edge guard: centre xyz, ln_near; then 1/ln_width.
    guard: vec4<f32>,
    guard_inv_ln_width: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
}

// Per pixel, eight words, one 32-byte row:
//   0 total tau (fixed point)        1 far depth (f32 bits)
//   2 spare
//   3 the K-list's own tau, summed (fixed point, kept as entries come and go)
//   4..7 the weighted colour sums r, g, b, w (fixed point)
// Fixed point so they can be added with atomics.
const STRIDE: u32 = 8u;
const TAU: u32 = 0u;
const ZMAX: u32 = 1u;
const LISTED: u32 = 3u;
const SUM_R: u32 = 4u;
const TAU_SCALE: f32 = 1048576.0;  // 2^20: tau resolves to 1e-6
const W_SCALE: f32 = 4194304.0;    // 2^22: weights sum to under ~1
// Where the total stops counting, to keep its word from overflowing. Not
// lower: the tail's share of the total ranks the fragments behind the K-th,
// and capping it near opacity (tried at 32) shifts thick pixels' colour.
const TAU_SATURATED: f32 = 1024.0;
// A fragment with this much in front of it is hidden: everything behind that
// depth together carries under exp(-9) = 0.012% of the pixel's colour.
const TAU_HIDDEN: f32 = 9.0;
