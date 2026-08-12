//! Headless renderer
//!
//! Renders a scene at arbitrary resolution without opening a window (no
//! surface, no event loop, no focus stealing). Runs the chaos game until the
//! point buffer is full, then renders one or more views of the resulting
//! point cloud and saves a PNG. Because the point cloud is view-independent,
//! grid modes (orbit / move contact sheets) cost one fill plus one cheap
//! render pass per tile. Mutation sheets (`render_mutations`) re-fill per
//! tile since each variant has different transforms.
//!
//! Prints a per-tile camera/mutation mapping and a timing breakdown to
//! stdout so automated callers can budget render effort.

use std::path::Path;
use std::time::Instant;

use glam::{Mat4, Vec3};
use rand::SeedableRng;

use crate::camera::{CameraOverride, OrbitCamera};
use crate::glyphs;
use crate::gpu::buffers::CameraUniforms;
use crate::gpu::{PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};
use crate::path::CameraPath;
use crate::scene::Scene;
use crate::render_job::{JobControl, Outcome, CANCELLED};
use crate::view::View;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Multi-view layouts. Grid tiles are laid out row-major.
#[derive(Clone, Copy, Debug)]
pub enum GridMode {
    /// One image from the base camera
    Single,
    /// cols*rows views evenly spaced around a full horizontal orbit,
    /// starting from the base yaw
    Orbit { cols: u32, rows: u32 },
    /// Camera nudged in the view plane: columns sweep left->right, rows sweep
    /// up->down, all still looking at the focus. `step` is the nudge per grid
    /// unit as a fraction of orbit distance. The center tile (odd dimensions)
    /// is the unmoved base view.
    Move { cols: u32, rows: u32, step: f32 },
}

impl GridMode {
    fn tile_count(&self) -> (u32, u32) {
        match *self {
            GridMode::Single => (1, 1),
            GridMode::Orbit { cols, rows } | GridMode::Move { cols, rows, .. } => (cols, rows),
        }
    }
}

/// One view to render: camera matrices plus a human/LLM-readable description
struct TileView {
    view_proj: Mat4,
    label: String,
}

pub struct OfflineParams<'a> {
    pub scene: Scene,
    pub view: Option<View>,
    /// Per-tile output size
    pub width: u32,
    pub height: u32,
    pub out_path: &'a Path,
    /// Extra chaos-game frames after the point buffer is full
    pub accumulate: u32,
    pub haze_enabled: bool,
    pub grid: GridMode,
    /// Use the additive log-density splat renderer instead of plain points
    pub splat: bool,
    /// Splat exposure multiplier (ignored for the point renderer)
    pub exposure: f32,
    /// Write an alpha channel: the background clears to transparent and the
    /// fractal carries its own coverage, so the render can be composited.
    pub transparent: bool,
    /// Progress reporting and pause/cancel, when the render was started from
    /// the app's render-job dialog. `None` for the CLI paths, which are
    /// blocking by design and have a terminal to print to.
    pub control: Option<JobControl>,
    /// Draw per-tile parameter labels into contact sheets. Single-tile
    /// renders are never labelled — there is nothing to tell apart.
    pub labels: bool,
    /// Camera flags (`--yaw` etc.), applied over the scene and any view
    pub camera: CameraOverride,
}

/// Evenly spaced values in [-1, 1] (a single sample sits at 0)
fn grid_axis(n: u32) -> Vec<f32> {
    if n <= 1 {
        return vec![0.0];
    }
    (0..n).map(|i| i as f32 / (n - 1) as f32 * 2.0 - 1.0).collect()
}

fn build_tiles(base: &OrbitCamera, grid: GridMode, aspect: f32) -> Vec<TileView> {
    match grid {
        GridMode::Single => vec![TileView {
            view_proj: base.view_proj(aspect),
            label: format!(
                "yaw {:.1}° pitch {:.1}° dist {:.2}",
                base.chart().yaw.degrees(),
                base.chart().pitch.degrees(),
                base.distance
            ),
        }],
        GridMode::Orbit { cols, rows } => {
            let n = cols * rows;
            (0..n)
                .map(|k| {
                    let mut cam = *base;
                    // About world up, so the sweep is the same orbit at every
                    // roll — and doesn't degenerate looking straight down.
                    cam.orientation = base.orientation.then_world(crate::rot::Turn::about(
                        glam::Vec3::Y,
                        k as f32 * std::f32::consts::TAU / n as f32,
                    ));
                    let yaw = cam.chart().yaw;
                    TileView {
                        view_proj: cam.view_proj(aspect),
                        // Radians in parens: paste directly into [camera] yaw
                        label: format!("yaw {:.1}° ({:.4})", yaw.degrees(), yaw.radians()),
                    }
                })
                .collect()
        }
        GridMode::Move { cols, rows, step } => {
            let proj = Mat4::perspective_rh(
                crate::camera::FOV_Y_RADIANS,
                aspect,
                crate::camera::Z_NEAR,
                crate::camera::Z_FAR,
            );
            let right = base.right();
            let up = base.up();
            let mut tiles = Vec::new();
            // Row 0 is "up", matching how the tiles read on the sheet
            for dy in grid_axis(rows) {
                for dx in grid_axis(cols) {
                    let nudge = (right * dx - up * dy) * step * base.distance;
                    let eye = base.eye() + nudge;
                    let view = Mat4::look_at_rh(eye, base.focus, Vec3::Y);
                    let h = |v: f32, neg: &str, pos: &str| -> String {
                        if v == 0.0 {
                            "center".to_string()
                        } else if v < 0.0 {
                            format!("{} {:.2}", neg, -v * step)
                        } else {
                            format!("{} {:.2}", pos, v * step)
                        }
                    };
                    // Equivalent orbit params, so a tile can be adopted
                    // directly into a [camera] block or view file
                    let v = eye - base.focus;
                    let d = v.length().max(1e-4);
                    tiles.push(TileView {
                        view_proj: proj * view,
                        label: format!(
                            "{} / {} = yaw {:.4} pitch {:.4} distance {:.3}",
                            h(dx, "left", "right"),
                            h(dy, "up", "down"),
                            v.x.atan2(v.z),
                            (v.y / d).clamp(-1.0, 1.0).asin(),
                            d,
                        ),
                    });
                }
            }
            tiles
        }
    }
}

