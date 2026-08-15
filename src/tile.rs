//! Splitting one output into tiles that each fit on the GPU.
//!
//! The ceiling on a render is not VRAM, it is
//! `max_storage_buffer_binding_size` — 2.15 GB on the reference GTX 1080,
//! against 8 GB of VRAM. The accumulation histogram is a single storage buffer
//! at 32 bytes a texel, so that limit caps *any* accumulating render at 67.1 M
//! texels: 8K at 1x, 4K at 2x, and about 512x512 at 16x. An A2 poster at
//! 600 ppi is 139 Mpx and does not fit at any supersampling at all.
//!
//! Tiling is what removes that ceiling. The output is cut into rectangles, each
//! with its own histogram sized under the limit, and each rendered as a window
//! onto the *same* camera. See `RENDER-SCALE-PLAN.md` §4.
//!
//! # This module is pure geometry
//!
//! No GPU, no wgpu types, no I/O. That is deliberate: every way tiling can go
//! visibly wrong — a seam, a doubled halo, a tile that silently exceeds the
//! limit, a frustum that doesn't line up with its neighbour — is a property of
//! the arithmetic here, and can be tested without a device. A seam across a
//! poster is the worst failure mode at this size, and it should be caught by
//! `cargo test` rather than by a six-hour render.
//!
//! # The two things that produce seams
//!
//! 1. **Halos.** Density estimation reads up to `MAX_RADIUS_PX` output pixels
//!    away, and the downsample filter reads `filter_radius`. A tile that
//!    renders only its own texels gets wrong values along every edge, which
//!    across a poster is a visible grid. So tiles overlap, render wider than
//!    they keep, and discard the margin. See [`Halo`].
//! 2. **One camera, sub-frustums.** Every tile is a window onto the same
//!    camera, *not* its own camera — unlike the contact sheets, which really do
//!    move the camera per tile. [`Tile::subfrustum`] is the only thing that
//!    should ever be used to aim a tile.

// Slice 1 of `RENDER-SCALE-PLAN.md` §9 is the geometry on its own, tested
// against the plan's own tables; slice 2 is the renderer that consumes it. The
// arithmetic is worth landing and reviewing before anything is wired to a
// device, so for one slice this module is legitimately unused.
#![allow(dead_code)]

/// Bytes per histogram texel — four channels of 64-bit fixed point.
///
/// Re-exported from the accumulator rather than redefined, so a change to the
/// histogram's layout cannot leave the planner quietly computing tile sizes for
/// a histogram that no longer exists.
pub use crate::gpu::points::accumulate::BYTES_PER_TEXEL;

/// The largest texture dimension wgpu will accept, and so the largest a tile's
/// *supersampled* side may be. The histogram is a buffer, but the density
/// pyramid and downsample intermediates are textures, and they are what this
/// binds on.
pub const MAX_TEXTURE_DIM: u32 = 32768;

/// How far outside its own rectangle a tile must render, in **output pixels**.
///
/// Both contributions scale with the supersample factor in texels — density
/// estimation reaches `MAX_RADIUS_PX * N` texels and the filter reaches
/// `filter_radius * N` — so in *output* pixels the halo is the same at every
/// supersampling. That is the whole reason it is expressed in output pixels
/// here: it makes the cost independent of the one setting that otherwise
/// squares everything.
///
/// # Sized from the settings, not from the maximum
///
/// The halo is a fixed cost in output pixels while the tile it surrounds
/// shrinks as N² for a given memory budget, so its overhead is negligible
/// everywhere except one corner — high supersampling on a small binding limit —
/// where it becomes the dominant cost. At a 32 MB budget and 16x, a
/// maximum-width halo leaves a 46-pixel tile inside a 62-pixel rendered square:
/// **81% of the work is halo**.
///
/// Sizing it from what is actually switched on takes that to 14%, for a change
/// with no downside — a halo wide enough for a pass that is not running is pure
/// waste. Density estimation is the whole of the expensive term, and it is
/// off by default.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Halo {
    /// Output pixels of margin on every side.
    pub px: u32,
}

