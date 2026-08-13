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
use crate::gpu::points::accumulate::Accumulator;
use crate::gpu::points::splat::Grade;
use crate::gpu::points::downsample::{Downsampler, Filter, Source as FilterSource};
use crate::gpu::{PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};
use crate::path::CameraPath;
use crate::record::RenderRecord;
use crate::scene::Scene;
use crate::render_job::{JobControl, Outcome, CANCELLED};
use crate::view::View;

/// Extra chaos-game frames after the ring fills, when nothing says otherwise.
///
/// Named so the accumulating path can tell "the user asked for extra churn"
/// from "nobody mentioned it" — under `--spp` the flag has no meaning and
/// saying so is only worth it when it was actually set.
pub const DEFAULT_ACCUMULATE: u32 = 32;

/// How many bits per channel the PNG gets.
///
/// This is an **output format** choice, not a render-quality one: the render is
/// identical either way and only the quantization of the file differs. So the
/// default stays at 8 — a 16-bit PNG is twice the size, and most renders are a
/// check on the framing rather than a keeper.
///
/// It is worth having at all because supersampling produces exactly what 8 bits
/// bands: smooth wide gradients across a large area, where the step between two
/// adjacent codes is visible as a contour.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum BitDepth {
    #[default]
    #[value(name = "8")]
    Eight,
    #[value(name = "16")]
    Sixteen,
}

impl BitDepth {
    /// The colour target this depth renders into.
    ///
    /// Eight keeps `Rgba8UnormSrgb`, where the hardware does the sRGB encode on
    /// store — and keeping it is what makes an 8-bit render **byte-identical**
    /// to every render made before this existed. A float target with a CPU
    /// encode would round differently in the last bit and silently churn every
    /// image in the project.
    fn format(self) -> wgpu::TextureFormat {
        match self {
            BitDepth::Eight => wgpu::TextureFormat::Rgba8UnormSrgb,
            // Linear, so the sRGB encode moves to the readback below.
            BitDepth::Sixteen => wgpu::TextureFormat::Rgba16Float,
        }
    }

    /// Bits per channel, for a render record that a person will read.
    pub fn bits(self) -> u32 {
        match self {
            BitDepth::Eight => 8,
            BitDepth::Sixteen => 16,
        }
    }

    fn bytes_per_texel(self) -> u32 {
        match self {
            BitDepth::Eight => 4,
            BitDepth::Sixteen => 8,
        }
    }
}

/// Decode an IEEE 754 binary16.
///
/// Twelve lines rather than a dependency, and exhaustively tested below — there
/// are only 65536 inputs, so "tested" here means every one of them against the
/// reference conversion rather than against a handful of samples.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) as u32;
    let exp = ((bits >> 10) & 0x1F) as u32;
    let mant = (bits & 0x3FF) as u32;
    let out = if exp == 0 {
        if mant == 0 {
            sign << 31 // ±0
        } else {
            // Subnormal: renormalize into an f32 normal. `e` counts the shifts
            // needed to bring bit 10 up, and starts at 0 — a subnormal's value
            // is `mant * 2^-24`, so shifting k times lands the exponent at
            // `127 - 15 + 1 - k`.
            let mut e = 0i32;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            let m = m & 0x3FF;
            (sign << 31) | (((127 - 15 + 1 + e) as u32) << 23) | (m << 13)
        }
    } else if exp == 0x1F {
        (sign << 31) | (0xFF << 23) | (mant << 13) // inf / NaN
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(out)
}

/// The sRGB transfer function (IEC 61966-2-1), linear -> encoded.
///
/// The `Rgba8UnormSrgb` target does this in hardware; a float target does not,
/// so the 16-bit path does it here. Alpha is deliberately left linear, which is
/// what sRGB targets do too — coverage is not a colour.
fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    let out = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    // Clamped on the way out as well as in, so a kernel with negative lobes
    // upstream can't push a value out of range here.
    out.clamp(0.0, 1.0)
}

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
    /// Supersampling: render the histogram at `N x` output resolution and
    /// filter down. 1 is off. The single biggest visible quality win here —
    /// see `gpu::points::downsample`.
    pub supersample: u32,
    /// Reconstruction kernel for the downsample
    pub filter: Filter,
    /// Kernel half-width in *output* pixels
    pub filter_radius: f32,
    /// Bits per channel in the PNG. Ignored for animation, which is 8-bit by
    /// codec.
    pub bit_depth: BitDepth,
    /// Where the scene came from, for the render record's `[source]` block.
    /// `None` for a `--random` roll or a blank canvas, which have no file.
    pub scene_path: Option<std::path::PathBuf>,
    /// CPU threads this job may use. One value for the whole job — the
    /// encoders read it, and the record reports it as information.
    pub threads: usize,
    /// Measure and report GPU-busy time per chaos dispatch. Off by default:
    /// it is an investigation, not part of a render.
    pub gpu_timing: bool,
    /// Seeds the chaos game's walkers. The default reproduces every render
    /// made before this existed; any other value is an independent deal of the
    /// same attractor.
    pub chaos_seed: u64,
    /// Target samples per **output** pixel, accumulated into a persistent
    /// histogram. `None` is the ring-buffer render this program has always
    /// done, where the sample count is the buffer's capacity and nothing more.
    ///
    /// Per *output* pixel deliberately, not per accumulation texel: it is the
    /// user-facing quality dial, and it should not silently multiply by N²
    /// when you turn on supersampling.
    pub spp: Option<u32>,
    /// Tonemap grade: gamma, its toe, and vibrancy. [`Grade::NEUTRAL`] is the
    /// tonemap that existed before these knobs did, so an unspecified grade
    /// leaves every earlier render byte-identical.
    pub grade: Grade,
    /// Save the pre-tonemap linear density beside the PNG, so the grade can be
    /// redone without re-rendering. See `grade_file`.
    pub grade_out: Option<std::path::PathBuf>,
}

/// Everything about a render that is measured in render-target pixels.
///
/// One value, derived once, because the failure mode of supersampling is
/// **disagreement**: the camera's `screen_height`, the near-field size cap and
/// the subpixel `use_point_primitives` test all describe the same target, and
/// any one of them left on the output size silently cancels the feature for
/// part of the picture. `use_point_primitives` is the worst of the three —
/// points that are subpixel at output but not at accumulation resolution would
/// keep taking the unfiltered 1px path, which is exactly the finest, most
/// alias-prone material there is, and the result looks like the feature did
/// nothing at all.
#[derive(Clone, Copy)]
struct Sampling {
    /// Supersample factor N, at least 1
    n: u32,
    /// Output height in pixels
    height: u32,
}

impl Sampling {
    fn new(supersample: u32, height: u32) -> Self {
        Self { n: supersample.max(1), height }
    }

    /// Height of the surface actually being rasterized into.
    /// Refuse a factor whose accumulation would not fit in a texture.
    ///
    /// Supersampling multiplies both axes, and 4x at 4K wants 15360 px against
    /// a common limit of 8192 — which arrives, without this, as a wgpu
    /// validation panic naming a number the user never typed. Checked against
    /// the device rather than a constant because the limit is the adapter's.
    ///
    /// Every entry point that builds a supersampled target has to call this;
    /// there is no type that can force it, so the five call sites sit directly
    /// after `create_device` where they can be read off together.
    fn check_fits(&self, device: &wgpu::Device, width: u32, height: u32) -> Result<(), String> {
        let max = device.limits().max_texture_dimension_2d;
        let (w, h) = (width * self.n, height * self.n);
        if w > max || h > max {
            return Err(format!(
                "--supersample {} makes a {}x{} accumulation, past this GPU's {}px texture limit \
                 — render smaller, or lower --supersample",
                self.n, w, h, max
            ));
        }
        Ok(())
    }

    fn target_height(&self) -> f32 {
        (self.height * self.n) as f32
    }