/// Create a headless wgpu device with storage limits raised to adapter max
fn create_device() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|e| format!("No suitable GPU adapter: {}", e))?;
    log::info!("Offline render using adapter: {:?}", adapter.get_info().name);

    let adapter_limits = adapter.limits();
    let required_limits = wgpu::Limits {
        max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
        max_buffer_size: adapter_limits.max_buffer_size,
        ..wgpu::Limits::default()
    };
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("fracturize_offline_device"),
        required_features: wgpu::Features::empty(),
        required_limits,
        memory_hints: wgpu::MemoryHints::Performance,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: Default::default(),
    }))
    .map_err(|e| format!("Failed to create device: {}", e))
}

/// Run the chaos game until the point buffer is full and then some. The long
/// phase of any job, and the first of the three places a job can be paused or
/// cancelled.
///
/// Returns the compute pipeline, the valid point count, and whether it got all
/// the way there. Cancelling **stops the fill but does not fail it**: the
/// chaos game is an anytime algorithm, so a buffer abandoned at 60% is the same
/// picture as one abandoned at 100%, just noisier, and the downstream save path
/// already works from a smaller-than-target point count — a mid-warmup buffer
/// is exactly that. It used to return `Err(CANCELLED)`, which propagated
/// through `render()`'s `?` and meant stopping at 99% got you nothing.
///
/// Callers decide what a partial fill is worth to them: a still writes it, an
/// animation doesn't (a sparse cloud drawn across every frame is a full-cost
/// job at reduced quality, not a partial result).
fn fill_points(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    accumulate: u32,
    control: Option<&JobControl>,
) -> Result<(PointCompute, u32, Outcome), String> {
    let mut compute = PointCompute::new(
        device,
        &scene.transforms,
        &scene.colors,
        &scene.colormap,
        scene.point_count as u32,
    );
    compute.zoom = scene_zoom(scene)?;
    let total_frames = compute.warmup_frames + accumulate.max(1);
    log::info!(
        "Filling {} point buffer: {} warmup + {} accumulation frames",
        scene.point_count, compute.warmup_frames, accumulate.max(1)
    );
    if let Some(c) = control {
        c.phase("filling points");
        c.log(format!(
            "chaos game: {} points, {} warmup + {} accumulation frames",
            scene.point_count,
            compute.warmup_frames,
            accumulate.max(1)
        ));
    }
    let mut point_count = 0;
    let mut outcome = Outcome::Complete;
    for i in 0..total_frames {
        point_count = compute.advance_frame(queue, 1.0 / 60.0);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offline_compute_encoder"),
        });
        compute.dispatch(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        if i % 16 == 15 {
            // Let the queue drain so we don't buffer unbounded work
            let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            // Checked on the same cadence: the drain is where this loop has
            // already given up its latency, so pausing or cancelling here
            // costs nothing extra and can't leave submitted work orphaned.
            if let Some(c) = control {
                c.progress(i + 1, total_frames);
                if c.should_stop() {
                    c.log(format!(
                        "stopped at frame {} of {} — keeping the {} points accumulated so far",
                        i + 1,
                        total_frames,
                        point_count
                    ));
                    outcome = Outcome::Partial;
                    break;
                }
            }
        }
    }
    // Unconditional, and it has to be: on the cancel path there is submitted
    // work in flight that the readback would otherwise race.
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    if let Some(c) = control {
        c.progress(total_frames, total_frames);
    }
    Ok((compute, point_count, outcome))
}

/// The scene's infinite-zoom renormalization, if it asked for one.
///
/// Offline, an unusable zoom map is fatal rather than a status-bar note: a
/// render is a thing you come back to later, and silently getting the ordinary
/// bounded attractor back is the kind of surprise that costs an hour.
fn scene_zoom(scene: &Scene) -> Result<Option<crate::renorm::Renorm>, String> {
    scene
        .zoom
        .as_ref()
        .map(|spec| {
            crate::renorm::Renorm::build(spec, &scene.transforms, scene.camera_distance)
                .map_err(|e| format!("infinite zoom: {}", e))
        })
        .transpose()
}

/// Haze for an offline render: the falloff pair, plus the band *if* a view
/// pinned one by hand.
///
/// An unpinned band is resolved per frame from the camera's own distance, the
/// same rule `App::haze_range` follows. Freezing it at the scene's authored
/// distance was a real artifact and not a subtle one: a zoom loop's camera
/// ends a period closer in than it started, so against a fixed band the whole
/// image drifted out of the haze as the loop ran and snapped back at the wrap
/// — measured on `wellspiral` as a 12% brightening across the loop undone by
/// an 11% drop in one frame. Offline was the only renderer with the bug,
/// which is why it showed up in a render and never in the window.
#[derive(Clone, Copy)]
struct Haze {
    /// `Some` only when a view pinned the band; otherwise auto-ranged
    pinned: Option<(f32, f32)>,
    transmittance: f32,
    saturation: f32,
}

impl Haze {
    /// `(near, far)` for a camera at `distance`
    fn band(&self, distance: f32) -> (f32, f32) {
        self.pinned.unwrap_or_else(|| crate::haze::auto_band(distance))
    }
}

/// The camera a render would actually use: a `--view`'s framing if there is
/// one, else the scene's, with the camera flags over the top.
///
/// One function so that anything reporting a framing reports the one that
/// would be drawn — `--info` prints through this, which is the only reason it
/// can be trusted about a scene it was handed a view for.
pub fn effective_camera(view: Option<&View>, scene: &Scene, over: CameraOverride) -> OrbitCamera {
    effective_camera_folded(view, scene, over).0
}

