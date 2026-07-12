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

use crate::camera::OrbitCamera;
use crate::gpu::buffers::CameraUniforms;
use crate::gpu::{PointCompute, PointRenderer, DEPTH_FORMAT};
use crate::scene::Scene;
use crate::view::View;

/// Matches the interactive renderer's clear color
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.02,
    b: 0.05,
    a: 1.0,
};

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
    pub fog_enabled: bool,
    pub grid: GridMode,
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
                        label: format!("yaw {:.1}°", cam.yaw.to_degrees()),
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
                    tiles.push(TileView {
                        view_proj: proj * view,
                        label: format!(
                            "{} / {} (of distance, still looking at focus)",
                            h(dx, "left", "right"),
                            h(dy, "up", "down"),
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
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
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
fn fill_points(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    accumulate: u32,
) -> (PointCompute, u32) {
    let mut compute = PointCompute::new(
        device,
        &scene.transforms,
        &scene.colormap,
        scene.point_count as u32,
    );
    let total_frames = compute.warmup_frames + accumulate.max(1);
    log::info!(
        "Filling {} point buffer: {} warmup + {} accumulation frames",
        scene.point_count, compute.warmup_frames, accumulate.max(1)
    );
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
        }
    }
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    (compute, point_count)
}

/// Base camera and render params from a view file, or scene defaults
fn base_setup(view: &Option<View>, scene: &Scene, fog_enabled: bool) -> (OrbitCamera, f32, (f32, f32, f32, f32)) {
    match view {
        Some(v) => (
            OrbitCamera::from_legacy(
                Vec3::from(v.focus),
                Vec3::from(v.offset),
                v.distance,
                v.rotation,
                v.pitch,
            ),
            v.point_size,
            (v.fog_near, v.fog_far, v.fog_brightness, v.fog_saturation),
        ),
        None => {
            let (fb, fs) = if fog_enabled { (0.4, 0.3) } else { (1.0, 1.0) };
            (
                OrbitCamera {
                    yaw: scene.camera_yaw,
                    pitch: scene.camera_pitch,
                    distance: scene.camera_distance,
                    focus: scene.camera_focus,
                },
                scene.point_size,
                (3.0, 4.5, fb, fs),
            )
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
}

impl TileTarget {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
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
        }
    }

    /// Render one view of the point cloud and copy it into `sheet` at tile
    /// position (col, row)
    #[allow(clippy::too_many_arguments)]
    fn render_tile(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &PointRenderer,
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
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offline_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
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
        fog_enabled,
        grid,
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

    let (device, queue) = create_device()?;
    let t_setup = Instant::now();

    let (compute, point_count) = fill_points(&device, &queue, &scene, accumulate);
    let renderer = PointRenderer::new(&device, FORMAT, &compute.point_buffer, &compute.colormap_buffer);
    let t_fill = Instant::now();

    let (base_camera, point_size, fog) = base_setup(&view, &scene, fog_enabled);

    let aspect = width as f32 / height as f32;
    let tiles = build_tiles(&base_camera, grid, aspect);
    let (cols, rows) = grid.tile_count();
    let use_point_primitives = point_size * height as f32 / base_camera.distance <= 1.5;

    let target = TileTarget::new(&device, width, height);
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u8; (sheet_w * sheet_h * 4) as usize];

    for (idx, tile) in tiles.iter().enumerate() {
        let camera = CameraUniforms::new(
            tile.view_proj, height as f32, point_size, aspect, 1.0,
            fog.0, fog.1, fog.2, fog.3, color_contrast,
        );
        renderer.upload_camera(&queue, &camera);

        let (col, row) = (idx as u32 % cols, idx as u32 / cols);
        target.render_tile(
            &device, &queue, &renderer, point_count, use_point_primitives,
            &mut sheet, sheet_w, col, row,
        );
        if tiles.len() > 1 {
            println!("tile [row {}, col {}]: {}", row, col, tile.label);
        }
    }
    let t_render = Instant::now();

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
        fog_enabled,
        grid: _,
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

    let (device, queue) = create_device()?;
    let t_setup = Instant::now();

    let (base_camera, point_size, fog) = base_setup(&view, &scene, fog_enabled);
    let aspect = width as f32 / height as f32;
    let view_proj = base_camera.view_proj(aspect);
    let use_point_primitives = point_size * height as f32 / base_camera.distance <= 1.5;
    let camera = CameraUniforms::new(
        view_proj, height as f32, point_size, aspect, 1.0,
        fog.0, fog.1, fog.2, fog.3, color_contrast,
    );

    let target = TileTarget::new(&device, width, height);
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u8; (sheet_w * sheet_h * 4) as usize];

    let out_stem = out_path.with_extension("");
    let mut fill_total = 0.0f32;
    for (idx, (variant, label)) in variants.iter().enumerate() {
        let t0 = Instant::now();
        // Each variant is a different IFS: refill the point buffer
        let (compute, point_count) = fill_points(&device, &queue, variant, accumulate);
        let renderer =
            PointRenderer::new(&device, FORMAT, &compute.point_buffer, &compute.colormap_buffer);
        renderer.upload_camera(&queue, &camera);
        fill_total += t0.elapsed().as_secs_f32();

        let (col, row) = (idx as u32 % cols, idx as u32 / cols);
        target.render_tile(
            &device, &queue, &renderer, point_count, use_point_primitives,
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