impl Halo {
    /// The halo these settings actually need.
    pub fn for_settings(
        density_estimation: crate::gpu::points::density::DensityEstimation,
        filter_radius: f32,
    ) -> Self {
        let de = if density_estimation.is_off() {
            0.0
        } else {
            crate::gpu::points::density::MAX_RADIUS_PX
        };
        // Ceil, not round: a halo half a pixel short is a seam, and one pixel
        // long is one pixel of waste.
        Self { px: (de + filter_radius.max(0.0)).ceil() as u32 }
    }
}

/// One tile: the output rectangle it owns, and the larger rectangle it renders.
///
/// The distinction between the two is the entire point. `x/y/width/height` is
/// what this tile contributes to the file and must not overlap any other tile's
/// — write those and the image is exactly covered, once. The rendered rectangle
/// is bigger by the halo and *does* overlap its neighbours; those pixels are
/// computed, used to make the kept ones correct, and thrown away.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tile {
    /// Left edge of the kept rectangle, in output pixels.
    pub x: u32,
    /// Top edge of the kept rectangle, in output pixels.
    pub y: u32,
    /// Width of the kept rectangle, in output pixels.
    pub width: u32,
    /// Height of the kept rectangle, in output pixels.
    pub height: u32,
    /// Left edge of the rendered rectangle. Clamped to the image, so a tile on
    /// the left edge has no left halo — the picture's own border is not a seam.
    pub render_x: u32,
    /// Top edge of the rendered rectangle.
    pub render_y: u32,
    /// Width of the rendered rectangle, halo included.
    pub render_width: u32,
    /// Height of the rendered rectangle, halo included.
    pub render_height: u32,
}

impl Tile {
    /// Where the kept rectangle sits inside the rendered one, in output pixels.
    /// This is the crop to apply after rendering and before writing.
    pub fn crop_offset(&self) -> (u32, u32) {
        (self.x - self.render_x, self.y - self.render_y)
    }

    /// This tile's window onto the shared camera, as a scale and offset in
    /// normalised device coordinates.
    ///
    /// Returned as `(scale_x, scale_y, offset_x, offset_y)`, to be applied to
    /// the projection matrix: a tile covering the whole image gives
    /// `(1, 1, 0, 0)`. Derived from the **rendered** rectangle, because that is
    /// what actually gets drawn — aiming at the kept rectangle would put the
    /// halo outside the frustum and make it useless.
    ///
    /// Y is flipped relative to pixel space: NDC runs bottom-up, image rows run
    /// top-down, and getting this backwards flips every tile into the wrong row
    /// — which looks like a scrambled image rather than a subtle seam, so it is
    /// at least loud.
    pub fn subfrustum(&self, output_width: u32, output_height: u32) -> (f32, f32, f32, f32) {
        let (ow, oh) = (output_width as f32, output_height as f32);
        let (rw, rh) = (self.render_width as f32, self.render_height as f32);
        let scale_x = ow / rw;
        let scale_y = oh / rh;
        // Centre of the rendered rect in NDC, negated and scaled: this is the
        // translation that brings that centre to the origin.
        let cx = (self.render_x as f32 + rw / 2.0) / ow * 2.0 - 1.0;
        let cy = 1.0 - (self.render_y as f32 + rh / 2.0) / oh * 2.0;
        (scale_x, scale_y, -cx * scale_x, -cy * scale_y)
    }

    /// Histogram texels this tile needs at supersample factor `n`.
    pub fn texels(&self, n: u32) -> u64 {
        let n = n.max(1) as u64;
        self.render_width as u64 * n * (self.render_height as u64 * n)
    }
}