/// The same, and how many zoom periods the fold moved it by.
///
/// The count is what carries the *point buffer* to the same place — see
/// `PointCompute::rewrap`. Without it a run of stills stepping along a flight
/// is not a run of frames: the fold lands two neighbouring stills a whole
/// period apart in the world, and each one draws the structure from a fresh
/// deal of octaves, so consecutive frames differ by a full resample of the dot
/// field rather than by the motion between them. Carrying the buffer by the
/// same count is what makes `--path-t 0.0` and `--path-t 0.002` two frames of
/// one flight rather than two independent pictures of one object.
pub fn effective_camera_folded(
    view: Option<&View>,
    scene: &Scene,
    over: CameraOverride,
) -> (OrbitCamera, i32) {
    let mut camera = match view {
        Some(v) => OrbitCamera::from_legacy(
            Vec3::from(v.focus),
            Vec3::from(v.offset),
            v.distance,
            v.rotation,
            v.pitch,
            v.roll,
        ),
        None => scene.camera(),
    };
    // The path, if one was asked for, before the field overrides: `--path-t`
    // says where the flight is and `--distance` and friends then adjust that
    // framing, which is the order anyone typing both would expect. The default
    // is built from the framing so far, so a scene with no authored path still
    // answers — with a point on its full orbit.
    if let Some(t) = over.path_t {
        let default = CameraPath::full_orbit(&camera);
        camera = crate::path::resolve(scene.camera_path.as_ref(), &default).sample(t);
    }
    // Flags last: they are the most specific thing anyone said.
    over.apply(&mut camera);
    // Under infinite zoom the framing is only defined up to a zoom period, so
    // put it in the canonical one before anything derives from it. A framing
    // that came from a view file is already there and this is a no-op.
    let mut folded = 0;
    if let Ok(Some(zoom)) = scene_zoom(scene) {
        folded = zoom.wrap(&mut camera);
    }
    (camera, folded)
}

/// Base camera and render params from a view file, or scene defaults
fn base_setup(
    view: &Option<View>,
    scene: &Scene,
    haze_enabled: bool,
    over: CameraOverride,
) -> (OrbitCamera, f32, Haze, i32) {
    let (point_size, mut haze) = base_render_params(view, scene, haze_enabled);
    // A band pinned in world units cannot survive a zoom wrap: the wrap
    // rescales the camera and the picture together and leaves the band where
    // it was, so the image drifts out of the haze across a period and snaps
    // back at the seam. Under infinite zoom the pin is a bug by construction,
    // and every legacy view carries one — say so and auto-range instead.
    if haze.pinned.is_some() && scene.zoom.is_some() {
        haze.pinned = None;
        println!("haze band auto-ranged (a pinned band is not wrap-invariant under infinite zoom)");
    }
    let (camera, folded) = effective_camera_folded(view.as_ref(), scene, over);
    if !over.is_empty() {
        // So a framing found by flags can be kept without transcription
        println!("{}", CameraOverride::describe(&camera));
    }
    (camera, point_size, haze, folded)
}

/// Point size and haze from a view file, or the scene's own. The framing that
/// goes with them is `effective_camera`.
fn base_render_params(view: &Option<View>, scene: &Scene, haze_enabled: bool) -> (f32, Haze) {
    match view {
        Some(v) => (
            v.point_size,
            // A view's band follows the same rule as `App::apply_view`: only
            // a hand-pinned one overrides the auto range. A legacy view (no
            // `haze` amount) carried raw shader values chosen by hand, so its
            // band counts as pinned.
            Haze {
                pinned: (v.haze.is_none() || v.haze_band_pinned)
                    .then_some((v.haze_near, v.haze_far)),
                transmittance: v.haze_transmittance,
                saturation: v.haze_saturation,
            },
        ),
        None => {
            // `--fog` predates haze being scene data and now just means "on at
            // the old default strength"; the scene's own value wins. Same
            // precedence as `App::new`, so a `--render` matches what the
            // window shows.
            let amount = if scene.haze > 0.0 {
                scene.haze
            } else if haze_enabled {
                crate::haze::amount_from_brightness(0.4)
            } else {
                0.0
            };
            let (fb, fs) = crate::haze::falloff(amount);
            (
                scene.point_size,
                // Auto-ranged: resolved from whichever camera ends up rendering
                // each frame, not from the scene's authored distance. A
                // `--distance` flag now moves the haze with it too.
                Haze { pinned: None, transmittance: fb, saturation: fs },
            )
        }
    }
}

/// Which renderer draws the tiles
enum TileRenderer {
    Points(PointRenderer),
    Splat(SplatRenderer),
}

impl TileRenderer {
    /// Build the requested renderer over a filled point buffer. For splat,
    /// exposure is normalized against the buffer's point count and tile
    /// height, matching the interactive renderer.
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        compute: &PointCompute,
        splat: bool,
        exposure: f32,
        point_count: u32,
        height: u32,
        clear: wgpu::Color,
        transparent: bool,
    ) -> Self {
        if splat {
            let renderer = SplatRenderer::new(
                device, FORMAT, &compute.point_buffer, &compute.colormap_buffer,
            );
            renderer.upload_params(queue, exposure, point_count, height as f32, clear, transparent);
            TileRenderer::Splat(renderer)
        } else {
            TileRenderer::Points(PointRenderer::new(
                device, FORMAT, &compute.point_buffer, &compute.colormap_buffer,
            ))
        }
    }

    fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        match self {
            TileRenderer::Points(r) => r.upload_camera(queue, camera),
            TileRenderer::Splat(r) => r.upload_camera(queue, camera),
        }
    }
}

/// Reusable offscreen tile target: color + depth textures and a readback
/// buffer, rendered per tile and blitted into the CPU-side contact sheet
struct TileTarget {
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    color_texture: wgpu::Texture,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    /// What the point pass clears to — the scene's background, with alpha 0
    /// for a transparent render.
    clear: wgpu::Color,
}