    /// Whether points are small enough for the native 1px point-primitive
    /// path. Measured against the *target* height, not the output height.
    fn use_point_primitives(&self, point_size: f32, distance: f32) -> bool {
        point_size * self.target_height() / distance <= 1.5
    }
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
fn create_device() -> Result<(wgpu::Device, wgpu::Queue, String), String> {
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
    let adapter_name = adapter.get_info().name.clone();
    log::info!("Offline render using adapter: {:?}", adapter_name);

    let adapter_limits = adapter.limits();
    let required_limits = wgpu::Limits {
        max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
        max_buffer_size: adapter_limits.max_buffer_size,
        ..wgpu::Limits::default()
    };
    // Asked for when the adapter has it, never required. It costs nothing when
    // unused and is the only way to tell fixed dispatch overhead from
    // proportional GPU time — but a device that cannot offer it must still
    // render. See `gpu::timing`.
    let required_features =
        adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("fracturize_offline_device"),
        required_features,
        required_limits,
        memory_hints: wgpu::MemoryHints::Performance,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: Default::default(),
    }))
    .map_err(|e| format!("Failed to create device: {}", e))?;
    Ok((device, queue, adapter_name))
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
    gpu_timing: bool,
    chaos_seed: u64,
) -> Result<(PointCompute, u32, Outcome), String> {
    let mut compute = PointCompute::new(
        device,
        &scene.transforms,
        &scene.colors,
        &scene.colormap,
        scene.point_count as u32,
        chaos_seed,
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
    // A sample of the first few hundred dispatches, which is plenty to
    // characterize a loop that runs identically twelve thousand times. `None`
    // unless asked for, and `None` anyway on a device without the feature.
    let mut timer = gpu_timing
        .then(|| crate::gpu::GpuTimer::new(device, queue, GPU_TIMING_SAMPLES))
        .flatten();
    for i in 0..total_frames {
        point_count = compute.advance_frame(queue, 1.0 / 60.0);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offline_compute_encoder"),
        });
        compute.dispatch_timed(&mut encoder, timer.as_mut());
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
    if let Some(t) = &timer {
        let ms = t.read(device, queue);
        // Per *dispatch*, and stated as such. The question this answers is
        // whether a bigger batch would amortize anything: if the median is flat
        // as `iterations_per_walker` grows, the loop is overhead-bound and the
        // batch should grow; if it scales, the GPU was already the limit.
        println!(
            "{}",
            crate::gpu::timing::summarize("GPU chaos dispatch", &ms)
        );
        println!(
            "  {} walkers x {} iters = {} points/dispatch, {} dispatches",
            compute.num_walkers,
            compute.iterations_per_walker,
            compute.num_walkers * compute.iterations_per_walker,
            total_frames,
        );
    }
    Ok((compute, point_count, outcome))
}

