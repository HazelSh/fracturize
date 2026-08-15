#![allow(dead_code)]

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
#[allow(unused_imports)]
use half::f16;

/// Hash grid capacity (4M entries)
pub const HASH_GRID_SIZE: usize = 4 * 1024 * 1024;

/// Maximum render voxels after compaction (2M)
pub const MAX_RENDER_VOXELS: usize = 2 * 1024 * 1024;

/// Empty cell marker (0 so clear_buffer works)
pub const EMPTY_KEY: u32 = 0;

/// Hash cell: 16 bytes
/// Stored in GPU hash table, supports atomic operations
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HashCell {
    /// Encoded key: screen_x:11 | screen_y:11 | depth_slice:10
    /// EMPTY_KEY (0xFFFFFFFF) means empty cell
    pub key: u32,
    /// Accumulated density (fixed-point u16.16 for atomic add precision)
    pub density: u32,
    /// colormap_idx:8 | reserved:8 | weight:16 (for weighted color averaging)
    pub color_weight: u32,
    /// Nearest depth in cell (for occlusion) - uses atomicMin
    pub min_depth: u32,
}

impl HashCell {
    pub fn empty() -> Self {
        Self {
            key: EMPTY_KEY,
            density: 0,
            color_weight: 0,
            min_depth: 0, // cleared state
        }
    }
}

/// Output voxel for rendering: 16 bytes
/// Produced by compaction pass, consumed by voxel renderer
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RenderVoxel {
    /// NDC position (x, y, z) and depth (w) as f16
    pub ndc_xyzw: [f16; 4],
    /// RGB color from colormap lookup
    pub color_rgb: [f16; 3],
    /// Density for alpha/size modulation
    pub density: f16,
}

impl RenderVoxel {
    pub fn zeroed() -> Self {
        Self {
            ndc_xyzw: [f16::ZERO; 4],
            color_rgb: [f16::ZERO; 3],
            density: f16::ZERO,
        }
    }
}

/// Hash grid parameters uniform (must be 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HashGridParams {
    /// Previous frame inverse view-projection matrix
    pub prev_inv_view_proj: [[f32; 4]; 4],
    /// Current frame view-projection matrix
    pub curr_view_proj: [[f32; 4]; 4],
    /// Grid resolution in pixels (matches screen size)
    pub res_x: u32,
    pub res_y: u32,
    /// Depth slices (e.g., 1024)
    pub depth_slices: u32,
    /// Decay factor for reprojection (0.98)
    pub decay: f32,
    /// Hash table size
    pub hash_size: u32,
    /// Near/far planes for depth encoding
    pub near_plane: f32,
    pub far_plane: f32,
    /// Frame number for debugging
    pub frame: u32,
    /// Depth culling enabled (1 = yes, 0 = no)
    pub depth_cull_enabled: u32,
    /// Padding for 16-byte alignment (WGSL requires struct size multiple of largest alignment)
    pub _pad: [u32; 3],
}

impl HashGridParams {
    pub fn new(
        prev_inv_view_proj: Mat4,
        curr_view_proj: Mat4,
        res_x: u32,
        res_y: u32,
        near_plane: f32,
        far_plane: f32,
        decay: f32,
        frame: u32,
        depth_cull_enabled: bool,
    ) -> Self {
        Self {
            prev_inv_view_proj: prev_inv_view_proj.to_cols_array_2d(),
            curr_view_proj: curr_view_proj.to_cols_array_2d(),
            res_x,
            res_y,
            depth_slices: 2048,  // 11 bits (stolen 1 from y)
            decay,
            hash_size: HASH_GRID_SIZE as u32,
            near_plane,
            far_plane,
            frame,
            depth_cull_enabled: if depth_cull_enabled { 1 } else { 0 },
            _pad: [0; 3],
        }
    }
}

/// Voxel count buffer for indirect draw
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoxelCounter {
    /// Number of voxels after compaction
    pub count: u32,
    /// Padding for alignment
    pub _pad: [u32; 3],
}