impl TileTarget {
    fn new(device: &wgpu::Device, width: u32, height: u32, clear: wgpu::Color) -> Self {
        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offline_color"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offline_depth"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let padded_bytes_per_row = (width * 4 + 255) & !255;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offline_readback"),
            size: (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            color_view: color_texture.create_view(&wgpu::TextureViewDescriptor::default()),
            depth_view: depth_texture.create_view(&wgpu::TextureViewDescriptor::default()),
            color_texture,
            readback,
            width,
            height,
            padded_bytes_per_row,
            clear,
        }
    }

    /// Render one view of the point cloud and copy it into `sheet` at tile
    /// position (col, row)
    #[allow(clippy::too_many_arguments)]
    fn render_tile(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut TileRenderer,
        point_count: u32,
        use_point_primitives: bool,
        sheet: &mut [u8],
        sheet_w: u32,
        col: u32,
        row: u32,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offline_render_encoder"),
        });
        match renderer {
            TileRenderer::Points(renderer) => {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("offline_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(self.clear),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                renderer.draw(&mut pass, point_count, use_point_primitives);
            }
            TileRenderer::Splat(renderer) => {
                renderer.render(
                    device,
                    &mut encoder,
                    &self.color_view,
                    &self.depth_view,
                    self.width,
                    self.height,
                    point_count,
                    use_point_primitives,
                );
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        {
            let data = slice.get_mapped_range();
            let bytes_per_row = (self.width * 4) as usize;
            let x0 = (col * self.width * 4) as usize;
            let y0 = (row * self.height) as usize;
            for y in 0..self.height as usize {
                let src = y * self.padded_bytes_per_row as usize;
                let dst = (y0 + y) * (sheet_w * 4) as usize + x0;
                sheet[dst..dst + bytes_per_row].copy_from_slice(&data[src..src + bytes_per_row]);
            }
        }
        self.readback.unmap();
    }
}

/// Draw a tile's parameters into its top-left corner.
///
/// A contact sheet prints its per-tile mapping to stdout, which serves a human
/// at a terminal and nothing that reads the PNG. Labelling the tile makes the
/// sheet describe itself. Off with `--no-labels`.
fn label_tile(sheet: &mut [u8], sheet_w: u32, sheet_h: u32, tile_w: u32, tile_h: u32,
              col: u32, row: u32, text: &str) {
    let scale = glyphs::scale_for_tile(tile_w);
    let inset = 2 * scale;
    let (ox, oy) = (col * tile_w + inset, row * tile_h + inset);
    // Never let a label spill into the neighbouring tile.
    let max_w = tile_w.saturating_sub(2 * inset);
    if tile_h < glyphs::text_height(scale) + 2 * inset {
        return;
    }
    glyphs::draw_label(sheet, sheet_w, sheet_h, ox, oy, text, scale, max_w);
}

fn save_sheet(out_path: &Path, sheet: &[u8], w: u32, h: u32) -> Result<(), String> {
    if let Some(dir) = out_path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
        }
    }
    image::save_buffer(out_path, sheet, w, h, image::ColorType::Rgba8)
        .map_err(|e| format!("Failed to save {}: {}", out_path.display(), e))
}

fn print_timing(t0: Instant, t1: Instant, t2: Instant, t3: Instant, t4: Instant) {
    let secs = |a: Instant, b: Instant| (b - a).as_secs_f32();
    println!(
        "Timing: setup {:.2}s | chaos fill {:.2}s | render {:.2}s | encode+save {:.2}s | total {:.2}s",
        secs(t0, t1),
        secs(t1, t2),
        secs(t2, t3),
        secs(t3, t4),
        secs(t0, t4),
    );
}