/// How many chaos dispatches `--gpu-timing` measures.
///
/// A sample, not the whole run: the loop is identical every iteration, so a few
/// hundred describe it, and a query set sized for all ~12,000 would be 200 KB
/// of GPU memory to answer a question the first 256 already answer.
const GPU_TIMING_SAMPLES: u32 = 256;

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
    /// Build the requested renderer over a filled point buffer.
    ///
    /// For splat, exposure is normalized against the buffer's point count and
    /// the **accumulation** height, matching the interactive renderer. That the
    /// height is the supersampled one is what keeps brightness invariant to the
    /// factor: the filter takes a weighted *mean* over each N x N block, which
    /// divides density by N², and `exposure_scale` carries an N²-larger
    /// height² that cancels it exactly. Passing the output height here would
    /// darken every supersampled render by N².
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        compute: &PointCompute,
        splat: bool,
        exposure: f32,
        // What exposure is normalized against: the ring's point count in the
        // ordinary path, the total accumulated sample count when there is a
        // persistent histogram behind the tonemap.
        samples: f64,
        sampling: Sampling,
        filter: Filter,
        filter_radius: f32,
        depth: BitDepth,
        clear: wgpu::Color,
        transparent: bool,
        grade: Grade,
    ) -> Self {
        if splat {
            let mut renderer = SplatRenderer::new(
                device, depth.format(), &compute.point_buffer, &compute.colormap_buffer,
            );
            renderer.set_supersample(device, queue, sampling.n, filter, filter_radius);
            renderer.upload_params(
                queue,
                exposure,
                samples,
                sampling.target_height(),
                clear,
                transparent,
                grade,
            );
            TileRenderer::Splat(renderer)
        } else {
            TileRenderer::Points(PointRenderer::new(
                device, depth.format(), &compute.point_buffer, &compute.colormap_buffer,
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

/// Reusable offscreen tile target: colour + depth textures and a readback
/// buffer, rendered per tile and blitted into the CPU-side contact sheet.
///
/// Under `N x` supersampling this owns two colour surfaces rather than one: an
/// `N·W x N·H` one that gets rasterized into, and the output-sized one that
/// gets read back, with the reconstruction filter between them. Only the
/// **points** renderer uses that pair — the splat renderer supersamples
/// internally, because its filter has to run on the linear accumulation
/// *before* the log tonemap, which is a place only it can reach.
struct TileTarget {
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    color_texture: wgpu::Texture,
    readback: wgpu::Buffer,
    /// Output size — what lands in the sheet
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    /// What the point pass clears to — the scene's background, with alpha 0
    /// for a transparent render.
    clear: wgpu::Color,
    /// The supersampled surfaces and the filter, when `n > 1`. `None` at 1x,
    /// so an unsupersampled render allocates and encodes exactly what it
    /// always did.
    supersampled: Option<Supersampled>,
    /// Which colour target was allocated, and so how the readback is decoded.
    depth_bits: BitDepth,
}

struct Supersampled {
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    downsampler: Downsampler,
}

impl TileTarget {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        clear: wgpu::Color,
        sampling: Sampling,
        filter: Filter,
        filter_radius: f32,
        depth: BitDepth,
    ) -> Self {
        let make_color = |label, extra| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: width * if extra { sampling.n } else { 1 },
                    height: height * if extra { sampling.n } else { 1 },
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: depth.format(),
                usage: if extra {
                    // Read by the filter pass rather than copied out
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
                } else {
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
                },
                view_formats: &[],
            })
        };
        let make_depth = |label, n: u32| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: width * n,
                        height: height * n,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };

        let color_texture = make_color("offline_color", false);
        // Output-sized: the splat tonemap pass clears it, and a colour
        // attachment and a depth attachment in one pass must agree on size.
        let depth_view = make_depth("offline_depth", 1);

        let supersampled = (sampling.n > 1).then(|| {
            let ss_color = make_color("offline_color_supersampled", true);
            // The points renderer is depth-tested, so its depth buffer has to
            // match the surface it is rasterizing into, not the output.
            let ss_depth = make_depth("offline_depth_supersampled", sampling.n);
            let downsampler = Downsampler::new(device, depth.format());
            // The points target is sRGB with straight alpha: `textureLoad`
            // decodes to linear and the sRGB target re-encodes on store, so
            // the averaging happens in linear light for free, and the
            // premultiply/unpremultiply keeps a transparent render's dusty
            // edges from darkening the colour they sit next to.
            downsampler.upload_params(
                queue,
                sampling.n,
                filter,
                filter_radius,
                FilterSource::StraightAlpha,
            );
            Supersampled {
                color_view: ss_color.create_view(&wgpu::TextureViewDescriptor::default()),
                depth_view: ss_depth,
                downsampler,
            }
        });

        let padded_bytes_per_row = (width * depth.bytes_per_texel() + 255) & !255;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offline_readback"),
            size: (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            color_view: color_texture.create_view(&wgpu::TextureViewDescriptor::default()),
            depth_view,
            color_texture,
            readback,
            width,
            height,
            padded_bytes_per_row,
            clear,
            supersampled,
            depth_bits: depth,
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
        sheet: &mut [u16],
        sheet_w: u32,
        col: u32,
        row: u32,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offline_render_encoder"),
        });
        match renderer {
            TileRenderer::Points(renderer) => {
                // Draw into the supersampled surface when there is one, then
                // filter down into the output-sized one the readback copies.
                let (color, depth) = match &self.supersampled {
                    Some(ss) => (&ss.color_view, &ss.depth_view),
                    None => (&self.color_view, &self.depth_view),
                };
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("offline_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: color,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(self.clear),
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: Some(
                            wgpu::RenderPassDepthStencilAttachment {
                                view: depth,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(1.0),
                                    store: wgpu::StoreOp::Discard,
                                }),
                                stencil_ops: None,
                            },
                        ),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    renderer.draw(&mut pass, point_count, use_point_primitives);
                }
                if let Some(ss) = &self.supersampled {
                    ss.downsampler.pass(device, &mut encoder, &ss.color_view, &self.color_view);
                }
            }
            TileRenderer::Splat(renderer) => {
                // Output size: the splat renderer sizes its own accumulation
                // from the factor it was given.
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
        self.copy_out(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        self.read_into_sheet(device, sheet, sheet_w, col, row);
    }

    /// Queue the colour target's copy into the readback buffer.
    fn copy_out(&self, encoder: &mut wgpu::CommandEncoder) {
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
    }

    /// Map the readback, decode it to 16-bit sRGB, and blit it into the sheet
    /// at tile position `(col, row)`. The copy must already have been submitted.
    fn read_into_sheet(
        &self,
        device: &wgpu::Device,
        sheet: &mut [u16],
        sheet_w: u32,
        col: u32,
        row: u32,
    ) {
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        {
            let data = slice.get_mapped_range();
            let x0 = (col * self.width * 4) as usize;
            let y0 = (row * self.height) as usize;
            for y in 0..self.height as usize {
                let src = y * self.padded_bytes_per_row as usize;
                let dst = (y0 + y) * (sheet_w * 4) as usize + x0;
                let row_out = &mut sheet[dst..dst + (self.width * 4) as usize];
                match self.depth_bits {
                    // The hardware already encoded sRGB into 8 bits; widening
                    // by 257 is exactly reversible, so the sheet can be 16 bits
                    // wide without an 8-bit save writing anything different.
                    BitDepth::Eight => {
                        let src_row = &data[src..src + (self.width * 4) as usize];
                        for (o, &b) in row_out.iter_mut().zip(src_row) {
                            *o = b as u16 * 257;
                        }
                    }
                    // Linear f16 off a float target: encode sRGB here, and
                    // leave alpha alone — coverage is not a colour, which is
                    // also what an sRGB target does.
                    BitDepth::Sixteen => {
                        let src_row = &data[src..src + (self.width * 8) as usize];
                        for (i, o) in row_out.iter_mut().enumerate() {
                            let h = u16::from_le_bytes([src_row[i * 2], src_row[i * 2 + 1]]);
                            let v = f16_to_f32(h);
                            let v = if i % 4 == 3 { v.clamp(0.0, 1.0) } else { linear_to_srgb(v) };
                            *o = (v * 65535.0 + 0.5) as u16;
                        }
                    }
                }
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
fn label_tile(sheet: &mut [u16], sheet_w: u32, sheet_h: u32, tile_w: u32, tile_h: u32,
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

/// Write the finished sheet, with the render record embedded in it.
///
/// Drives `png::Encoder` directly rather than going through
/// `image::save_buffer`: `image`'s convenience wrapper has no passthrough for
/// text chunks, and carrying its own recipe is most of what makes a render
/// findable a year later. See `src/record.rs`.
///
/// The sheet is always 16 bits per channel; an 8-bit save narrows it back by
/// 257, which is exact for anything that came off an 8-bit target — so this
/// writes the same pixels it wrote before 16-bit output existed.
fn save_sheet(
    out_path: &Path,
    sheet: &[u16],
    w: u32,
    h: u32,
    depth: BitDepth,
    record: Option<&RenderRecord>,
) -> Result<(), String> {
    if let Some(dir) = out_path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
        }
    }
    let err = |e: std::io::Error| format!("Failed to save {}: {}", out_path.display(), e);
    let file = std::fs::File::create(out_path).map_err(err)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(match depth {
        BitDepth::Eight => png::BitDepth::Eight,
        BitDepth::Sixteen => png::BitDepth::Sixteen,
    });

    if let Some(r) = record {
        for (keyword, text) in r.png_chunks() {
            // `iTXt` throughout, not `tEXt`: tEXt is Latin-1, and a scene name
            // or an author's name is UTF-8 in general — `α-0.4` alone would not
            // survive it. iTXt is the chunk PNG defines for UTF-8, and it is
            // compressed, which matters for the whole-scene chunk.
            //
            // Best-effort: a keyword the encoder rejects must not cost the
            // render. The test in `record.rs` is what keeps that from being a
            // silent loss, since it checks every keyword against the spec.
            if let Err(e) = enc.add_itxt_chunk(keyword.clone(), text) {
                log::warn!("PNG metadata chunk {:?} was not written: {}", keyword, e);
            }
        }
    }

    let mut writer = enc.write_header().map_err(|e| encode_err(out_path, e))?;
    match depth {
        BitDepth::Eight => {
            let narrowed: Vec<u8> = sheet.iter().map(|&v| (v / 257) as u8).collect();
            writer.write_image_data(&narrowed).map_err(|e| encode_err(out_path, e))?;
        }
        BitDepth::Sixteen => {
            // PNG is big-endian; the sheet is native. Written explicitly rather
            // than transmuted, so this is correct on either endianness.
            let mut bytes = Vec::with_capacity(sheet.len() * 2);
            for &v in sheet {
                bytes.extend_from_slice(&v.to_be_bytes());
            }
            writer.write_image_data(&bytes).map_err(|e| encode_err(out_path, e))?;
        }
    }
    writer.finish().map_err(|e| encode_err(out_path, e))
}

fn encode_err(out_path: &Path, e: png::EncodingError) -> String {
    format!("Failed to save {}: {}", out_path.display(), e)
}

/// Build the record for a finished render, and file the sidecar.
///
/// Failing to write the receipt never fails the render — the picture exists
/// either way, and reporting a successful render as failed because a text file
/// could not be written would be the wrong trade. It is logged and printed
/// instead, so it is not silent either.
#[allow(clippy::too_many_arguments)]
fn make_record(
    scene: &Scene,
    scene_path: Option<&Path>,
    camera: &OrbitCamera,
    quality: crate::record::Quality,
    threads: usize,
    adapter: String,
    elapsed: f32,
) -> Option<RenderRecord> {
    let scene_toml = match scene.to_toml_string() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("no render record: the scene would not serialize: {}", e);
            return None;
        }
    };
    Some(RenderRecord {
        scene_path: scene_path.map(|p| p.to_path_buf()),
        scene_toml,
        camera: *camera,
        quality,
        machine: crate::record::Machine {
            threads,
            elapsed_seconds: elapsed,
            adapter,
        },
        created: crate::record::timestamp_utc(),
    })
}

/// Read an `Rgba32Float` texture back to CPU floats.
///
/// Used for the grade buffer, whose whole premise is that these values are
/// *exactly* the tonemap's input — so this copies the texture verbatim rather
/// than going anywhere near the tonemap or the 8/16-bit output path.
/// Both float formats in this pipeline appear here, and getting that wrong is
/// not a subtle error: the two splat paths differ in it. The accumulating path
/// resolves its histogram into `Rgba32Float`, while the ordinary ring-buffer
/// path's accumulation is `Rgba16Float` — the widest format with guaranteed
/// blending support. Reading fp16 pairs as f32 produces confident garbage,
/// which is exactly what it did the first time.
fn read_float_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<f32> {
    let half = match texture.format() {
        wgpu::TextureFormat::Rgba16Float => true,
        wgpu::TextureFormat::Rgba32Float => false,
        other => panic!("grade buffer readback does not know {:?}", other),
    };
    let bytes_per_texel: u32 = if half { 8 } else { 16 };
    let padded = (width * bytes_per_texel).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grade_readback"),
        size: (padded * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("grade_readback_encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    {
        let data = slice.get_mapped_range();
        for y in 0..height as usize {
            let row = y * padded as usize;
            for i in 0..(width * 4) as usize {
                if half {
                    let o = row + i * 2;
                    out.push(f16_to_f32(u16::from_le_bytes([data[o], data[o + 1]])));
                } else {
                    let o = row + i * 4;
                    out.push(f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]));
                }
            }
        }
    }
    readback.unmap();
    out
}

/// Which tonemap knob a `--grade-sweep` walks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum GradeAxis {
    Exposure,
    Gamma,
    GammaThreshold,
    Vibrancy,
}

impl GradeAxis {
    fn label(self) -> &'static str {
        match self {
            GradeAxis::Exposure => "exposure",
            GradeAxis::Gamma => "gamma",
            GradeAxis::GammaThreshold => "gamma_threshold",
            GradeAxis::Vibrancy => "vibrancy",
        }
    }

    fn apply(self, v: f32, exposure: &mut f32, grade: &mut Grade) {
        match self {
            GradeAxis::Exposure => *exposure = v,
            GradeAxis::Gamma => grade.gamma = v,
            GradeAxis::GammaThreshold => grade.gamma_threshold = v,
            GradeAxis::Vibrancy => grade.vibrancy = v,
        }
    }
}

