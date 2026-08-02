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
use crate::gpu::buffers::CameraUniforms;
use crate::gpu::{PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};
use crate::path::CameraPath;
use crate::scene::Scene;
use crate::render_job::{JobControl, CANCELLED};
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
                base.yaw.to_degrees(),
                base.pitch.to_degrees(),
                base.distance
            ),
        }],
        GridMode::Orbit { cols, rows } => {
            let n = cols * rows;
            (0..n)
                .map(|k| {
                    let mut cam = *base;
                    cam.yaw = base.yaw + k as f32 * std::f32::consts::TAU / n as f32;
                    TileView {
                        view_proj: cam.view_proj(aspect),
                        // Radians in parens: paste directly into [camera] yaw
                        label: format!("yaw {:.1}° ({:.4})", cam.yaw.to_degrees(), cam.yaw),
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

/// Run the chaos game until the point buffer is full plus `accumulate`
/// frames. Returns the compute pipeline and the valid point count.
/// Run the chaos game until the point buffer is full and then some. The long
/// phase of any job, and the first of the three places a job can be paused or
/// cancelled — `Err(CANCELLED)` means the caller should clean up and stop.
fn fill_points(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    accumulate: u32,
    control: Option<&JobControl>,
) -> Result<(PointCompute, u32), String> {
    let mut compute = PointCompute::new(
        device,
        &scene.transforms,
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
                    return Err(CANCELLED.to_string());
                }
            }
        }
    }
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    if let Some(c) = control {
        c.progress(total_frames, total_frames);
    }
    Ok((compute, point_count))
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

/// Base camera and render params from a view file, or scene defaults
fn base_setup(
    view: &Option<View>,
    scene: &Scene,
    haze_enabled: bool,
    over: CameraOverride,
) -> (OrbitCamera, f32, (f32, f32, f32, f32)) {
    let (mut camera, point_size, haze) = base_setup_unwrapped(view, scene, haze_enabled);
    // Flags last: they are the most specific thing anyone said.
    over.apply(&mut camera);
    // Under infinite zoom the framing is only defined up to a zoom period, so
    // put it in the canonical one before anything derives from it. A framing
    // that came from a view file is already there and this is a no-op.
    if let Ok(Some(zoom)) = scene_zoom(scene) {
        zoom.wrap(&mut camera);
    }
    if !over.is_empty() {
        // So a framing found by flags can be kept without transcription
        println!("{}", CameraOverride::describe(&camera));
    }
    (camera, point_size, haze)
}

fn base_setup_unwrapped(view: &Option<View>, scene: &Scene, haze_enabled: bool) -> (OrbitCamera, f32, (f32, f32, f32, f32)) {
    match view {
        Some(v) => (
            OrbitCamera::from_legacy(
                Vec3::from(v.focus),
                Vec3::from(v.offset),
                v.distance,
                v.rotation,
                v.pitch,
                v.roll,
            ),
            v.point_size,
            (v.haze_near, v.haze_far, v.haze_transmittance, v.haze_saturation),
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
            let (near, far) = crate::haze::auto_band(scene.camera_distance);
            let (fb, fs) = crate::haze::falloff(amount);
            (
                OrbitCamera {
                    yaw: scene.camera_yaw,
                    pitch: scene.camera_pitch,
                    distance: scene.camera_distance,
                    focus: scene.camera_focus,
                    roll: scene.camera_roll,
                },
                scene.point_size,
                (near, far, fb, fs),
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

pub fn render(params: OfflineParams) -> Result<(), String> {
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

    let (compute, point_count) = fill_points(&device, &queue, &scene, accumulate, control.as_ref())?;
    let mut renderer =
        TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count, height, clear, transparent,
    );
    let t_fill = Instant::now();

    let (base_camera, point_size, haze) = base_setup(&view, &scene, haze_enabled, camera_over);

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
        // one, and cancelling then falls through to the fill-points check
        // having already done the expensive part — which is the honest
        // outcome, not something to pretend around.
        if let Some(c) = &control {
            c.progress(idx as u32, tiles.len() as u32);
            if c.should_stop() {
                return Err(CANCELLED.to_string());
            }
        }
        let camera = CameraUniforms::new(
            tile.view_proj, height as f32, point_size, aspect, 1.0,
            haze.0, haze.1, haze.2, haze.3, color_contrast, scene.background.to_array(),
            transparent,
        );
        renderer.upload_camera(&queue, &camera);

        let (col, row) = (idx as u32 % cols, idx as u32 / cols);
        target.render_tile(
            &device, &queue, &mut renderer, point_count, use_point_primitives,
            &mut sheet, sheet_w, col, row,
        );
        if tiles.len() > 1 {
            println!("tile [row {}, col {}]: {}", row, col, tile.label);
        }
    }
    let t_render = Instant::now();

    if let Some(c) = &control {
        c.phase("saving");
    }
    save_sheet(out_path, &sheet, sheet_w, sheet_h)?;
    let t_done = Instant::now();

    println!(
        "Rendered {}x{} ({} tile{} of {}x{}, {} points) -> {}",
        sheet_w, sheet_h,
        tiles.len(), if tiles.len() == 1 { "" } else { "s" },
        width, height, point_count, out_path.display(),
    );
    print_timing(t_start, t_setup, t_fill, t_render, t_done);
    Ok(())
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
    /// AV1 quality 0-100 (higher = better)
    pub quality: u8,
}

/// Render an animated AVIF: the camera flies the scene's [[camera.path]]
/// spline (or a seamless full-turn orbit when no path is authored) while the
/// point cloud stays fixed — one chaos fill, one cheap render pass per frame,
/// frames streamed straight into the AV1 encoder.
pub fn render_animation(params: OfflineParams, anim: AnimParams) -> Result<(), String> {
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

    let (compute, point_count) = fill_points(&device, &queue, &scene, accumulate, control.as_ref())?;
    let mut renderer =
        TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count, height, clear, transparent,
    );
    let t_fill = Instant::now();

    let (base_camera, point_size, haze) = base_setup(&view, &scene, haze_enabled, camera_over);
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
        if path.closed { "closed loop" } else { "open path" },
        if is_default { " (auto full orbit)" } else { "" },
    );

    let mut encoder = crate::avif::AnimationEncoder::new(width, height, anim.fps, anim.quality, 8)?;
    let aspect = width as f32 / height as f32;
    let target = TileTarget::new(&device, width, height, clear);
    let mut frame_buf = vec![0u8; (width * height * 4) as usize];

    if let Some(c) = &control {
        c.phase("rendering frames");
        c.log(format!("{} frames at {} fps, {:.1}s", frames, anim.fps, seconds));
    }
    for i in 0..frames {
        // Third cancel point, and the one that matters most: an animation is
        // minutes of work and every frame is a natural place to stop.
        if let Some(c) = &control {
            c.progress(i, frames);
            if c.should_stop() {
                return Err(CANCELLED.to_string());
            }
        }
        // Closed paths exclude t=1 so the loop wraps without a repeated frame
        let t = if path.closed {
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
            z.wrap(&mut cam);
        }
        let camera = CameraUniforms::new(
            cam.view_proj(aspect), height as f32, point_size, aspect, 1.0,
            haze.0, haze.1, haze.2, haze.3, color_contrast, scene.background.to_array(),
            transparent,
        );
        renderer.upload_camera(&queue, &camera);
        let use_point_primitives = point_size * height as f32 / cam.distance <= 1.5;
        target.render_tile(
            &device, &queue, &mut renderer, point_count, use_point_primitives,
            &mut frame_buf, width, 0, 0,
        );
        encoder.push_frame(&frame_buf)?;
    }
    let t_render = Instant::now();

    // rav1e defers most of its work to the flush, so the frame loop hitting
    // 100% is not the job being nearly done. Say so, rather than leaving the
    // dialog parked at a full bar for another ten seconds.
    if let Some(c) = &control {
        c.phase("encoding");
        c.log("flushing the AV1 encoder and muxing");
    }
    encoder.finish(out_path)?;
    let t_done = Instant::now();

    println!(
        "Rendered {}x{} animation ({} frames, {} points) -> {}",
        width, height, frames, point_count, out_path.display(),
    );
    println!(
        "Timing: setup {:.2}s | chaos fill {:.2}s | render+encode {:.2}s | flush+mux {:.2}s | total {:.2}s",
        (t_setup - t_start).as_secs_f32(),
        (t_fill - t_setup).as_secs_f32(),
        (t_render - t_fill).as_secs_f32(),
        (t_done - t_render).as_secs_f32(),
        (t_done - t_start).as_secs_f32(),
    );
    Ok(())
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
) -> Result<(), String> {
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

    let (base_camera, point_size, haze) = base_setup(&view, &scene, haze_enabled, camera_over);
    let aspect = width as f32 / height as f32;
    let view_proj = base_camera.view_proj(aspect);
    let use_point_primitives = point_size * height as f32 / base_camera.distance <= 1.5;
    let camera = CameraUniforms::new(
        view_proj, height as f32, point_size, aspect, 1.0,
        haze.0, haze.1, haze.2, haze.3, color_contrast, scene.background.to_array(),
        transparent,
    );

    let target = TileTarget::new(&device, width, height, clear);
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u8; (sheet_w * sheet_h * 4) as usize];

    let out_stem = out_path.with_extension("");
    remove_stale_variants(&out_stem);
    let mut fill_total = 0.0f32;
    for (idx, (variant, label)) in variants.iter().enumerate() {
        let t0 = Instant::now();
        // Each variant is a different IFS: refill the point buffer
        let (compute, point_count) =
            fill_points(&device, &queue, variant, accumulate, control.as_ref())?;
        let mut renderer =
            TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count, height, clear, transparent,
    );
        renderer.upload_camera(&queue, &camera);
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
    }
    let t_render = Instant::now();

    save_sheet(out_path, &sheet, sheet_w, sheet_h)?;
    let t_done = Instant::now();

    println!(
        "Rendered {}x{} ({} mutation tiles of {}x{}) -> {}",
        sheet_w, sheet_h, n, width, height, out_path.display(),
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
    Ok(())
}