/// Render a still, or a contact sheet of tiles sharing one point cloud.
///
/// Cancelling does not throw the work away: whatever the buffer holds is
/// rendered and saved, and the return says [`Outcome::Partial`] so no caller
/// can report it as a finished render.
pub fn render(params: OfflineParams) -> Result<Outcome, String> {
    let OfflineParams {
        mut scene,
        view,
        width,
        height,
        out_path,
        accumulate,
        haze_enabled,
        grid,
        splat,
        exposure,
        transparent,
        control,
        camera: camera_over,
        labels,
    } = params;
    let t_start = Instant::now();

    // A view can override the scene's color parameters. Falloff feeds the
    // chaos game (per-transform speeds), so apply it before compute setup.
    if let Some(f) = view.as_ref().and_then(|v| v.color_falloff) {
        scene.color_falloff = f;
        crate::scene::resolve_color_speeds(&mut scene.transforms, scene.color_speed, f);
    }
    let color_contrast = view
        .as_ref()
        .and_then(|v| v.color_contrast)
        .unwrap_or(scene.color_contrast);

    let clear = crate::scene::clear_color(scene.background, if transparent { 0.0 } else { 1.0 });

    let (device, queue) = create_device()?;
    let t_setup = Instant::now();

    let (compute, point_count, mut outcome) =
        fill_points(&device, &queue, &scene, accumulate, control.as_ref())?;
    let mut renderer =
        TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count, height, clear, transparent,
    );
    let t_fill = Instant::now();

    let (base_camera, point_size, haze, folded) = base_setup(&view, &scene, haze_enabled, camera_over);
    let zoom = scene_zoom(&scene)?;
    // The fold moved the camera; the points go with it. On one still this is
    // invisible — it permutes which point sits on which octave of a band that
    // is unchanged either way — and it is what makes a *run* of stills along a
    // flight comparable frame to frame. See `effective_camera_folded`.
    compute.rewrap(&device, &queue, folded);

    let aspect = width as f32 / height as f32;
    let tiles = build_tiles(&base_camera, grid, aspect);
    let (cols, rows) = grid.tile_count();
    let use_point_primitives = point_size * height as f32 / base_camera.distance <= 1.5;

    let target = TileTarget::new(&device, width, height, clear);
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u8; (sheet_w * sheet_h * 4) as usize];

    if let Some(c) = &control {
        c.phase("rendering");
    }
    for (idx, tile) in tiles.iter().enumerate() {
        // Second cancel point. A grid sheet is many tiles; a single still is
        // one, and by here the expensive part is already done, so stopping
        // saves what there is: the tiles rendered so far, with the rest of the
        // sheet left at the clear colour. Partial either way — the caller says
        // so rather than passing it off as the sheet that was asked for.
        if let Some(c) = &control {
            c.progress(idx as u32, tiles.len() as u32);
            if c.should_stop() {
                c.log(format!("stopped after {} of {} tiles", idx, tiles.len()));
                outcome = Outcome::Partial;
                break;
            }
        }
        // One band for the whole sheet, off the base framing: a contact sheet
        // is meant to be compared tile against tile, so the haze has to be the
        // same in each. Grid tiles only re-aim the camera anyway.
        let (haze_near, haze_far) = haze.band(base_camera.distance);
        let camera = CameraUniforms::new(
            tile.view_proj, height as f32, point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        // One guard for the whole sheet, off the base framing, for the same
        // reason as the haze band above: grid tiles only re-aim the camera.
        .with_zoom_guard(zoom.as_ref(), base_camera.eye());
        renderer.upload_camera(&queue, &camera);

        let (col, row) = (idx as u32 % cols, idx as u32 / cols);
        target.render_tile(
            &device, &queue, &mut renderer, point_count, use_point_primitives,
            &mut sheet, sheet_w, col, row,
        );
        if tiles.len() > 1 {
            println!("tile [row {}, col {}]: {}", row, col, tile.label);
            if labels {
                label_tile(&mut sheet, sheet_w, sheet_h, width, height, col, row, &tile.label);
            }
        }
    }
    let t_render = Instant::now();

    if let Some(c) = &control {
        c.phase("saving");
    }
    save_sheet(out_path, &sheet, sheet_w, sheet_h)?;
    let t_done = Instant::now();

    println!(
        "{} {}x{} ({} tile{} of {}x{}, {} points) -> {}",
        if outcome.is_partial() { "Stopped, partial render" } else { "Rendered" },
        sheet_w, sheet_h,
        tiles.len(), if tiles.len() == 1 { "" } else { "s" },
        width, height, point_count, out_path.display(),
    );
    print_timing(t_start, t_setup, t_fill, t_render, t_done);
    Ok(outcome)
}