/// A tonemap sweep: one axis, walked across a contact sheet.
///
/// This is what slice 5 is *for*. Every tonemap question in the plan —
/// which gamma, how much toe, whether adaptive exposure normalization is worth
/// having — was previously answerable only by re-rendering per guess. From one
/// saved buffer the whole sheet costs one device creation and N fullscreen
/// passes, so the answer arrives as a picture instead of an argument.
#[derive(Clone, Copy, Debug)]
pub struct GradeSweep {
    pub axis: GradeAxis,
    pub from: f32,
    pub to: f32,
    pub steps: usize,
}

/// Re-grade a saved linear buffer: upload it, run the tonemap, save a PNG.
///
/// No scene, no chaos game, no point buffer — the tonemap never needed any of
/// those, and that is the whole point. What used to cost a whole render now
/// costs one fullscreen pass, which is what turns "which gamma?" from an
/// argument into a thing you look at.
///
/// The buffer's own exposure and grade are the starting point; `exposure` and
/// `grade` here override them. So `--retonemap x.fgrade -r out.png` with no
/// other flags reproduces the render it came from.
pub fn retonemap(
    path: &Path,
    out_path: &Path,
    exposure: Option<f32>,
    gamma: Option<f32>,
    gamma_threshold: Option<f32>,
    vibrancy: Option<f32>,
    bit_depth: BitDepth,
    sweep: Option<GradeSweep>,
    labels: bool,
) -> Result<Outcome, String> {
    let t_start = Instant::now();
    let buf = crate::grade_file::GradeBuffer::read(path)?;
    let (width, height) = (buf.width, buf.height);
    let exposure = exposure.unwrap_or(buf.exposure);
    let grade = Grade {
        gamma: gamma.unwrap_or(buf.grade.gamma),
        gamma_threshold: gamma_threshold.unwrap_or(buf.grade.gamma_threshold),
        vibrancy: vibrancy.unwrap_or(buf.grade.vibrancy),
    };

    let (device, queue, _adapter) = create_device()?;
    let t_setup = Instant::now();

    // Upload the linear density as a texture the tonemap can read. Same format
    // the accumulator resolves into, because that is what it is.
    let linear = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("retonemap_linear"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::gpu::points::accumulate::RESOLVED_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut bytes = Vec::with_capacity(buf.pixels.len() * 4);
    for v in &buf.pixels {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &linear,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 16),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    let linear_view = linear.create_view(&wgpu::TextureViewDescriptor::default());

    // A splat renderer with no points behind it: only its tonemap pipeline and
    // params buffer are used, which is exactly the pure function this re-runs.
    let empty = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("retonemap_unused_points"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let renderer = SplatRenderer::new(&device, bit_depth.format(), &empty, &empty);
    let clear = wgpu::Color {
        r: buf.background[0] as f64,
        g: buf.background[1] as f64,
        b: buf.background[2] as f64,
        a: if buf.transparent { 0.0 } else { 1.0 },
    };
    renderer.upload_params(
        &queue,
        exposure,
        buf.samples,
        buf.screen_height,
        clear,
        buf.transparent,
        grade,
    );

    // 1x: the buffer is already output-sized and already filtered.
    let sampling = Sampling::new(1, height);
    let target = TileTarget::new(
        &device, &queue, width, height, clear, sampling, Filter::default(),
        crate::gpu::points::downsample::DEFAULT_FILTER_RADIUS, bit_depth,
    );

    // One tile, or a contact sheet walking one knob. Same loop: the sweep is
    // just N of the thing the single case does once, which is the point of the
    // buffer existing at all.
    let steps = sweep.map_or(1, |s| s.steps.max(1));
    let (cols, rows) = grid_shape(steps);
    let (sheet_w, sheet_h) = (width * cols, height * rows);
    let mut sheet = vec![0u16; (sheet_w * sheet_h * 4) as usize];
    let bind_group = renderer.tonemap_bind_group(&device, &linear_view);

    for i in 0..steps {
        let (mut e, mut g) = (exposure, grade);
        let label = match sweep {
            Some(sw) => {
                // Inclusive of both ends, so a sweep says what it was asked
                // for: `1:4` in 4 steps is 1, 2, 3, 4 and not 1, 1.75, 2.5, 3.25.
                let t = if steps == 1 { 0.0 } else { i as f32 / (steps - 1) as f32 };
                let v = sw.from + (sw.to - sw.from) * t;
                sw.axis.apply(v, &mut e, &mut g);
                format!("{} {:.4}", sw.axis.label(), v)
            }
            None => String::new(),
        };
        renderer.upload_params(
            &queue, e, buf.samples, buf.screen_height, clear, buf.transparent, g,
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("retonemap_encoder"),
        });
        renderer.tonemap_pass(&mut encoder, &target.color_view, &target.depth_view, &bind_group);
        target.copy_out(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        let (col, row) = (i as u32 % cols, i as u32 / cols);
        target.read_into_sheet(&device, &mut sheet, sheet_w, col, row);
        if sweep.is_some() {
            println!("tile [row {}, col {}]: {}", row, col, label);
            if labels {
                label_tile(&mut sheet, sheet_w, sheet_h, width, height, col, row, &label);
            }
        }
    }
    let t_render = Instant::now();

    save_sheet(out_path, &sheet, sheet_w, sheet_h, bit_depth, None)?;
    let t_done = Instant::now();
    match sweep {
        Some(sw) => println!(
            "Re-graded {} tiles of {}x{}, {} from {} to {} -> {}",
            steps, width, height, sw.axis.label(), sw.from, sw.to, out_path.display(),
        ),
        None => println!(
            "Re-graded {}x{} (exposure {}, gamma {}, threshold {}, vibrancy {}) -> {}",
            width, height, exposure, grade.gamma, grade.gamma_threshold, grade.vibrancy,
            out_path.display(),
        ),
    }
    print_timing(t_start, t_setup, t_setup, t_render, t_done);
    Ok(Outcome::Complete)
}

/// The tidiest grid for `n` tiles: as square as possible, wider than tall.
///
/// A sweep has no natural two-dimensional shape the way an orbit grid does, so
/// this picks one rather than making the caller say.
fn grid_shape(n: usize) -> (u32, u32) {
    let cols = (n as f64).sqrt().ceil().max(1.0) as u32;
    (cols, (n as u32).div_ceil(cols))
}

/// Save the pre-tonemap linear density, if asked for.
///
/// Best-effort in the same sense the sidecar is: the picture exists, and a
/// checkpoint that could not be filed is worth saying out loud but is not a
/// failed render.
#[allow(clippy::too_many_arguments)]
fn save_grade_buffer(
    path: &Path,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    samples: f64,
    screen_height: f32,
    clear: wgpu::Color,
    transparent: bool,
    exposure: f32,
    grade: Grade,
    record: Option<&RenderRecord>,
) {
    let buf = crate::grade_file::GradeBuffer {
        width,
        height,
        samples,
        screen_height,
        background: [clear.r as f32, clear.g as f32, clear.b as f32],
        transparent,
        exposure,
        grade,
        pixels: read_float_texture(device, queue, texture, width, height),
    };
    match buf.write(path, record) {
        Ok(()) => println!(
            "Grade buffer: {} ({:.0} MB) — re-grade with --retonemap",
            path.display(),
            (width as f64 * height as f64 * 16.0) / 1e6
        ),
        Err(e) => eprintln!("warning: {}", e),
    }
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
    if params.spp.is_some() {
        return render_accumulated(params);
    }
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
        supersample,
        filter,
        filter_radius,
        bit_depth,
        scene_path,
        threads,
        gpu_timing,
        chaos_seed,
        // `Some` took the accumulating path at the top of this function.
        spp: _,
        grade,
        grade_out,
    } = params;
    let sampling = Sampling::new(supersample, height);
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

    let (device, queue, adapter) = create_device()?;
    sampling.check_fits(&device, width, height)?;
    let t_setup = Instant::now();

    let (compute, point_count, mut outcome) =
        fill_points(&device, &queue, &scene, accumulate, control.as_ref(), gpu_timing, chaos_seed)?;
    let mut renderer =
        TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count as f64, sampling, filter,
        filter_radius, bit_depth, clear, transparent, grade,
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
    let use_point_primitives = sampling.use_point_primitives(point_size, base_camera.distance);

    let target = TileTarget::new(
        &device, &queue, width, height, clear, sampling, filter, filter_radius, bit_depth,
    );
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u16; (sheet_w * sheet_h * 4) as usize];

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
            tile.view_proj, sampling.target_height(), point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        // One guard for the whole sheet, off the base framing, for the same
        // reason as the haze band above: grid tiles only re-aim the camera.
        .with_zoom_guard(zoom.as_ref(), base_camera.eye())
        // The near-field size cap is in target pixels and means 12 *output*
        // pixels, so it scales with N alongside `target_height` above.
        .with_supersample(sampling.n);
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
    let grade_save = |record: Option<&RenderRecord>| if let Some(p) = &grade_out {
        // One tile only: a contact sheet has a different linear buffer behind
        // every tile, and there is no honest single file to write. Said out
        // loud rather than silently writing whichever tile happened to be last.
        match &renderer {
            TileRenderer::Splat(r) if tiles.len() == 1 => {
                if let Some(tex) = r.output_texture() {
                    save_grade_buffer(
                        p, &device, &queue, tex, width, height, point_count as f64,
                        sampling.target_height(), clear, transparent, exposure, grade, record,
                    );
                }
            }
            TileRenderer::Splat(_) => eprintln!(
                "warning: --grade-out needs a single tile; a contact sheet has one linear \
                 buffer per tile. Not written."
            ),
            TileRenderer::Points(_) => eprintln!(
                "warning: --grade-out is a splat-renderer feature (there is no linear density \
                 behind the points renderer). Not written."
            ),
        }
    };
    let t_render = Instant::now();

    if let Some(c) = &control {
        c.phase("saving");
    }
    // The camera in the record is the base framing. For a grid sheet that is
    // the framing every tile is derived from, which is the honest answer to
    // "what was this made from"; the per-tile mapping is already on stdout.
    let record = make_record(
        &scene,
        scene_path.as_deref(),
        &base_camera,
        crate::record::Quality {
            width,
            height,
            points: scene.point_count,
            accumulate,
            // No histogram: the sample count is `points`.
            spp: None,
            grade,
            splat,
            exposure,
            transparent,
            supersample: sampling.n,
            filter,
            filter_radius,
            bit_depth,
        },
        threads,
        adapter,
        (t_render - t_start).as_secs_f32(),
    );
    grade_save(record.as_ref());
    save_sheet(out_path, &sheet, sheet_w, sheet_h, bit_depth, record.as_ref())?;
    if let Some(r) = &record {
        match r.write_sidecar(out_path) {
            Ok(p) => println!("Render record: {}", p.display()),
            // The picture exists; a receipt that could not be filed is worth
            // saying out loud but is not a failed render.
            Err(e) => eprintln!("warning: {}", e),
        }
    }
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

/// `--spp` is a still's dial. Every other entry point says so rather than
/// silently rendering at the ring-buffer sample count and letting the flag
/// read as if it had worked.
///
/// An animation is the interesting case: accumulating each frame to a fixed
/// sample count is a perfectly sensible thing to want, and the reason it is not
/// here is cost, not principle — a 600-frame flight at `--spp 500` is 600
/// accumulated stills. It wants its own budget in frames, not a flag inherited
/// from the still path.
fn reject_grade_out(grade_out: Option<std::path::PathBuf>, what: &str) -> Result<(), String> {
    match grade_out {
        None => Ok(()),
        Some(_) => Err(format!(
            "--grade-out saves one still's linear density; {} would need one buffer per frame. \
             Render the frames separately.",
            what
        )),
    }
}

fn reject_spp(spp: Option<u32>, what: &str) -> Result<(), String> {
    match spp {
        None => Ok(()),
        Some(_) => Err(format!(
            "--spp accumulates one still into a persistent histogram; {} would be one full \
             accumulation run per frame. Render the frames separately.",
            what
        )),
    }
}

/// Render a still through the persistent accumulation histogram: the path with
/// no sample ceiling.
///
/// The ordinary path splats the point ring once, so the distinct samples in the
/// image are exactly the ring's capacity however long the chaos game ran. Here
/// the ring is a streaming working set. Each time the chaos game has replaced
/// every point in it — one *lap* — the whole buffer is splatted into a transient
/// texture and folded into a 64-bit fixed-point histogram that outlives it, and
/// the samples are then free to be overwritten. Quality is `--spp`, and the only
/// thing that limits it is time.
///
/// Splatting a whole lap rather than each dispatch's delta is what makes this
/// affordable: the fold is a compute pass over every texel of the accumulation,
/// so it wants to be amortized over a full buffer of points, not over an
/// eightieth of one.
///
/// Deliberately narrower than `render`, and it says so rather than quietly
/// doing something else:
///
/// * **Splat only.** The histogram accumulates linear density for a log
///   tonemap. The plain points renderer is depth-tested and opaque — averaging
///   its output over time is a different operation with a different meaning.
/// * **Single tile only.** Each tile of a contact sheet would need its own full
///   accumulation run, so a 4x2 sheet at `--spp 500` is eight `--spp 500`
///   renders. That is a fine thing to want and a terrible thing to get by
///   accident.
///
/// Cancelling keeps the work: the histogram at lap 400 of 1000 is the same
/// picture as at lap 1000, noisier, and exposure is normalized by the laps
/// actually completed so the brightness is right either way. This is the
/// strongest form of the anytime property this renderer has.
fn render_accumulated(params: OfflineParams) -> Result<Outcome, String> {
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
        // Nothing to label: a single tile is never labelled, and this path
        // renders exactly one.
        labels: _,
        supersample,
        filter,
        filter_radius,
        bit_depth,
        scene_path,
        threads,
        gpu_timing,
        chaos_seed,
        spp,
        grade,
        grade_out,
    } = params;
    let spp = spp.expect("only called with a sample target");
    if !splat {
        return Err(
            "--spp accumulates linear density for the log tonemap, so it needs --splat".into()
        );
    }
    if !matches!(grid, GridMode::Single) {
        return Err(
            "--spp renders one accumulated still; a contact sheet would be one full accumulation \
             run per tile. Render the tiles separately."
                .into(),
        );
    }
    let sampling = Sampling::new(supersample, height);
    let t_start = Instant::now();

    if let Some(f) = view.as_ref().and_then(|v| v.color_falloff) {
        scene.color_falloff = f;
        crate::scene::resolve_color_speeds(&mut scene.transforms, scene.color_speed, f);
    }
    let color_contrast =
        view.as_ref().and_then(|v| v.color_contrast).unwrap_or(scene.color_contrast);
    let clear = crate::scene::clear_color(scene.background, if transparent { 0.0 } else { 1.0 });

    let (device, queue, adapter) = create_device()?;
    sampling.check_fits(&device, width, height)?;
    let t_setup = Instant::now();

    let mut compute = PointCompute::new(
        &device,
        &scene.transforms,
        &scene.colors,
        &scene.colormap,
        scene.point_count as u32,
        chaos_seed,
    );
    compute.zoom = scene_zoom(&scene)?;
    let capacity = compute.buffer_capacity;

    // Laps to reach the target. `--spp` counts against *output* pixels, so
    // turning on supersampling costs N² more fill per lap but does not silently
    // demand N² more laps.
    let target_samples = spp as f64 * width as f64 * height as f64;
    let laps = (target_samples / capacity as f64).ceil().max(1.0) as u32;

    let (base_camera, point_size, haze, folded) =
        base_setup(&view, &scene, haze_enabled, camera_over);
    let zoom = scene_zoom(&scene)?;
    compute.rewrap(&device, &queue, folded);

    let aspect = width as f32 / height as f32;
    let use_point_primitives = sampling.use_point_primitives(point_size, base_camera.distance);

    let mut renderer = SplatRenderer::new(
        &device,
        bit_depth.format(),
        &compute.point_buffer,
        &compute.colormap_buffer,
    );
    // The batch texture has to be the supersampled size, so the renderer is told
    // the factor — but its own filter/tonemap chain is never used here. This
    // path calls `splat_pass` and `tonemap_pass` directly, with the histogram
    // and *its* filter in between.
    renderer.set_supersample(&device, &queue, sampling.n, filter, filter_radius);

    let (haze_near, haze_far) = haze.band(base_camera.distance);
    let camera = CameraUniforms::new(
        base_camera.view_proj(aspect),
        sampling.target_height(),
        point_size,
        aspect,
        1.0,
        haze_near,
        haze_far,
        haze.transmittance,
        haze.saturation,
        color_contrast,
        scene.background.to_array(),
        transparent,
        scene.color_mode.packs_rgb(),
    )
    .with_zoom_guard(zoom.as_ref(), base_camera.eye())
    .with_supersample(sampling.n);
    renderer.upload_camera(&queue, &camera);

    let target = TileTarget::new(
        &device, &queue, width, height, clear, sampling, filter, filter_radius, bit_depth,
    );
    // Sizes the batch texture and hands back the view the histogram reads.
    let (batch_view, accum_w, accum_h) = renderer.prepare_accum(&device, width, height);
    let batch_view = batch_view.clone();
    let histogram =
        Accumulator::new(&device, &queue, width, height, sampling.n, filter, filter_radius)?;
    let batch_bind_group = histogram.bind_batch(&device, &batch_view);
    {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("accum_clear_encoder"),
        });
        histogram.clear(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
    }

    let frames_per_lap = compute.frames_per_lap();
    println!(
        "Accumulating {} laps x {} points ({} samples/px target) into a {}x{} histogram",
        laps, capacity, spp, accum_w, accum_h,
    );
    if accumulate != DEFAULT_ACCUMULATE {
        println!("  (--accumulate is ignored here: --spp sets how long the chaos game runs)");
    }
    if let Some(c) = &control {
        c.phase("accumulating");
        c.log(format!("{} laps of {} points, target {} samples/px", laps, capacity, spp));
    }

    let mut done_laps: u32 = 0;
    let mut outcome = Outcome::Complete;
    let mut timer = gpu_timing
        .then(|| crate::gpu::GpuTimer::new(&device, &queue, GPU_TIMING_SAMPLES))
        .flatten();
    for lap in 0..laps {
        for _ in 0..frames_per_lap {
            compute.advance_fill(&queue);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("accum_chaos_encoder"),
            });
            compute.dispatch_timed(&mut encoder, timer.as_mut());
            queue.submit(std::iter::once(encoder.finish()));
        }
        // Splat the whole ring and fold it in. Both passes go in one encoder:
        // the fold reads exactly what the splat wrote, and queue order inside a
        // submission gives that ordering.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("accum_fold_encoder"),
        });
        renderer.splat_pass(&mut encoder, 0..capacity, use_point_primitives);
        histogram.add(&mut encoder, &batch_bind_group);
        queue.submit(std::iter::once(encoder.finish()));
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        done_laps += 1;

        if let Some(c) = &control {
            c.progress(lap + 1, laps);
            if c.should_stop() {
                c.log(format!(
                    "stopped after {} of {} laps — keeping the {:.0} samples/px accumulated",
                    done_laps,
                    laps,
                    done_laps as f64 * capacity as f64 / (width as f64 * height as f64),
                ));
                outcome = Outcome::Partial;
                break;
            }
        }
    }
    let t_fill = Instant::now();
    if let Some(t) = &timer {
        println!("{}", crate::gpu::timing::summarize("GPU chaos dispatch", &t.read(&device, &queue)));
    }

    // Exposure is normalized by the samples actually accumulated, not the ones
    // asked for. That is what makes a cancelled render come out at the right
    // brightness rather than dark in proportion to how early it stopped — and
    // what makes `--spp` a quality dial rather than an exposure dial.
    let accumulated = done_laps as f64 * capacity as f64;
    renderer.upload_params(
        &queue,
        exposure,
        accumulated,
        sampling.target_height(),
        clear,
        transparent,
        grade,
    );

    let mut sheet = vec![0u16; (width * height * 4) as usize];
    {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("accum_resolve_encoder"),
        });
        // Resolve to output-sized linear density (filtering on the way when
        // supersampling is on), then the ordinary log tonemap.
        let resolved = histogram.resolve(&device, &mut encoder);
        let bind_group = renderer.tonemap_bind_group(&device, resolved);
        renderer.tonemap_pass(&mut encoder, &target.color_view, &target.depth_view, &bind_group);
        target.copy_out(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        target.read_into_sheet(&device, &mut sheet, width, 0, 0);
    }
    let t_render = Instant::now();

    if let Some(c) = &control {
        c.phase("saving");
    }
    let achieved_spp = (accumulated / (width as f64 * height as f64)) as f32;
    let record = make_record(
        &scene,
        scene_path.as_deref(),
        &base_camera,
        crate::record::Quality {
            width,
            height,
            points: scene.point_count,
            accumulate,
            spp: Some(achieved_spp),
            grade,
            splat,
            exposure,
            transparent,
            supersample: sampling.n,
            filter,
            filter_radius,
            bit_depth,
        },
        threads,
        adapter,
        (t_render - t_start).as_secs_f32(),
    );
    if let Some(p) = &grade_out {
        save_grade_buffer(
            p, &device, &queue, histogram.output_texture(), width, height, accumulated,
            sampling.target_height(), clear, transparent, exposure, grade, record.as_ref(),
        );
    }
    save_sheet(out_path, &sheet, width, height, bit_depth, record.as_ref())?;
    if let Some(r) = &record {
        match r.write_sidecar(out_path) {
            Ok(p) => println!("Render record: {}", p.display()),
            Err(e) => eprintln!("warning: {}", e),
        }
    }
    let t_done = Instant::now();

    println!(
        "{} {}x{} ({:.0} samples/px from {} laps of {} points) -> {}",
        if outcome.is_partial() { "Stopped, partial render" } else { "Rendered" },
        width,
        height,
        achieved_spp,
        done_laps,
        capacity,
        out_path.display(),
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
        supersample,
        filter,
        filter_radius,
        // Ignored: both codecs take 8-bit YUV, so the frame path is 8-bit
        // whatever the caller asked for. Named below rather than here so the
        // reason sits next to the frame buffer it governs.
        bit_depth: _,
        scene_path,
        threads,
        gpu_timing,
        chaos_seed,
        spp,
        grade,
        grade_out,
    } = params;
    reject_spp(spp, "an animation")?;
    reject_grade_out(grade_out, "an animation")?;
    // 4:2:0 chroma needs even dimensions
    let (width, height) = (width & !1, height & !1);
    let sampling = Sampling::new(supersample, height);
    // The frame path always renders 8-bit: both codecs take 8-bit 4:2:0 YUV, so
    // a float target would buy nothing and cost an sRGB encode per frame.
    // `render_tile` shares the 16-bit sheet type, so each frame is narrowed
    // back on the way to the encoder — exactly, by 257.
    let bit_depth = BitDepth::Eight;
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

    let (device, queue, adapter) = create_device()?;
    sampling.check_fits(&device, width, height)?;
    let t_setup = Instant::now();

    let (compute, point_count, fill) =
        fill_points(&device, &queue, &scene, accumulate, control.as_ref(), gpu_timing, chaos_seed)?;
    // See this function's doc comment: an animation has nothing to keep from a
    // half-filled cloud, so a cancel here is a cancel.
    if fill.is_partial() {
        return Err(CANCELLED.to_string());
    }
    let mut renderer =
        TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count as f64, sampling, filter,
        filter_radius, bit_depth, clear, transparent, grade,
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
        anim.format, width, height, anim.fps, anim.quality, 8, threads,
    )?;
    println!("Encoding on {} thread{}", threads, if threads == 1 { "" } else { "s" });
    let aspect = width as f32 / height as f32;
    let target = TileTarget::new(
        &device, &queue, width, height, clear, sampling, filter, filter_radius, bit_depth,
    );
    let mut frame_buf = vec![0u16; (width * height * 4) as usize];
    let mut frame_rgba8 = vec![0u8; (width * height * 4) as usize];

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
            cam.view_proj(aspect), sampling.target_height(), point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        // After the wrap too, and for the same reason: the guard is a ramp in
        // multiples of the eye distance, so it has to be rebuilt from the
        // camera that actually renders this frame.
        .with_zoom_guard(zoom.as_ref(), cam.eye())
        .with_supersample(sampling.n);
        renderer.upload_camera(&queue, &camera);
        let use_point_primitives = sampling.use_point_primitives(point_size, cam.distance);
        target.render_tile(
            &device, &queue, &mut renderer, point_count, use_point_primitives,
            &mut frame_buf, width, 0, 0,
        );
        for (o, &v) in frame_rgba8.iter_mut().zip(&frame_buf) {
            *o = (v / 257) as u8;
        }
        encoder.push_frame(&frame_rgba8)?;
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
    // Sidecar only. The muxer here emits `ftyp`/`mdat`/`moov` and nothing else;
    // adding a `udta`/`meta` box is real work in a format this project already
    // treats gingerly for upload-pipeline compatibility, so an animation gets
    // the same record beside it rather than inside it.
    if let Some(r) = make_record(
        &scene,
        scene_path.as_deref(),
        &base_camera,
        crate::record::Quality {
            width,
            height,
            points: scene.point_count,
            accumulate,
            // No histogram: the sample count is `points`.
            spp: None,
            grade,
            splat,
            exposure,
            transparent,
            supersample: sampling.n,
            filter,
            filter_radius,
            bit_depth,
        },
        threads,
        adapter,
        (t_render - t_start).as_secs_f32(),
    ) {
        match r.write_sidecar(out_path) {
            Ok(p) => println!("Render record: {}", p.display()),
            Err(e) => eprintln!("warning: {}", e),
        }
    }
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
        supersample,
        filter,
        filter_radius,
        bit_depth,
        // Unused here: a sheet of many scenes gets no render record, so there
        // is no `[source]` to name and no `[machine]` to report. See the
        // `save_sheet(.., None)` call below.
        scene_path: _,
        threads: _,
        gpu_timing,
        chaos_seed,
        spp,
        grade,
        grade_out,
    } = params;
    reject_spp(spp, "a variant sheet")?;
    reject_grade_out(grade_out, "a variant sheet")?;
    let sampling = Sampling::new(supersample, height);
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

    let (device, queue, _adapter) = create_device()?;
    sampling.check_fits(&device, width, height)?;
    let t_setup = Instant::now();

    let (base_camera, point_size, haze, _folded) = base_setup(&view, &scene, haze_enabled, camera_over);
    let aspect = width as f32 / height as f32;
    let view_proj = base_camera.view_proj(aspect);
    let use_point_primitives = sampling.use_point_primitives(point_size, base_camera.distance);
    let (haze_near, haze_far) = haze.band(base_camera.distance);
    // One framing and one haze band for every tile; only the edge guard is
    // per-variant, because mutating the transforms moves the zoom's fixed
    // point and a guard aimed at the wrong centre would fade the wrong side
    // of the picture. A variant whose zoom no longer resolves simply goes
    // unguarded — the tile is a still, and the sheet is worth more than the
    // one tile is.
    let camera = |zoom: Option<&crate::renorm::Renorm>| {
        CameraUniforms::new(
            view_proj, sampling.target_height(), point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        .with_zoom_guard(zoom, base_camera.eye())
        .with_supersample(sampling.n)
    };

    let target = TileTarget::new(
        &device, &queue, width, height, clear, sampling, filter, filter_radius, bit_depth,
    );
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u16; (sheet_w * sheet_h * 4) as usize];

    let out_stem = out_path.with_extension("");
    remove_stale_variants(&out_stem);
    let mut fill_total = 0.0f32;
    let mut outcome = Outcome::Complete;
    let mut done = 0usize;
    for (idx, (variant, label)) in variants.iter().enumerate() {
        let t0 = Instant::now();
        // Each variant is a different IFS: refill the point buffer
        let (compute, point_count, fill) =
            fill_points(&device, &queue, variant, accumulate, control.as_ref(), gpu_timing, chaos_seed)?;
        // A tile cut off mid-fill is still a tile — sparser than its
        // neighbours, which is exactly what a sheet read tile-against-tile must
        // not hide, so the whole sheet is reported partial.
        outcome = outcome.and(fill);
        let mut renderer =
            TileRenderer::new(
        &device, &queue, &compute, splat, exposure, point_count as f64, sampling, filter,
        filter_radius, bit_depth, clear, transparent, grade,
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

    // No record: a mutation or sweep sheet is many different scenes, and a
    // `fracturize:scene` chunk claiming one of them would be a confident lie
    // about the other tiles. The per-tile mapping is printed, and mutation
    // variants are already written out as `<out>.mutN.toml`.
    save_sheet(out_path, &sheet, sheet_w, sheet_h, bit_depth, None)?;
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
        supersample,
        filter,
        filter_radius,
        bit_depth,
        // Unused here: a sheet of many scenes gets no render record, so there
        // is no `[source]` to name and no `[machine]` to report. See the
        // `save_sheet(.., None)` call below.
        scene_path: _,
        threads: _,
        gpu_timing,
        chaos_seed,
        spp,
        grade,
        grade_out,
    } = params;
    reject_spp(spp, "a sweep sheet")?;
    reject_grade_out(grade_out, "a sweep sheet")?;
    let sampling = Sampling::new(supersample, height);
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

    let (device, queue, _adapter) = create_device()?;
    sampling.check_fits(&device, width, height)?;
    let t_setup = Instant::now();

    // One framing and one haze band for the whole sheet: a contact sheet is
    // read tile against tile, so only the swept parameter may differ.
    let (base_camera, point_size, haze, _folded) = base_setup(&view, &scene, haze_enabled, camera_over);
    let aspect = width as f32 / height as f32;
    let view_proj = base_camera.view_proj(aspect);
    let use_point_primitives = sampling.use_point_primitives(point_size, base_camera.distance);
    let (haze_near, haze_far) = haze.band(base_camera.distance);
    // Per-variant edge guard: a sweep may be sweeping a zoom parameter, or
    // anything that moves the renormalizing map's fixed point.
    let camera = |zoom: Option<&crate::renorm::Renorm>| {
        CameraUniforms::new(
            view_proj, sampling.target_height(), point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            color_contrast, scene.background.to_array(),
            transparent, scene.color_mode.packs_rgb(),
        )
        .with_zoom_guard(zoom, base_camera.eye())
        .with_supersample(sampling.n)
    };

    let target = TileTarget::new(
        &device, &queue, width, height, clear, sampling, filter, filter_radius, bit_depth,
    );
    let sheet_w = width * cols;
    let sheet_h = height * rows;
    let mut sheet = vec![0u16; (sheet_w * sheet_h * 4) as usize];

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
            fill_points(&device, &queue, variant, accumulate, control.as_ref(), gpu_timing, chaos_seed)?;
        outcome = outcome.and(fill);
        let mut renderer = TileRenderer::new(
            &device, &queue, &compute, splat, exposure, point_count as f64, sampling, filter,
            filter_radius, bit_depth, clear, transparent, grade,
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
    // No record: a mutation or sweep sheet is many different scenes, and a
    // `fracturize:scene` chunk claiming one of them would be a confident lie
    // about the other tiles. The per-tile mapping is printed, and mutation
    // variants are already written out as `<out>.mutN.toml`.
    save_sheet(out_path, &sheet, sheet_w, sheet_h, bit_depth, None)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Exhaustive rather than sampled: there are only 65536 binary16 values, so
    /// "tested" can mean every one of them. Checked against the same decode
    /// expressed independently — build an f32 whose bit pattern comes from the
    /// IEEE definition directly.
    #[test]
    fn f16_decodes_every_bit_pattern() {
        for bits in 0u16..=u16::MAX {
            let got = f16_to_f32(bits);
            let want = reference_f16(bits);
            if want.is_nan() {
                assert!(got.is_nan(), "0x{:04x}: expected NaN, got {}", bits, got);
            } else {
                assert_eq!(got.to_bits(), want.to_bits(), "0x{:04x}", bits);
            }
        }
    }

    /// binary16 straight from its definition: sign, a biased 5-bit exponent and
    /// a 10-bit significand, with the exponent extremes reserved.
    fn reference_f16(bits: u16) -> f32 {
        let sign = if bits >> 15 == 1 { -1.0f32 } else { 1.0 };
        let exp = ((bits >> 10) & 0x1F) as i32;
        let mant = (bits & 0x3FF) as f32;
        match exp {
            0 => sign * mant / 1024.0 * 2.0f32.powi(-14),
            0x1F if mant == 0.0 => sign * f32::INFINITY,
            0x1F => f32::NAN,
            _ => sign * (1.0 + mant / 1024.0) * 2.0f32.powi(exp - 15),
        }
    }

    /// The two constants of the sRGB piecewise curve, plus its endpoints. The
    /// 8-bit path gets this from the hardware; the 16-bit path has to match it,
    /// or the same render at two depths would not be the same picture.
    #[test]
    fn srgb_encode_matches_the_standard() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        // In f32 the curve comes to 0.99999994 at 1.0 rather than exactly 1.
        // What matters is the code that reaches the file, so that is what is
        // asserted: white is white, and black is black.
        let code = |v: f32| (linear_to_srgb(v) * 65535.0 + 0.5) as u16;
        assert_eq!(code(1.0), u16::MAX);
        assert_eq!(code(0.0), 0);
        assert_eq!(code(2.0), u16::MAX);
        assert_eq!(code(-1.0), 0);
        // The piece boundary is continuous
        let lo = 0.0031308 * 12.92;
        assert!((linear_to_srgb(0.0031308) - lo).abs() < 1e-6);
        // Mid grey: 0.2140 linear is ~0.5 encoded
        assert!((linear_to_srgb(0.2140) - 0.5).abs() < 2e-3, "{}", linear_to_srgb(0.2140));
        // Out of range is clamped rather than producing a NaN from powf
        assert_eq!(linear_to_srgb(-1.0), 0.0);
        assert!(linear_to_srgb(2.0) <= 1.0);
    }

    /// A 16-bit sheet holding 8-bit data narrows back exactly, which is what
    /// lets one sheet type serve both depths without changing 8-bit output.
    #[test]
    fn the_sheet_round_trips_eight_bit_data() {
        for v in 0u8..=255 {
            let wide = v as u16 * 257;
            assert_eq!((wide / 257) as u8, v);
        }
    }

    /// The record is only worth writing if it can be read back. Writes a real
    /// PNG through the real encoder and decodes it with the real decoder —
    /// nothing here is mocked, because what this guards is the interaction
    /// between the two.
    #[test]
    fn a_saved_png_carries_its_record_and_still_decodes() {
        let dir = std::env::temp_dir().join(format!("fracturize-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let (w, h) = (3u32, 2u32);
        let sheet = vec![0u16; (w * h * 4) as usize];

        let record = RenderRecord {
            scene_path: Some(std::path::PathBuf::from("scenes/x.toml")),
            // Non-ASCII on purpose: `tEXt` is Latin-1 and would mangle this,
            // which is why the chunks are `iTXt`.
            scene_toml: "[meta]\nname = \"Ωmega — α\"\n".to_string(),
            camera: OrbitCamera::from_chart(0.1, 0.2, 0.0, 3.0, Vec3::ZERO),
            quality: crate::record::Quality {
                width: w,
                height: h,
                points: 1000,
                accumulate: 4,
                spp: None,
                grade: Grade::NEUTRAL,
                splat: true,
                exposure: 1.0,
                transparent: false,
                supersample: 2,
                filter: Filter::Gaussian,
                filter_radius: 0.5,
                bit_depth: BitDepth::Eight,
            },
            machine: Default::default(),
            created: "2026-08-12T00:00:00Z".to_string(),
        };
        save_sheet(&path, &sheet, w, h, BitDepth::Eight, Some(&record)).unwrap();

        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        // The picture is intact: metadata must not cost the image.
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (w, h));

        let text = &reader.info().utf8_text;
        let get = |k: &str| {
            text.iter()
                .find(|c| c.keyword == k)
                .map(|c| {
                    let mut c = c.clone();
                    c.decompress_text().unwrap();
                    c.get_text().unwrap()
                })
                .unwrap_or_else(|| panic!("chunk {:?} missing", k))
        };
        assert_eq!(get(crate::record::KEY_SCENE), record.scene_toml);
        assert_eq!(get(crate::record::KEY_SCENE_SHA), record.scene_sha256());
        assert!(get("Software").contains(crate::version::VERSION));
        // The record parses as TOML straight out of the file
        let v: toml::Value = toml::from_str(&get(crate::record::KEY_RENDER)).unwrap();
        assert_eq!(v["render"]["supersample"].as_integer(), Some(2));

        // And a render with no record still writes a perfectly good PNG
        let plain = dir.join("plain.png");
        save_sheet(&plain, &sheet, w, h, BitDepth::Eight, None).unwrap();
        let mut r2 = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&plain).unwrap())).read_info().unwrap();
        let mut b2 = vec![0; r2.output_buffer_size().unwrap()];
        assert_eq!(r2.next_frame(&mut b2).unwrap().width, w);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sweep says what it was asked for: both ends land on the sheet.
    ///
    /// The alternative — `from + range * i/steps` — puts the last tile short
    /// of `to`, so `1:4` in four steps would top out at 3.25 and the sheet
    /// would quietly not contain the value in its own filename.
    #[test]
    fn a_sweep_includes_both_ends() {
        let (from, to, steps) = (1.0f32, 4.0f32, 4usize);
        let at = |i: usize| from + (to - from) * (i as f32 / (steps - 1) as f32);
        assert_eq!(at(0), 1.0);
        assert_eq!(at(steps - 1), 4.0);
    }

    /// One tile is one tile, and n tiles get as square a sheet as they can.
    #[test]
    fn the_sweep_sheet_is_as_square_as_it_can_be() {
        assert_eq!(grid_shape(1), (1, 1));
        assert_eq!(grid_shape(4), (2, 2));
        assert_eq!(grid_shape(9), (3, 3));
        // Never fewer cells than tiles, or a tile would have nowhere to go.
        for n in 1..40usize {
            let (c, r) = grid_shape(n);
            assert!((c * r) as usize >= n, "{} tiles do not fit {}x{}", n, c, r);
        }
    }

    /// Each axis has to move its own knob and nothing else — the sweep loop
    /// reuses one `Grade` per tile, so a stray write would leak across tiles.
    #[test]
    fn each_grade_axis_moves_only_its_own_knob() {
        use crate::gpu::points::splat::Grade;
        for axis in [
            GradeAxis::Exposure,
            GradeAxis::Gamma,
            GradeAxis::GammaThreshold,
            GradeAxis::Vibrancy,
        ] {
            let (mut e, mut g) = (1.0f32, Grade::NEUTRAL);
            axis.apply(0.5, &mut e, &mut g);
            let moved = (e != 1.0) as u8
                + (g.gamma != Grade::NEUTRAL.gamma) as u8
                + (g.gamma_threshold != Grade::NEUTRAL.gamma_threshold) as u8
                + (g.vibrancy != Grade::NEUTRAL.vibrancy) as u8;
            assert_eq!(moved, 1, "{:?} moved {} knobs, not 1", axis, moved);
        }
    }

    /// Every axis is named on the command line and printed back onto the
    /// sheet's tiles, and the two spellings differ on purpose: clap kebab-cases
    /// to match the `--gamma-threshold` flag, while `label` is the TOML key the
    /// record uses. So this checks they agree *modulo the separator*, which
    /// still catches a typo in either without forcing a false unification.
    #[test]
    fn grade_axis_labels_match_their_cli_names() {
        use clap::ValueEnum;
        for a in GradeAxis::value_variants() {
            let cli = a.to_possible_value().unwrap().get_name().replace('-', "_");
            assert_eq!(cli, a.label(), "{:?}", a);
        }
    }

    /// The three things measured in render-target pixels have to agree, and
    /// `Sampling` is the single place that decides them.
    #[test]
    fn sampling_measures_everything_against_the_accumulation() {
        let one = Sampling::new(1, 600);
        let four = Sampling::new(4, 600);
        assert_eq!(one.target_height(), 600.0);
        assert_eq!(four.target_height(), 2400.0);
        // A point subpixel at output but not at 4x accumulation resolution
        // must leave the unfiltered 1px path — that is the whole feature.
        let point_size = 1.5 / 600.0 * 0.9;
        assert!(one.use_point_primitives(point_size, 1.0));
        assert!(!four.use_point_primitives(point_size, 1.0));
        // 0 is coerced to 1 rather than dividing by nothing
        assert_eq!(Sampling::new(0, 600).target_height(), 600.0);
    }
}