/// GPU transform representation (256 bytes)
///
/// Carries colour twice, because the two colour models want different things
/// from it: `color_value` is a *position* in the 256-entry colormap (the
/// scalar the walker's EMA converges toward), and `color_rgb` is the
/// transform's actual colour, for `ColorMode::Mix` where the walker carries a
/// 3-vector and colours genuinely blend. Only one is read per run.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuTransform {
    pub matrix: [[f32; 4]; 4],       // 64 bytes - pre-affine (before variations)
    pub post_affine: [[f32; 4]; 4],  // 64 bytes - post-affine (after variations)
    pub color_value: f32,            // 4 bytes - colormap index (0.0-1.0)
    pub weight: f32,                 // 4 bytes
    pub cumulative_weight: f32,      // 4 bytes
    pub color_speed: f32,            // 4 bytes - per-transform blending speed
    /// Variation blend weights (slot order matches chaos.wgsl / scene::VARIATION_NAMES)
    pub var_weights: [f32; crate::scene::NUM_VARIATIONS], // 80 bytes
    /// This transform's own colour, linear RGB (`ColorMode::Mix` only)
    pub color_rgb: [f32; 3],         // 12 bytes
    /// Where this transform's symmetry group starts in the group-element
    /// buffer, and how many elements it has. `group_count == 0` is a transform
    /// with no symmetry, and the walk skips the whole thing — so an ordinary
    /// scene pays one comparison per iteration and nothing else.
    ///
    /// A slice rather than a group *id* because the alternative is a second
    /// indirection buffer to resolve the id, and motifs sharing a group already
    /// share a slice: three motifs under `I` point at the same 60 elements.
    pub group_start: u32,            // 4 bytes
    pub group_count: u32,            // 4 bytes
    /// 1 when the colormap index should be offset by which group element was
    /// drawn (`OrbitColor::Orbit`), 0 for the shared default. A float because
    /// it is multiplied rather than branched on in the shader.
    pub orbit_color: f32,            // 4 bytes
    /// WGSL gives `color_rgb: vec3<f32>` an alignment of 16, so it lands at
    /// offset 224 and the three scalars after it run 236..248; the struct then
    /// rounds up to 256. Rust's `repr(C)` would stop at 248 and every transform
    /// after the first would read the wrong memory.
    pub _pad: [f32; 2],              // 8 bytes
    // Total: 256 bytes (was 240, and 176 before the post-affine slot)
}

impl GpuTransform {
    pub fn new(
        spec: &crate::scene::TransformSpec,
        cumulative_weight: f32,
        effective_weight: f32,
        color_rgb: glam::Vec3,
        // Where this transform's group sits in the element buffer, from
        // `points::compute::GroupTable`. `(0, 0)` for no symmetry.
        group: (u32, u32),
    ) -> Self {
        Self {
            matrix: spec.matrix.to_cols_array_2d(),
            post_affine: spec.post_affine.to_cols_array_2d(),
            color_value: spec.color_value,
            weight: effective_weight,
            cumulative_weight,
            color_speed: spec.color_speed,
            var_weights: spec.variations,
            color_rgb: color_rgb.to_array(),
            group_start: group.0,
            group_count: group.1,
            orbit_color: match spec.symmetry.as_ref().map(|s| s.color()) {
                Some(crate::symmetry::OrbitColor::Orbit) => 1.0,
                _ => 0.0,
            },
            _pad: [0.0; 2],
        }
    }
}