/// Delete `<stem>.mutN.toml` leftovers from previous runs at the same out
/// path, so the variant files on disk always describe the current sheet
/// (a stale mutN also hijacks the comment-preserving save into merging with
/// the old variant). Purely best-effort: missing dirs/files, unreadable
/// entries, or racing deletions are all silently fine — the render itself
/// never depends on this.
fn remove_stale_variants(out_stem: &Path) {
    let Some(name) = out_stem.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let dir = match out_stem.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{name}.mut");
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(f) = file_name.to_str() else { continue };
        let is_variant = f
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".toml"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if is_variant {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Animation settings for `render_animation`
pub struct AnimParams {
    pub fps: u32,
    /// Duration override; None uses the path's own duration
    pub seconds: Option<f32>,
    /// Encode quality 0-100 (higher = better). Means the AV1 quantizer for
    /// AVIF and H.264's QP for MP4 — the same promise either way: pick a
    /// fidelity and let the bitrate land where it lands.
    pub quality: u8,
    /// Which file to write, and so which codec encodes it
    pub format: crate::video::Format,
    /// CPU threads the encoder may use. The job's one value, not the machine's
    /// full core count — see `render_job::default_threads`.
    pub threads: usize,
}

/// Render an animation: the camera flies the scene's [[camera.path]] spline
/// (or a seamless full-turn orbit when no path is authored) while the point
/// cloud stays fixed — one chaos fill, one cheap render pass per frame, frames
/// streamed straight into the encoder `anim.format` selects (AV1 for .avif,
/// H.264 for .mp4).
///
/// Cancelling behaves differently here than for a still, deliberately. Stopping
/// during the **fill** writes nothing: a sparse point cloud drawn across every
/// frame is a full-cost job at reduced quality, not a partial result, and
/// nobody who hit cancel wanted the remaining minutes spent. Stopping during
/// the **frame loop** keeps what has been encoded — a shorter clip is a real
/// partial result — provided there are at least two frames to mux.
pub fn render_animation(params: OfflineParams, anim: AnimParams) -> Result<Outcome, String> {
    let OfflineParams {
        mut scene,
        view,
        width,
        height,
        out_path,
        accumulate,
        haze_enabled,
        grid: _,
        splat,
        exposure,
        transparent,
        control,
        camera: camera_over,
        // animation writes frames, not a sheet: nothing to label
        labels: _,
    } = params;
    // 4:2:0 chroma needs even dimensions
    let (width, height) = (width & !1, height & !1);
    let t_start = Instant::now();

    if let Some(f) = view.as_ref().and_then(|v| v.color_falloff) {
        scene.color_falloff = f;
        crate::scene::resolve_color_speeds(&mut scene.transforms, scene.color_speed, f);
    }
    let color_contrast = view
        .as_ref()
        .and_then(|v| v.color_contrast)
        .unwrap_or(scene.color_contrast);

    let clear = crate::scene::clear_color(scene.background, if transparent { 0.0 } else { 1.0 });

    let (device, queue) = create_device()?;
    let t_setup = Instant::now();

    let (compute, point_count, fill) =
        fill_points(&device, &queue, &scene, accumulate, control.as_ref())?;
    // See this function's doc comment: an animation has nothing to keep from a
    // half-filled cloud, so a cancel here is a cancel.
    if fill.is_partial() {
        return Err(CANCELLED.to_string());
    }
    let mut renderer =
        TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count, height, clear, transparent,
    );
    let t_fill = Instant::now();

    let (base_camera, point_size, haze, _folded) = base_setup(&view, &scene, haze_enabled, camera_over);
    let zoom = scene_zoom(&scene)?;
    // A view overrides the base framing but the scene still owns the path.
    // `path::resolve` is the same rule the app flies (see `App::camera_path`),
    // so a preview in the window and this render agree about what the camera
    // does — including for a scene with no path, which gets a seamless full
    // orbit around the base framing.
    let default = CameraPath::full_orbit(&base_camera);
    let path = crate::path::resolve(scene.camera_path.as_ref(), &default).clone();
    let is_default = std::ptr::eq(
        crate::path::resolve(scene.camera_path.as_ref(), &default),
        &default,
    );

    let seconds = anim.seconds.unwrap_or_else(|| path.duration()).max(0.1);
    let frames = ((seconds * anim.fps as f32).round() as u32).max(2);
    println!(
        "Animating {} frames ({:.1}s at {} fps): {} keypoints, {}{}",
        frames,
        seconds,
        anim.fps,
        path.keys.len(),
        path.loops.kind().label(),
        if is_default { " (auto full orbit)" } else { "" },
    );
    println!("Encoding {} ({})", anim.format.codec_label(), anim.format.extension());

    let mut encoder = crate::video::AnimationEncoder::new(
        anim.format, width, height, anim.fps, anim.quality, 8, anim.threads,
    )?;
    println!("Encoding on {} thread{}", anim.threads, if anim.threads == 1 { "" } else { "s" });
    let aspect = width as f32 / height as f32;
    let target = TileTarget::new(&device, width, height, clear);
    let mut frame_buf = vec![0u8; (width * height * 4) as usize];

    if let Some(c) = &control {
        c.phase("rendering frames");
        c.log(format!("{} frames at {} fps, {:.1}s", frames, anim.fps, seconds));
    }
    // Fold depth the point buffer has been carried to; see the wrap below.
    let mut carried: Option<i32> = None;
    let mut rendered = 0u32;
    let mut outcome = Outcome::Complete;
    for i in 0..frames {
        // Third cancel point, and the one that matters most: an animation is
        // minutes of work and every frame is a natural place to stop. What has
        // been pushed to the encoder is a shorter clip, which is worth keeping
        // — but only if there is enough of it to mux, and the sample table
        // needs more than a single frame to describe a clip at all.
        if let Some(c) = &control {
            c.progress(i, frames);
            if c.should_stop() {
                if rendered < 2 {
                    return Err(CANCELLED.to_string());
                }
                c.log(format!(
                    "stopped after {} of {} frames — muxing the {:.1}s that rendered",
                    rendered,
                    frames,
                    rendered as f32 / anim.fps as f32,
                ));
                outcome = Outcome::Partial;
                break;
            }
        }
        // Closed paths exclude t=1 so the loop wraps without a repeated frame
        let t = if path.wraps() {
            i as f32 / frames as f32
        } else {
            i as f32 / (frames - 1) as f32
        };
        let mut cam = path.sample(t);
        // The one place the wrap really earns itself: a path whose distance
        // keys span many periods (they interpolate in log space, so that's a
        // constant-rate zoom) is folded back into one period every frame. The
        // camera never leaves f32's comfortable range and the zoom is seamless
        // for as long as the path asks for.
        if let Some(z) = &zoom {
            let depth = z.wrap(&mut cam);
            // And the points come with it, so the frame a wrap lands on is not
            // redrawn from a different set of them — the twitch `tools/
            // zoom_twitch.py` measures. What `wrap` returns here is the
            // absolute depth of an unwrapped spline sample rather than a step,
            // so carry the buffer by the *change* in it. The first frame has
            // nothing to be a change from and sets the reference.
            let step = depth - carried.unwrap_or(depth);
            compute.rewrap(&device, &queue, step);
            carried = Some(depth);
        }
        // After the wrap, so the band is derived from the camera that actually
        // renders this frame. That is what closes a zoom loop: the wrapped
        // camera and its haze scale together, so the last frame matches the
        // first instead of the image drifting out of a band nailed to the
        // scene's authored distance.
        let (haze_near, haze_far) = haze.band(cam.distance);
        let camera = CameraUniforms::new(
            cam.view_proj(aspect), height as f32, point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        // After the wrap too, and for the same reason: the guard is a ramp in
        // multiples of the eye distance, so it has to be rebuilt from the
        // camera that actually renders this frame.
        .with_zoom_guard(zoom.as_ref(), cam.eye());
        renderer.upload_camera(&queue, &camera);
        let use_point_primitives = point_size * height as f32 / cam.distance <= 1.5;
        target.render_tile(
            &device, &queue, &mut renderer, point_count, use_point_primitives,
            &mut frame_buf, width, 0, 0,
        );
        encoder.push_frame(&frame_buf)?;
        rendered += 1;
    }
    let t_render = Instant::now();

    // rav1e defers most of its work to the flush, so for AVIF the frame loop
    // hitting 100% is not the job being nearly done — say so, rather than
    // leaving the dialog parked at a full bar for another ten seconds. H.264
    // encodes on the way in and only has the muxing left.
    if let Some(c) = &control {
        c.phase("encoding");
        c.log(match anim.format {
            crate::video::Format::Avif => "flushing the AV1 encoder and muxing",
            crate::video::Format::Mp4 => "muxing the MP4",
        });
    }
    encoder.finish(out_path)?;
    let t_done = Instant::now();

    println!(
        "{} {}x{} {} animation ({} of {} frames, {} points) -> {}",
        if outcome.is_partial() { "Stopped, partial" } else { "Rendered" },
        width, height, anim.format.codec_label(), rendered, frames, point_count,
        out_path.display(),
    );
    println!(
        "Timing: setup {:.2}s | chaos fill {:.2}s | render+encode {:.2}s | flush+mux {:.2}s | total {:.2}s",
        (t_setup - t_start).as_secs_f32(),
        (t_fill - t_setup).as_secs_f32(),
        (t_render - t_fill).as_secs_f32(),
        (t_done - t_render).as_secs_f32(),
        (t_done - t_start).as_secs_f32(),
    );
    Ok(outcome)
}

/// Render a mutation contact sheet: tile 0 is the unmutated scene, tiles
/// 1..=count are random variants (each also saved as `<out>.mutN.toml` next
/// to the sheet so a good one can be kept). Unlike the camera grids, every
/// tile needs its own chaos-game fill.
pub fn render_mutations(
    params: OfflineParams,
    count: u32,
    strength: f32,
    seed: Option<u64>,
) -> Result<Outcome, String> {
    let OfflineParams {
        mut scene,
        view,
        width,
        height,
        out_path,
        accumulate,
        haze_enabled,
        grid: _,
        splat,
        exposure,
        transparent,
        control,
        camera: camera_over,
        labels,
    } = params;
    let t_start = Instant::now();

    if let Some(f) = view.as_ref().and_then(|v| v.color_falloff) {
        scene.color_falloff = f;
        crate::scene::resolve_color_speeds(&mut scene.transforms, scene.color_speed, f);
    }
    let color_contrast = view
        .as_ref()
        .and_then(|v| v.color_contrast)
        .unwrap_or(scene.color_contrast);

    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    });
    println!("Mutation seed: {} (pass --seed {} to reproduce)", seed, seed);

    // Build the variants (tile 0 = original)
    let mut variants: Vec<(Scene, String)> = vec![(scene.clone(), "original".to_string())];
    for i in 1..=count {
        let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(seed.wrapping_add(i as u64));
        let mut variant = scene.clone();
        let log = crate::mutate::mutate(&mut variant, &mut rng, strength);
        variants.push((variant, log.join("; ")));
    }

    // Sheet layout: near-square grid over count+1 tiles
    let n = variants.len() as u32;
    let cols = (n as f32).sqrt().ceil() as u32;
    let rows = n.div_ceil(cols);

    let clear = crate::scene::clear_color(scene.background, if transparent { 0.0 } else { 1.0 });

    let (device, queue) = create_device()?;
    let t_setup = Instant::now();

    let (base_camera, point_size, haze, _folded) = base_setup(&view, &scene, haze_enabled, camera_over);
    let aspect = width as f32 / height as f32;
    let view_proj = base_camera.view_proj(aspect);
    let use_point_primitives = point_size * height as f32 / base_camera.distance <= 1.5;
    let (haze_near, haze_far) = haze.band(base_camera.distance);
    // One framing and one haze band for every tile; only the edge guard is
    // per-variant, because mutating the transforms moves the zoom's fixed
    // point and a guard aimed at the wrong centre would fade the wrong side
    // of the picture. A variant whose zoom no longer resolves simply goes
    // unguarded — the tile is a still, and the sheet is worth more than the
    // one tile is.
    let camera = |zoom: Option<&crate::renorm::Renorm>| {
        CameraUniforms::new(
            view_proj, height as f32, point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        .with_zoom_guard(zoom, base_camera.eye())
    };

    let target = TileTarget::new(&device, width, height, clear);
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u8; (sheet_w * sheet_h * 4) as usize];

    let out_stem = out_path.with_extension("");
    remove_stale_variants(&out_stem);
    let mut fill_total = 0.0f32;
    let mut outcome = Outcome::Complete;
    let mut done = 0usize;
    for (idx, (variant, label)) in variants.iter().enumerate() {
        let t0 = Instant::now();
        // Each variant is a different IFS: refill the point buffer
        let (compute, point_count, fill) =
            fill_points(&device, &queue, variant, accumulate, control.as_ref())?;
        // A tile cut off mid-fill is still a tile — sparser than its
        // neighbours, which is exactly what a sheet read tile-against-tile must
        // not hide, so the whole sheet is reported partial.
        outcome = outcome.and(fill);
        let mut renderer =
            TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count, height, clear, transparent,
    );
        renderer.upload_camera(&queue, &camera(compute.zoom.as_ref()));
        fill_total += t0.elapsed().as_secs_f32();

        let (col, row) = (idx as u32 % cols, idx as u32 / cols);
        target.render_tile(
            &device, &queue, &mut renderer, point_count, use_point_primitives,
            &mut sheet, sheet_w, col, row,
        );

        if idx == 0 {
            println!("tile [row 0, col 0]: original");
        } else {
            let toml_path = format!("{}.mut{}.toml", out_stem.display(), idx);
            variant
                .save(&toml_path)
                .map_err(|e| format!("Failed to save variant {}: {}", idx, e))?;
            println!("tile [row {}, col {}]: {} -> {}", row, col, label, toml_path);
        }
        if labels {
            let text = if idx == 0 { "ORIGINAL" } else { label.as_str() };
            label_tile(&mut sheet, sheet_w, sheet_h, width, height, col, row, text);
        }
        done += 1;
        // Every tile is its own fill, so this is where a mutation sheet is
        // stopped. The tiles already drawn are worth keeping; the rest of the
        // sheet stays at the clear colour.
        if let Some(c) = &control {
            if c.should_stop() {
                c.log(format!("stopped after {} of {} tiles", done, n));
                outcome = Outcome::Partial;
                break;
            }
        }
    }
    let t_render = Instant::now();

    save_sheet(out_path, &sheet, sheet_w, sheet_h)?;
    let t_done = Instant::now();

    println!(
        "{} {}x{} ({} of {} mutation tiles of {}x{}) -> {}",
        if outcome.is_partial() { "Stopped, partial sheet" } else { "Rendered" },
        sheet_w, sheet_h, done, n, width, height, out_path.display(),
    );
    // Fill and render interleave per tile here; report fill separately
    println!(
        "Timing: setup {:.2}s | fills {:.2}s ({} tiles) | render+readback {:.2}s | encode+save {:.2}s | total {:.2}s",
        (t_setup - t_start).as_secs_f32(),
        fill_total,
        n,
        (t_render - t_setup).as_secs_f32() - fill_total,
        (t_done - t_render).as_secs_f32(),
        (t_done - t_start).as_secs_f32(),
    );
    Ok(outcome)
}