/// A complete tiling: which rectangles to render, and in what groups.
#[derive(Clone, PartialEq, Debug)]
pub struct TilePlan {
    /// Output size this plans for.
    pub output_width: u32,
    pub output_height: u32,
    /// Supersample factor. Every per-texel figure here is already multiplied by
    /// N², which is why it is carried rather than passed around separately.
    pub supersample: u32,
    pub halo: Halo,
    pub tiles: Vec<Tile>,
    pub cols: u32,
    pub rows: u32,
    /// Tiles that can be resident on the device at once, so the number of tiles
    /// a single pass can hold.
    pub resident: u32,
}

/// Why a tiling could not be produced at all.
#[derive(Clone, PartialEq, Debug)]
pub enum TileError {
    /// The output has a zero dimension.
    Empty,
    /// Even a single output pixel plus its halo exceeds a limit. Only reachable
    /// with an absurd supersample factor or a tiny budget, but it is a real
    /// arithmetic possibility and returning it beats looping forever.
    HopelessTile { needed: u64, limit: u64 },
}

impl std::fmt::Display for TileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TileError::Empty => write!(f, "an output with a zero dimension has nothing to tile"),
            TileError::HopelessTile { needed, limit } => write!(
                f,
                "even a single pixel needs {} of histogram against a {} limit — lower the \
                 supersampling, or turn off density estimation to shrink the halo",
                crate::render_job::human_bytes(*needed),
                crate::render_job::human_bytes(*limit),
            ),
        }
    }
}

/// What the machine will allow.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Budget {
    /// `max_storage_buffer_binding_size`. Caps one tile's histogram.
    pub binding_limit: u64,
    /// Total histogram bytes that may be resident at once. Caps how many tiles
    /// a pass can hold, and so how many passes the render takes — which is the
    /// thing that actually costs wall clock.
    pub resident_limit: u64,
}

impl Budget {
    /// The reference desktop: a 2.15 GB binding limit, and about three
    /// full-size histograms resident inside 8 GB of VRAM.
    pub fn gtx1080() -> Self {
        Self { binding_limit: 2_147_483_648, resident_limit: 6_442_450_944 }
    }
}

impl TilePlan {
    /// Plan a tiling, or say why there isn't one.
    ///
    /// Prefers **full-width strips** wherever they fit, and that is not just
    /// tidiness: a strip that spans the image needs no left or right halo,
    /// because those edges are the picture's own. Its overhead is linear in the
    /// strip's height instead of quadratic in a tile's side. Strips are only
    /// available while `width * N <= 32768`, since the density pyramid and
    /// downsample intermediates are textures — at A2 that covers 1x and 2x,
    /// which is what §3 argues for at poster size anyway.
    pub fn new(
        output_width: u32,
        output_height: u32,
        supersample: u32,
        halo: Halo,
        budget: Budget,
    ) -> Result<Self, TileError> {
        if output_width == 0 || output_height == 0 {
            return Err(TileError::Empty);
        }
        let n = supersample.max(1);

        // The cheapest tiling that satisfies both constraints. Grow the grid one
        // step at a time, always splitting whichever axis currently gives the
        // less square tile, so tiles stay near-square and the halo — which is
        // paid per side — stays a small fraction of each.
        let (mut cols, mut rows) = (1u32, 1u32);
        loop {
            if Self::fits(output_width, output_height, cols, rows, n, halo, budget.binding_limit) {
                break;
            }
            // A single output pixel that still doesn't fit is hopeless, and
            // without this the loop would never terminate.
            if cols >= output_width && rows >= output_height {
                let t = Self::probe_tile(output_width, output_height, cols, rows, halo);
                return Err(TileError::HopelessTile {
                    needed: t.texels(n) * BYTES_PER_TEXEL,
                    limit: budget.binding_limit,
                });
            }
            let tile_w = output_width.div_ceil(cols);
            let tile_h = output_height.div_ceil(rows);
            // Prefer splitting height: a wider, shorter tile is closer to a
            // strip, and strips are the cheap shape.
            if (tile_h >= tile_w && rows < output_height) || cols >= output_width {
                rows += 1;
            } else {
                cols += 1;
            }
        }

        let mut tiles = Vec::with_capacity((cols * rows) as usize);
        for row in 0..rows {
            for col in 0..cols {
                tiles.push(Self::tile_at(output_width, output_height, cols, rows, col, row, halo));
            }
        }

        // Residency decides passes, and passes decide the wall clock. At least
        // one, or a render with a large tile would report zero passes and do
        // nothing.
        let per_tile =
            tiles.iter().map(|t| t.texels(n) * BYTES_PER_TEXEL).max().unwrap_or(1).max(1);
        let resident = (budget.resident_limit / per_tile).clamp(1, tiles.len() as u64) as u32;

        Ok(Self {
            output_width,
            output_height,
            supersample: n,
            halo,
            tiles,
            cols,
            rows,
            resident,
        })
    }