/// Camera uniforms (144 bytes, must be 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CameraUniforms {
    pub mvp: [[f32; 4]; 4], // 64 bytes
    pub screen_height: f32, // 4 bytes
    pub point_size: f32,    // 4 bytes
    pub aspect_ratio: f32,  // 4 bytes (width / height)
    pub min_point_pixels: f32, // 4 bytes - minimum screen size in pixels
    pub haze_near: f32,     // 4 bytes - distance where the haze starts
    pub haze_far: f32,      // 4 bytes - distance where the haze is thickest
    /// Fraction of a point's contribution that survives at `haze_far`
    /// (1 = no haze). Not a brightness multiplier — see `src/haze.rs`.
    pub haze_transmittance: f32, // 4 bytes
    pub haze_saturation: f32, // 4 bytes - saturation surviving at haze_far (1 = no change)
    pub color_contrast: f32, // 4 bytes - cyclic contrast stretch of colormap index (1=off)
    /// The background, linear RGB. Haze fades toward it, so the shaders need
    /// to know what it is; it sits in what used to be tail padding.
    pub background: [f32; 3], // 12 bytes
    /// 1 when this pass is writing an image with an alpha channel (a
    /// `--transparent` render or screenshot), 0 for the window. The points
    /// renderer is opaque and depth-tested, so it has no other way to know
    /// that the alpha it writes will be kept — see `render.wgsl`.
    pub transparent: f32,   // 4 bytes
    /// 1 in `ColorMode::Mix`, where a point's colour is packed into the top
    /// 24 bits of `Point.color_idx` as 8/8/8 rather than being a colormap
    /// index. A uniform branch rather than a second pipeline: it is perfectly
    /// coherent across every invocation, so it costs nothing measurable, and
    /// the alternative is duplicating four pipelines to change two lines.
    pub color_rgb_mode: f32, // 4 bytes
    /// The infinite-zoom edge guard: the fixed point, and the ramp that takes
    /// material to nothing before the band's outer edge can be seen. See
    /// `renorm::DEFAULT_EDGE_GUARD` — the short version is that it is a
    /// function of `|pos − centre| / |eye − centre|`, which the zoom wrap
    /// leaves exactly unchanged, so it fades the edge out over the progress of
    /// the zoom instead of at the wrap.
    ///
    /// Three scalars rather than a `vec3` because the run lands at offset 120,
    /// which is not 16-aligned — same reason `background` above is spelled
    /// this way, and the WGSL side must match field for field.
    pub guard_center: [f32; 3], // 12 bytes
    /// `ln(ρ_start · d)` for this frame's eye distance `d`
    pub guard_ln_near: f32, // 4 bytes
    /// `1 / ln(ρ_end / ρ_start)`. **Zero disables the guard**, which is what
    /// an ordinary scene (and a zoom scene with `edge_guard = 0`) gets.
    pub guard_inv_ln_width: f32, // 4 bytes
    /// Near-field cap on a point's screen size, in **render-target** pixels.
    ///
    /// Points brushing past the camera would otherwise smear into
    /// screen-eating squares (or gaussian washes) in volume-filling scenes;
    /// this holds them to motes. It used to be the literal `12.0` in both
    /// shaders, which was fine while the render target was always the output.
    /// Under N x supersampling it is not: a cap of 12 target pixels is a cap of
    /// 12/N *output* pixels, so close motes silently shrank as quality went up.
    /// See [`Self::with_supersample`], and note the *floor* deliberately does
    /// not scale — a finer floor is precisely what supersampling buys.
    ///
    /// It also uses up what was tail padding, so the struct is the same size.
    pub max_point_pixels: f32, // 4 bytes
    /// This tile's window onto the camera: `[scale_x, scale_y, offset_x,
    /// offset_y]`, applied in NDC after projection. `[1, 1, 0, 0]` — the
    /// default, and what every non-tiled render uses — is the identity.
    ///
    /// # Why this is a uniform and not baked into `mvp`
    ///
    /// It would be simpler to premultiply the projection and leave the shader
    /// alone, and it would be wrong. A splat's *position* is projected through
    /// `mvp`, but its *size* is added afterwards, in NDC, as a quad offset
    /// (see `splat.wgsl`). Baking the window into the matrix would scale where
    /// each point lands without scaling how big it is, so every tile of a
    /// tiled render would come out with points of the wrong size — and since
    /// the scale factor differs with the tile's shape, tiles would visibly
    /// differ in texture. That is exactly the failure `RENDER-SCALE-PLAN.md`
    /// §4 warns about, arriving through the other door.
    ///
    /// So the shader applies it to both, and `screen_height` stays the **full
    /// output** height rather than the tile's — which is the same rule, and
    /// the reason `Sampling` is built from the output size and passed down
    /// unchanged.
    pub subfrustum: [f32; 4], // 16 bytes
}