/// Render a parameter sweep as a labelled contact sheet.
///
/// Modelled on `render_mutations`, not on the camera grids, and the difference
/// matters: a swept parameter changes the IFS, so **every tile refills the
/// point buffer**. The camera grids share one fill because they only re-aim the
/// camera; doing that here would render the same attractor N times.
///
/// `build` turns a tile's extra `--set` arguments into a scene. It's a closure
/// rather than a path so that `main.rs` stays the single owner of the
/// load-then-`--zoom`-then-`--palette` pipeline; reloading from disk in here
/// would silently drop those.
pub fn render_sweep(
    params: OfflineParams,
    tiles: &[crate::sweep::Tile],
    cols: u32,
    rows: u32,
    build: &dyn Fn(&[String]) -> Result<Scene, String>,
) -> Result<Outcome, String> {
    let OfflineParams {
        scene,
        view,
        width,
        height,
        out_path,
        accumulate,
        haze_enabled,
        grid: _,
        splat,
        exposure,
        transparent,
        control,
        camera: camera_over,
        labels,
    } = params;
    let t_start = Instant::now();

    if let Some(f) = view.as_ref().and_then(|v| v.color_falloff) {
        // (the base scene is only used for framing and grading here)
        let _ = f;
    }
    let color_contrast = view
        .as_ref()
        .and_then(|v| v.color_contrast)
        .unwrap_or(scene.color_contrast);

    // Build every variant BEFORE touching the GPU. A sweep over a path that
    // doesn't resolve must fail once, up front — not N times, and not after a
    // minute of rendering.
    let variants: Vec<Scene> = tiles
        .iter()
        .map(|t| build(&t.sets))
        .collect::<Result<_, _>>()?;

    let clear = crate::scene::clear_color(scene.background, if transparent { 0.0 } else { 1.0 });

    let (device, queue) = create_device()?;
    let t_setup = Instant::now();

    // One framing and one haze band for the whole sheet: a contact sheet is
    // read tile against tile, so only the swept parameter may differ.
    let (base_camera, point_size, haze, _folded) = base_setup(&view, &scene, haze_enabled, camera_over);
    let aspect = width as f32 / height as f32;
    let view_proj = base_camera.view_proj(aspect);
    let use_point_primitives = point_size * height as f32 / base_camera.distance <= 1.5;
    let (haze_near, haze_far) = haze.band(base_camera.distance);
    // Per-variant edge guard: a sweep may be sweeping a zoom parameter, or
    // anything that moves the renormalizing map's fixed point.
    let camera = |zoom: Option<&crate::renorm::Renorm>| {
        CameraUniforms::new(
            view_proj, height as f32, point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        .with_zoom_guard(zoom, base_camera.eye())
    };

    let target = TileTarget::new(&device, width, height, clear);
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u8; (sheet_w * sheet_h * 4) as usize];

    let mut fill_total = 0.0f32;
    let mut outcome = Outcome::Complete;
    let mut done = 0usize;
    for (idx, (variant, tile)) in variants.iter().zip(tiles).enumerate() {
        // Every tile is its own fill, so this is where a sweep is stopped —
        // and the tiles already drawn are worth keeping. The rest of the sheet
        // stays at the clear colour, and the sheet is reported partial.
        if let Some(c) = &control {
            c.progress(idx as u32, tiles.len() as u32);
            if c.should_stop() {
                c.log(format!("stopped after {} of {} tiles", done, tiles.len()));
                outcome = Outcome::Partial;
                break;
            }
        }
        let t0 = Instant::now();
        let (compute, point_count, fill) =
            fill_points(&device, &queue, variant, accumulate, control.as_ref())?;
        outcome = outcome.and(fill);
        let mut renderer = TileRenderer::new(
            &device, &queue, &compute, splat, exposure, point_count, height, clear, transparent,
        );
        renderer.upload_camera(&queue, &camera(compute.zoom.as_ref()));
        fill_total += t0.elapsed().as_secs_f32();

        let (col, row) = (idx as u32 % cols, idx as u32 / cols);
        target.render_tile(
            &device, &queue, &mut renderer, point_count, use_point_primitives,
            &mut sheet, sheet_w, col, row,
        );
        if labels {
            label_tile(&mut sheet, sheet_w, sheet_h, width, height, col, row, &tile.label);
        }
        // The tile is fully described by one flag, so print that rather than
        // writing a variant file: it is copy-pasteable to reproduce or adopt.
        println!("tile [row {}, col {}]: {}", row, col, tile.description);
        done += 1;
    }
    let t_render = Instant::now();

    if let Some(c) = &control {
        c.phase("saving");
    }
    save_sheet(out_path, &sheet, sheet_w, sheet_h)?;
    let t_done = Instant::now();

    println!(
        "{} {}x{} ({} of {} sweep tiles of {}x{}) -> {}",
        if outcome.is_partial() { "Stopped, partial sheet" } else { "Rendered" },
        sheet_w, sheet_h, done, tiles.len(), width, height, out_path.display(),
    );
    println!(
        "Timing: setup {:.2}s | fills {:.2}s ({} tiles) | render+readback {:.2}s | encode+save {:.2}s | total {:.2}s",
        (t_setup - t_start).as_secs_f32(),
        fill_total,
        tiles.len(),
        (t_render - t_setup).as_secs_f32() - fill_total,
        (t_done - t_render).as_secs_f32(),
        (t_done - t_start).as_secs_f32(),
    );
    Ok(outcome)
}