    /// The tile at a grid position, halo clamped to the image.
    fn tile_at(w: u32, h: u32, cols: u32, rows: u32, col: u32, row: u32, halo: Halo) -> Tile {
        // Distribute the remainder rather than making the last tile a stub: a
        // 4097-wide image over two columns is 2049 + 2048, not 4096 + 1.
        let span = |total: u32, parts: u32, i: u32| {
            let base = total / parts;
            let extra = total % parts;
            let start = base * i + i.min(extra);
            let len = base + u32::from(i < extra);
            (start, len)
        };
        let (x, width) = span(w, cols, col);
        let (y, height) = span(h, rows, row);
        // A tile against the image edge gets no halo there — that boundary is
        // the picture's own, and padding it would render pixels that don't
        // exist.
        let render_x = x.saturating_sub(halo.px);
        let render_y = y.saturating_sub(halo.px);
        let render_right = (x + width + halo.px).min(w);
        let render_bottom = (y + height + halo.px).min(h);
        Tile {
            x,
            y,
            width,
            height,
            render_x,
            render_y,
            render_width: render_right - render_x,
            render_height: render_bottom - render_y,
        }
    }

    /// The largest tile a grid produces — always a middle one, which carries a
    /// halo on all four sides.
    fn probe_tile(w: u32, h: u32, cols: u32, rows: u32, halo: Halo) -> Tile {
        let col = if cols > 2 { 1 } else { 0 };
        let row = if rows > 2 { 1 } else { 0 };
        Self::tile_at(w, h, cols, rows, col, row, halo)
    }

    fn fits(w: u32, h: u32, cols: u32, rows: u32, n: u32, halo: Halo, limit: u64) -> bool {
        let t = Self::probe_tile(w, h, cols, rows, halo);
        let (tw, th) = (t.render_width as u64 * n as u64, t.render_height as u64 * n as u64);
        tw <= MAX_TEXTURE_DIM as u64
            && th <= MAX_TEXTURE_DIM as u64
            && tw * th * BYTES_PER_TEXEL <= limit
    }

    /// Passes this plan takes: how many times the chaos game has to be re-run.
    ///
    /// This — not the tile count — is what a big render costs. Each pass runs
    /// the full chaos game again, because re-generating samples is *cheaper
    /// than moving them*: the point spool for one run is `spp × pixels × 16`
    /// bytes against a histogram of `pixels × N² × 32`, so spooling is bigger
    /// whenever `spp > 2N²`. At A2 and 1,000 spp that is 2.2 TB of spool
    /// against 17.8 GB of histogram.
    pub fn passes(&self) -> u32 {
        (self.tiles.len() as u32).div_ceil(self.resident.max(1))
    }

    /// Tiles grouped into passes, in the order they should be rendered.
    pub fn pass_groups(&self) -> impl Iterator<Item = &[Tile]> {
        self.tiles.chunks(self.resident.max(1) as usize)
    }

    /// Whether this is a single tile covering everything — the case that must
    /// stay byte-identical to a render from before tiling existed.
    pub fn is_single(&self) -> bool {
        self.tiles.len() == 1
    }