/// The near-field cap at 1x, in output pixels. Both point renderers agree on
/// it, and it is quoted in AGENTS.md as "12px in both modes".
pub const MAX_POINT_PIXELS: f32 = 12.0;

impl CameraUniforms {
    pub fn new(
        mvp: Mat4,
        screen_height: f32,
        point_size: f32,
        aspect_ratio: f32,
        min_point_pixels: f32,
        haze_near: f32,
        haze_far: f32,
        haze_transmittance: f32,
        haze_saturation: f32,
        color_contrast: f32,
        background: [f32; 3],
        transparent: bool,
        color_rgb_mode: bool,
    ) -> Self {
        Self {
            mvp: mvp.to_cols_array_2d(),
            screen_height,
            point_size,
            aspect_ratio,
            min_point_pixels,
            haze_near,
            haze_far,
            haze_transmittance,
            haze_saturation,
            color_contrast,
            background,
            transparent: if transparent { 1.0 } else { 0.0 },
            color_rgb_mode: if color_rgb_mode { 1.0 } else { 0.0 },
            // No guard unless a zoom asks for one: see `with_zoom_guard`
            guard_center: [0.0; 3],
            guard_ln_near: 0.0,
            guard_inv_ln_width: 0.0,
            // 1x until someone says otherwise; see `with_supersample`.
            max_point_pixels: MAX_POINT_PIXELS,
            // The whole frame. Identity, so every render that is not tiled is
            // bit-for-bit what it was before tiling existed.
            subfrustum: [1.0, 1.0, 0.0, 0.0],
        }
    }

    /// Render only the part of the frame this tile covers.
    ///
    /// `(scale_x, scale_y, offset_x, offset_y)` straight from
    /// [`crate::tile::Tile::subfrustum`] — that is the only thing that should
    /// ever produce these, because getting them from anywhere else means two
    /// places compute where a tile aims and they can disagree by a pixel,
    /// which is a seam.
    pub fn with_subfrustum(mut self, window: (f32, f32, f32, f32)) -> Self {
        self.subfrustum = [window.0, window.1, window.2, window.3];
        self
    }

    /// Scale the caps that are expressed in render-target pixels for an
    /// `n` x supersampled render.
    ///
    /// Call it wherever `screen_height` was given the accumulation height
    /// rather than the output height — the two describe the same target and
    /// disagreeing about it is the bug this exists to prevent.
    pub fn with_supersample(mut self, n: u32) -> Self {
        self.max_point_pixels = MAX_POINT_PIXELS * n.max(1) as f32;
        self
    }

    /// Arm the infinite-zoom edge guard for a camera whose eye is at `eye`.
    /// A `None` zoom, or a zoom with the guard turned off, leaves it disabled.
    ///
    /// **Every frame, from that frame's eye.** The ramp is expressed in
    /// multiples of the current eye-to-fixed-point distance, and that is the
    /// whole trick: it makes the guard invariant across a zoom wrap and gives
    /// it a constant fade rate per octave of zoom. Passing a stale eye — the
    /// scene's authored distance, say — turns it back into the static
    /// world-space fade this replaced, artifact and all.
    pub fn with_zoom_guard(
        mut self,
        zoom: Option<&crate::renorm::Renorm>,
        eye: glam::Vec3,
    ) -> Self {
        if let Some(z) = zoom {
            let (ln_near, inv_ln_width) = z.guard_params(eye);
            self.guard_center = z.fixed_point.to_array();
            self.guard_ln_near = ln_near;
            self.guard_inv_ln_width = inv_ln_width;
        }
        self
    }
}

/// Create a uniform buffer for camera data
pub fn create_camera_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("camera_buffer"),
        size: std::mem::size_of::<CameraUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