    /// Total histogram bytes across every tile, halos included.
    pub fn total_histogram_bytes(&self) -> u64 {
        self.tiles.iter().map(|t| t.texels(self.supersample) * BYTES_PER_TEXEL).sum()
    }

    /// Extra work the halo costs, as a fraction of the ideal.
    ///
    /// 0.0 means free. This is the number that decides whether a small machine
    /// can tile at all: it is negligible everywhere except high supersampling
    /// on a small budget, where it can pass 0.8 if the halo is sized from the
    /// maximum rather than from the settings.
    pub fn halo_overhead(&self) -> f64 {
        let n = self.supersample as u64;
        let ideal = self.output_width as u64 * n * (self.output_height as u64 * n);
        if ideal == 0 {
            return 0.0;
        }
        let actual: u64 = self.tiles.iter().map(|t| t.texels(self.supersample)).sum();
        actual as f64 / ideal as f64 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::points::density::DensityEstimation;

    fn halo8() -> Halo {
        Halo { px: 8 }
    }

    /// A render that fits today must keep planning as exactly one tile — not
    /// one tile plus a degenerate second, and not a tile with a halo it doesn't
    /// need. Tiling has to be invisible until it is necessary.
    #[test]
    fn a_render_that_fits_is_still_one_tile() {
        let plan = TilePlan::new(1920, 1080, 2, halo8(), Budget::gtx1080()).unwrap();
        assert!(plan.is_single());
        assert_eq!(plan.passes(), 1);
        assert_eq!(plan.halo_overhead(), 0.0);
        let t = plan.tiles[0];
        assert_eq!((t.x, t.y, t.width, t.height), (0, 0, 1920, 1080));
        // No halo anywhere, because every edge is the image's own.
        assert_eq!((t.render_width, t.render_height), (1920, 1080));
        assert_eq!(t.crop_offset(), (0, 0));
        assert_eq!(t.subfrustum(1920, 1080), (1.0, 1.0, 0.0, 0.0));
    }

    /// The tiles must cover the output exactly once: no gap (a stripe of
    /// unwritten pixels) and no overlap (a stripe written twice, at double
    /// brightness if it is ever summed).
    #[test]
    fn tiles_cover_the_output_exactly_once() {
        for (w, h, limit) in
            [(4096u32, 4096u32, 64u64 << 20), (4097, 2161, 16 << 20), (1000, 999, 4 << 20)]
        {
            let plan = TilePlan::new(w, h, 2, halo8(), Budget { binding_limit: limit, resident_limit: limit })
                .unwrap();
            let mut seen = vec![0u8; (w as usize) * (h as usize)];
            for t in &plan.tiles {
                for yy in t.y..t.y + t.height {
                    for xx in t.x..t.x + t.width {
                        seen[(yy as usize) * (w as usize) + xx as usize] += 1;
                    }
                }
            }
            assert!(
                seen.iter().all(|&c| c == 1),
                "{w}x{h} at {limit}: {} pixels not covered exactly once",
                seen.iter().filter(|&&c| c != 1).count(),
            );
        }
    }

    /// Every tile's histogram has to fit the binding limit — that is the entire
    /// reason the module exists, and an off-by-one in the halo would break it
    /// silently, several seconds into a render, inside the driver.
    #[test]
    fn no_tile_exceeds_the_binding_limit() {
        for n in [1u32, 2, 4, 16] {
            for limit in [32u64 << 20, 512 << 20, 2_147_483_648] {
                let budget = Budget { binding_limit: limit, resident_limit: limit * 3 };
                let plan = TilePlan::new(3840, 2160, n, halo8(), budget).unwrap();
                for t in &plan.tiles {
                    let bytes = t.texels(n) * BYTES_PER_TEXEL;
                    assert!(bytes <= limit, "N={n} limit={limit}: tile needs {bytes}");
                    assert!(t.render_width * n <= MAX_TEXTURE_DIM);
                    assert!(t.render_height * n <= MAX_TEXTURE_DIM);
                }
            }
        }
    }

    /// Interior tiles carry a halo on all four sides; edge tiles must not carry
    /// one against the image border. Padding there would render pixels that do
    /// not exist, and the picture's own edge is not a seam.
    #[test]
    fn the_halo_stops_at_the_image_edge() {
        let budget = Budget { binding_limit: 8 << 20, resident_limit: 64 << 20 };
        let plan = TilePlan::new(2000, 2000, 1, halo8(), budget).unwrap();
        assert!(plan.cols >= 3 && plan.rows >= 3, "need interior tiles: {}x{}", plan.cols, plan.rows);
        for t in &plan.tiles {
            let left = t.x - t.render_x;
            let top = t.y - t.render_y;
            let right = (t.render_x + t.render_width) - (t.x + t.width);
            let bottom = (t.render_y + t.render_height) - (t.y + t.height);
            assert_eq!(left, if t.x == 0 { 0 } else { 8 });
            assert_eq!(top, if t.y == 0 { 0 } else { 8 });
            assert_eq!(right, if t.x + t.width == 2000 { 0 } else { 8 });
            assert_eq!(bottom, if t.y + t.height == 2000 { 0 } else { 8 });
            assert_eq!(t.crop_offset(), (left, top));
        }
    }

    /// Sizing the halo from what is switched on rather than from the maximum is
    /// the difference between a small machine being able to supersample and not.
    /// The plan's §10 quotes 81% overhead against 14%; both are this arithmetic.
    #[test]
    fn a_halo_sized_from_the_settings_is_what_lets_a_small_machine_tile() {
        let off = Halo::for_settings(DensityEstimation { amount: 0.0 }, 0.5);
        let on = Halo::for_settings(DensityEstimation { amount: 1.0 }, 0.5);
        assert_eq!(off.px, 1);
        assert_eq!(on.px, 7);

        // The corner the plan warns about: 16x on a 32 MB budget.
        let budget = Budget { binding_limit: 32 << 20, resident_limit: 32 << 20 };
        let wide = TilePlan::new(1920, 1080, 16, on, budget).unwrap();
        let lean = TilePlan::new(1920, 1080, 16, off, budget).unwrap();
        assert!(
            wide.halo_overhead() > lean.halo_overhead() * 3.0,
            "a maximum halo should cost multiples of a settings-sized one: {:.3} vs {:.3}",
            wide.halo_overhead(),
            lean.halo_overhead(),
        );
    }

    /// Passes, not tiles, are what a big render costs — so residency has to
    /// actually group them, and a plan must never claim zero passes.
    #[test]
    fn residency_decides_the_pass_count() {
        // Sixteen tiles, four resident: four passes.
        let budget = Budget { binding_limit: 8 << 20, resident_limit: 32 << 20 };
        let plan = TilePlan::new(4000, 4000, 1, halo8(), budget).unwrap();
        assert_eq!(plan.passes(), (plan.tiles.len() as u32).div_ceil(plan.resident));
        assert!(plan.passes() >= 1);
        let grouped: usize = plan.pass_groups().map(|g| g.len()).sum();
        assert_eq!(grouped, plan.tiles.len(), "every tile belongs to exactly one pass");
        assert!(plan.pass_groups().all(|g| g.len() <= plan.resident as usize));
    }

    /// Each tile is a window onto one shared camera. Adjacent sub-frustums must
    /// meet exactly: a gap or an overlap here is a seam that no amount of halo
    /// can fix, because the halo corrects filtering, not aim.
    #[test]
    fn subfrustums_tile_the_same_camera() {
        let budget = Budget { binding_limit: 4 << 20, resident_limit: 16 << 20 };
        let plan = TilePlan::new(1024, 1024, 1, Halo { px: 0 }, budget).unwrap();
        assert!(plan.cols > 1 && plan.rows > 1);
        for t in &plan.tiles {
            let (sx, sy, ox, oy) = t.subfrustum(1024, 1024);
            // The tile's own corners, mapped through its frustum, must land on
            // the NDC corners — that is what "this tile is exactly this window"
            // means.
            let left = (t.render_x as f32 / 1024.0) * 2.0 - 1.0;
            let right = ((t.render_x + t.render_width) as f32 / 1024.0) * 2.0 - 1.0;
            assert!((left * sx + ox + 1.0).abs() < 1e-4, "left edge: {}", left * sx + ox);
            assert!((right * sx + ox - 1.0).abs() < 1e-4, "right edge: {}", right * sx + ox);
            let top = 1.0 - (t.render_y as f32 / 1024.0) * 2.0;
            let bottom = 1.0 - ((t.render_y + t.render_height) as f32 / 1024.0) * 2.0;
            assert!((top * sy + oy - 1.0).abs() < 1e-4, "top edge: {}", top * sy + oy);
            assert!((bottom * sy + oy + 1.0).abs() < 1e-4, "bottom edge: {}", bottom * sy + oy);
        }
    }

    /// The poster this whole plan exists for: A2 at 600 ppi is 139 Mpx, which
    /// does not fit under the binding limit at any supersampling at all.
    #[test]
    fn an_a2_poster_at_600ppi_becomes_a_workable_plan() {
        // A2 is 420 x 594 mm; at 600 ppi that is 9921 x 14031.
        let plan = TilePlan::new(9921, 14031, 2, halo8(), Budget::gtx1080()).unwrap();
        assert!(!plan.is_single(), "139 Mpx at 2x cannot be one tile");
        assert!(plan.halo_overhead() < 0.05, "halo should stay cheap: {:.3}", plan.halo_overhead());
        // Every tile legal, and the whole thing covered.
        let covered: u64 = plan.tiles.iter().map(|t| t.width as u64 * t.height as u64).sum();
        assert_eq!(covered, 9921 * 14031);
        for t in &plan.tiles {
            assert!(t.texels(2) * BYTES_PER_TEXEL <= Budget::gtx1080().binding_limit);
        }
    }

    /// `RENDER-SCALE-PLAN.md` §4 costs this work in *passes* — how many times
    /// the chaos game has to be re-run — from a table computed by hand. This
    /// pins the planner to that table, so if the arithmetic ever drifts it
    /// fails here rather than in a six-hour render's wall clock.
    #[test]
    fn the_pass_table_from_the_plan_still_holds() {
        // label, width, height, N, expected passes
        let rows: &[(&str, u32, u32, u32, u32)] = &[
            ("A2@600 1x", 9921, 14031, 1, 1),
            ("A2@600 2x", 9921, 14031, 2, 3),
            ("A2@600 4x", 9921, 14031, 4, 12),
            ("8K 2x", 7680, 4320, 2, 1),
            ("8K 4x", 7680, 4320, 4, 3),
            ("4K 4x", 3840, 2160, 4, 1),
        ];
        for &(label, w, h, n, want) in rows {
            let p = TilePlan::new(w, h, n, Halo { px: 8 }, Budget::gtx1080()).unwrap();
            assert_eq!(p.passes(), want, "{label}: {} tiles, {} resident", p.tiles.len(), p.resident);
            // And the halo stays a rounding error at every one of them, which
            // is the claim that makes tiling worth doing at all.
            assert!(p.halo_overhead() < 0.02, "{label}: halo {:.3}", p.halo_overhead());
        }
    }

    /// A budget so small nothing can fit must say so rather than loop forever
    /// splitting a grid it can never make small enough.
    #[test]
    fn an_impossible_budget_is_reported_not_hung() {
        let budget = Budget { binding_limit: 16, resident_limit: 16 };
        let err = TilePlan::new(256, 256, 16, halo8(), budget).unwrap_err();
        assert!(matches!(err, TileError::HopelessTile { .. }), "{err:?}");
        assert!(err.to_string().contains("supersampling"), "{err}");
        assert_eq!(TilePlan::new(0, 100, 1, halo8(), Budget::gtx1080()).unwrap_err(), TileError::Empty);
    }
}