// =============================================================================
// Simple Point Renderer Types
// =============================================================================

/// A single point in the chaos game buffer (16 bytes)
/// Used by the simple point renderer
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Point {
    /// World-space position
    pub position: [f32; 3],
    /// Colormap index (0-255 in lower 8 bits)
    pub color_idx: u32,
}

/// Compute parameters for the simple chaos game (192 bytes)
///
/// The `zoom_*` block is the infinite-zoom renormalization (see `renorm.rs`);
/// it is all zeroes and `zoom_enabled = 0` for ordinary scenes. `mat3x3` is
/// three padded `vec4` columns under std140 — writing `[[f32; 3]; 3]` here
/// would silently misalign the second column.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PointComputeParams {
    pub num_transforms: u32,
    pub num_walkers: u32,
    pub iterations_per_walker: u32,
    pub write_offset: u32,
    pub buffer_capacity: u32,
    pub zoom_enabled: u32,
    /// Octaves of scale spread dealt out below the target radius
    pub zoom_levels: f32,
    /// ln(1/scale) of the renormalizing map — one zoom period in log-radius
    pub zoom_log_scale: f32,
    /// Per-octave point share, `scale^octave_falloff`: the ratio between the
    /// number of points given to one octave and the next one in. 1 = flat.
    pub zoom_octave_q: f32,
    /// 1 when the map is a similarity, so `A^k` has a closed form
    pub zoom_similar: u32,
    /// Contraction ratio (the closed form needs it on its own, not as a log)
    pub zoom_scale: f32,
    /// Periods to carry the whole buffer through, read only by the `rewrap`
    /// entry point. Always 0 for the chaos pass: that one places new points
    /// and must never move ones already placed.
    pub zoom_rewrap: f32,
    /// Threads per row of the `rewrap` dispatch, which has to be 2D: a 100M
    /// point buffer is 390,625 workgroups of 256, and a dispatch dimension
    /// stops at 65,535. Read only by that entry point.
    pub rewrap_stride: u32,
    /// std140 pads the scalar run out to the following vec4's alignment.
    /// **Thirteen scalars, so this is three words, not one** — get it wrong and
    /// the `vec4`s below land somewhere else and the zoom silently renders
    /// from garbage rather than failing to compile.
    ///
    /// Four rather than two since the outer-edge taper left: the edge is
    /// guarded at render time now (`CameraUniforms::guard_*`), because the
    /// deal cannot depend on the camera — this buffer is filled over ~800
    /// frames and would mix that many camera positions into one image.
    pub _pad: [u32; 3],
    /// xyz = the map's fixed point, w = target radius
    pub zoom_fixed: [f32; 4],
    /// xyz = the rotation axis of `A`, w = its angle
    pub zoom_axis_angle: [f32; 4],
    /// Linear part about the fixed point, and its inverse (padded columns)
    pub zoom_a: [[f32; 4]; 3],
    pub zoom_a_inv: [[f32; 4]; 3],
}

impl PointComputeParams {
    /// Pack a resolved renormalization, or the disabled state for `None`
    pub fn with_zoom(mut self, zoom: Option<&crate::renorm::Renorm>) -> Self {
        let pad = |m: glam::Mat3| m.to_cols_array_2d().map(|c| [c[0], c[1], c[2], 0.0]);
        match zoom {
            Some(z) => {
                self.zoom_enabled = 1;
                self.zoom_levels = z.periods;
                self.zoom_log_scale = z.log_scale;
                self.zoom_octave_q = z.octave_q;
                self.zoom_similar = z.similar as u32;
                self.zoom_scale = z.scale;
                self.zoom_fixed = z.fixed_point.extend(z.radius).to_array();
                self.zoom_axis_angle = z
                    .twist
                    .axis()
                    .unwrap_or(glam::Vec3::Y)
                    .extend(z.twist.magnitude())
                    .to_array();
                self.zoom_a = pad(z.a);
                self.zoom_a_inv = pad(z.a_inv);
            }
            None => self.zoom_enabled = 0,
        }
        self
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_gpu_transform_size() {
        // 64 (matrix) + 64 (post_affine) + 16 (four scalars) + 80 (var_weights)
        // = 224, then `color_rgb: vec3<f32>` at 224 (12 bytes, 16-aligned), then
        // the group slice and orbit flag at 236/240/244. WGSL rounds the struct
        // up to the largest member alignment, so both sides land on 256. The
        // explicit `_pad` is what keeps Rust's repr(C) from stopping at 248.
        assert_eq!(std::mem::size_of::<GpuTransform>(), 256, "GpuTransform must be 256 bytes to match WGSL struct");
        // The group slice has to sit exactly where WGSL puts it, immediately
        // after the vec3's 12 bytes and before its alignment tail. Getting this
        // wrong is not a compile error in either language — it is a symmetry
        // drawn from whatever happened to be at the offset.
        assert_eq!(std::mem::offset_of!(GpuTransform, color_rgb), 224);
        assert_eq!(std::mem::offset_of!(GpuTransform, group_start), 236);
        assert_eq!(std::mem::offset_of!(GpuTransform, group_count), 240);
        assert_eq!(std::mem::offset_of!(GpuTransform, orbit_color), 244);
    }

    #[test]
    fn test_camera_uniforms_size() {
        // 144 (see below) + the tile window's vec4.
        //
        // Declared in five shaders (points/render, points/splat, trace,
        // gizmo, density/voxel_render). They do *not* all have to grow
        // together, and that is the point of appending rather than inserting:
        // a uniform buffer may be larger than the struct a shader declares, so
        // only `points/splat` — the one that renders tiles — knows about
        // `subfrustum`, and the other four are untouched. Inserting a field
        // anywhere else would silently shift every field after it for all five.
        assert_eq!(
            std::mem::size_of::<CameraUniforms>(),
            160,
            "CameraUniforms must be 160 bytes to match the WGSL struct",
        );
        // The guard block starts where `_pad` used to, so nothing before it
        // moved and a shader that doesn't read it is unaffected.
        assert_eq!(std::mem::offset_of!(CameraUniforms, guard_center), 120);
        // vec4 wants 16-byte alignment, and 144 is a multiple of 16 — so the
        // window lands where WGSL expects it with no padding in between.
        assert_eq!(std::mem::offset_of!(CameraUniforms, subfrustum), 144);
        // The default is the whole frame, so an untiled render is unchanged.
        let c = CameraUniforms::new(
            Mat4::IDENTITY, 100.0, 0.01, 1.5, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, [0.0; 3], false,
            false,
        );
        assert_eq!(c.subfrustum, [1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn test_point_compute_params_size() {
        // std140: 12 scalars + 4 pad (64) + two vec4 (32) + two mat3x3 as
        // padded columns (48 each)
        assert_eq!(
            std::mem::size_of::<PointComputeParams>(),
            192,
            "PointComputeParams must be 192 bytes to match ComputeParams in chaos.wgsl"
        );
        // The vec4s have to start where std140 puts them. A wrong pad is not a
        // compile error in either language — it is a zoom rendered from
        // whatever happened to be at the offset instead.
        assert_eq!(
            std::mem::offset_of!(PointComputeParams, zoom_fixed),
            64,
            "the scalar run must pad out to a 16-byte boundary"
        );
        // `rewrap_stride` took the first of the pad words rather than being
        // appended, so every render made before it existed is unaffected. Size
        // alone would not catch a reordering that stayed 192 bytes.
        assert_eq!(
            std::mem::offset_of!(PointComputeParams, rewrap_stride),
            48,
            "rewrap_stride must sit where the first pad word was"
        );
    }

    #[test]
    fn test_hash_grid_params_size() {
        println!("HashGridParams size: {}", std::mem::size_of::<HashGridParams>());
        assert_eq!(std::mem::size_of::<HashGridParams>(), 176, "HashGridParams must be 176 bytes to match WGSL struct");
    }
}
