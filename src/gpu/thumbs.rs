//! Scene thumbnails: a small splat render of a scene, from the scene's own
//! camera, for the scene browser.
//!
//! Nothing here is a second renderer. A thumbnail is a [`PointCompute`] with
//! 256 walkers instead of 16,384 and a ring of [`POINTS`], drawn by an ordinary
//! [`SplatRenderer`] into a texture egui can show, and framed by the same
//! `offline::frame_setup` a `--render` uses. Both GPU objects are built on
//! pipelines compiled once ([`ThumbPipelines`]), because a compile is ~6 ms and
//! a folder is dozens of scenes.
//!
//! **A thumbnail converges once and is then a still.** The chaos game runs as
//! two submits the moment it is built — a burn-in, then exactly one lap of the
//! ring — and after that the texture is kept and nothing is re-run. Only the
//! entry being hovered re-renders, as it flies its camera path; the ring stays
//! put and the camera moves through it, which is one splat a frame.
//!
//! The burn-in is not optional. Walkers start at random positions and every
//! iteration writes a point, so the first few land nowhere near the attractor.
//! An interactive ring hides that by turning over for thirteen seconds; a
//! thumbnail has one lap, so it spends its first iterations off the books.
//! They are a separate submit because the ring is walker-major — one dispatch
//! writing more than a lap lets one walker's burn-in land on another walker's
//! finished points.
//!
//! Measured on the GTX 1080 at 96x96 and 8k points (see
//! `renders/thumb-study/`): shape and colour read from 8k points on nearly
//! every scene, where 2k reads only on compact ones; 2x supersampling isn't
//! worth its fill at this size.

use crate::camera::{CameraOverride, OrbitCamera};
use crate::gpu::buffers::CameraUniforms;
use crate::gpu::points::compute::{ChaosPipelines, LayerTable, PointCompute, DEFAULT_SEED};
use crate::gpu::points::splat::{Grade, SplatPipelines, SplatRenderer};
use crate::gpu::DEPTH_FORMAT;
use crate::scene::Scene;

/// The thumbnail's texture format: what the splat renders into, so the
/// hardware does the sRGB encode on store as it does for a `--render`.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// What egui samples the same texture as. Not `FORMAT`, and this was a real
/// bug: egui's shader treats every texel it samples as sRGB-*encoded* (its own
/// textures are `Rgba8Unorm`), so handing it the sRGB view decoded the texel
/// to linear on the way in and egui then read linear as encoded — every
/// thumbnail came out darker than the scene opens, a linear 0.315 background
/// showing as 80/255 instead of 152. Viewing the same bytes as plain unorm
/// hands egui exactly the encoded values the sRGB store wrote.
const EGUI_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Points in a thumbnail's ring.
pub const POINTS: u32 = 8192;

/// One workgroup: 256 walkers, so a lap is 32 iterations each.
const WORKGROUPS: u32 = 1;

/// Iterations each walker spends converging before it writes a point that
/// counts. A contraction of 0.5 per step is 1e-6 of the start after 20.
const BURN_IN: u32 = 20;

/// The compiled pipelines every thumbnail on one device shares.
pub struct ThumbPipelines {
    chaos: ChaosPipelines,
    splat: SplatPipelines,
}

impl ThumbPipelines {
    /// `chaos` is shared with whatever else runs the chaos game on this
    /// device; the splat pipelines are the thumbnails' own, being for their
    /// texture format rather than the window's.
    pub fn new(device: &wgpu::Device, chaos: &ChaosPipelines) -> Self {
        Self { chaos: chaos.clone(), splat: SplatPipelines::new(device, FORMAT) }
    }
}

pub struct Thumbnail {
    scene: Scene,
    compute: PointCompute,
    splat: SplatRenderer,
    /// Only read back by the tests; the view is what egui draws.
    #[cfg_attr(not(test), allow(dead_code))]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The same texture as egui samples it; see [`EGUI_FORMAT`].
    egui_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    width: u32,
    height: u32,
    /// How long one flight of the camera path takes, in seconds: the authored
    /// path's, else the default orbit's. The hovered entry plays at this rate.
    seconds: f32,
    /// Zoom periods the ring has been carried by, so a flight that crosses a
    /// wrap carries the points with the camera — by the change in fold depth,
    /// never by the depth itself (see "Infinite Zoom" in AGENTS.md).
    folded: i32,
}

impl Thumbnail {
    /// Build, converge and draw a thumbnail of `scene` from its own camera.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipes: &ThumbPipelines,
        scene: Scene,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let table = LayerTable::build(&scene, &[], scene.color_contrast);
        let mut compute = PointCompute::with_pipelines(
            device, &pipes.chaos, &table, POINTS, DEFAULT_SEED, WORKGROUPS,
        );
        compute.zoom = crate::offline::scene_zoom(&scene)?;

        // Burn in, then one lap. Two submits, because `advance_with_iters`
        // writes the params through the queue and both writes would otherwise
        // land before either dispatch runs.
        let lap = POINTS.div_ceil(compute.num_walkers);
        for iters in [BURN_IN, lap] {
            compute.advance_with_iters(queue, iters);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("thumb_chaos_encoder"),
            });
            compute.dispatch(&mut encoder);
            queue.submit(std::iter::once(encoder.finish()));
        }

        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("thumb_color"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[EGUI_FORMAT],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("thumb_depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let egui_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(EGUI_FORMAT),
            ..Default::default()
        });
        let depth_view = depth.create_view(&Default::default());

        let splat = SplatRenderer::with_pipelines(
            device, &pipes.splat, &compute.point_buffer, &compute.colormap_buffer,
        );
        splat.upload_params(
            queue,
            scene.exposure,
            POINTS as f64,
            height as f32,
            crate::scene::clear_color(scene.background, 1.0),
            crate::gpu::buffers::Alpha::Opaque,
            Grade::default(),
        );

        let default = crate::path::CameraPath::full_orbit(&scene.camera());
        let seconds = crate::path::resolve(scene.camera_path.as_ref(), &default).duration();

        let mut thumb = Self {
            scene, compute, splat, texture, view, egui_view, depth_view, width, height, seconds, folded: 0,
        };
        thumb.render(device, queue, None);
        Ok(thumb)
    }

    /// Redraw from the scene's own camera, or from `path_t` (0-1) along its
    /// camera path — the default orbit when it authors none. The points are
    /// not regenerated; this is one splat.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path_t: Option<f32>) {
        let over = CameraOverride { path_t, ..Default::default() };
        let (camera, point_size, haze, folded, _) =
            crate::offline::frame_setup(&None, &self.scene, false, over);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("thumb_render_encoder"),
        });
        if folded != self.folded {
            self.compute.rewrap_in(queue, &mut encoder, folded - self.folded);
            self.folded = folded;
        }

        let aspect = self.width as f32 / self.height as f32;
        let (haze_near, haze_far) = haze.band(camera.distance);
        let uniforms = CameraUniforms::new(
            camera.view_proj(aspect), camera.fov, self.height as f32, point_size, aspect, 1.0,
            haze_near, haze_far, haze.transmittance, haze.saturation,
            self.scene.color_contrast, self.scene.background.to_array(),
            false, self.scene.packs_rgb(),
        )
        .with_zoom_guard(self.compute.zoom.as_ref(), camera.eye());
        self.splat.upload_camera(queue, &uniforms);

        let primitives =
            OrbitCamera::point_pixels(point_size, camera.distance, self.height as f32, camera.fov) <= 1.5;
        self.splat.render(
            device, &mut encoder, &self.view, &self.depth_view,
            self.width, self.height, POINTS, primitives,
        );
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// What egui registers and draws.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.egui_view
    }
}

/// What a row gets back when it asks for its thumbnail.
pub enum ThumbState<'a> {
    Ready(egui::TextureId),
    /// Asked for; drawn by the next frame.
    Pending,
    Failed(&'a str),
}

/// What a thumbnail is of: a scene file, or a roll in the Random flames list
/// (by its id there, which is never reused).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ThumbKey {
    File(std::path::PathBuf),
    Random(u64),
}

struct Slot {
    modified: Option<std::time::SystemTime>,
    /// `(width, height)` it was drawn at; a different size is a rebuild.
    size: (u32, u32),
    built: Result<(Thumbnail, egui::TextureId), String>,
    /// `ThumbCache::frame` when a row last asked for it, for eviction.
    used: u64,
}

/// Thumbnails by file or roll: asked for by the rows of the scene browser and
/// the Random flames list while they draw, built
/// by [`ThumbCache::service`] at the top of the next render, and kept.
///
/// Split that way because the rows are drawn inside egui's UI pass, which has
/// neither the device nor egui's renderer to register a texture with; the
/// render has both. A new thumbnail therefore appears one frame after its row
/// does, which nobody can see.
///
/// A file's thumbnail is rebuilt when its modified time changes, so a scene
/// saved in another window, or a run landing in `scenes/local-llm/`, is
/// current the next time its row is drawn.
pub struct ThumbCache {
    pipes: ThumbPipelines,
    slots: std::collections::HashMap<ThumbKey, Slot>,
    wanted: Vec<(ThumbKey, Option<std::time::SystemTime>, (u32, u32), Scene)>,
    /// The entry under the pointer, flying its path, and since when.
    hovered: Option<(ThumbKey, std::time::Instant)>,
    /// What this frame's lists reported under the pointer (see [`Self::hover`]).
    hover_now: Option<ThumbKey>,
    /// One that stopped being hovered and needs putting back on its still.
    settle: Option<ThumbKey>,
    frame: u64,
}

/// Thumbnails kept at most. About 0.4 MB of VRAM each at 128x72, so this is
/// ~40 MB — and a folder bigger than this re-renders rows as they scroll back
/// into view, at under a millisecond each.
const MAX_SLOTS: usize = 96;

/// Build time spent per frame, past the first thumbnail. At ~0.7 ms apiece
/// this is a screenful per frame without costing the live view a frame.
const BUILD_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

impl ThumbCache {
    pub fn new(device: &wgpu::Device, chaos: &ChaosPipelines) -> Self {
        Self {
            pipes: ThumbPipelines::new(device, chaos),
            slots: Default::default(),
            wanted: Vec::new(),
            hovered: None,
            hover_now: None,
            settle: None,
            frame: 0,
        }
    }

    /// The thumbnail for `key` at `size` pixels, asking for it if it isn't
    /// built (or is stale). `scene` is only cloned when it is.
    pub fn get(
        &mut self,
        key: &ThumbKey,
        modified: Option<std::time::SystemTime>,
        size: (u32, u32),
        scene: &Scene,
    ) -> ThumbState<'_> {
        let fresh = self
            .slots
            .get(key)
            .is_some_and(|s| s.modified == modified && s.size == size);
        if !fresh {
            if !self.wanted.iter().any(|(k, ..)| k == key) {
                self.wanted.push((key.clone(), modified, size, scene.clone()));
            }
            return ThumbState::Pending;
        }
        let slot = self.slots.get_mut(key).expect("checked above");
        slot.used = self.frame;
        match &slot.built {
            Ok((_, id)) => ThumbState::Ready(*id),
            Err(e) => ThumbState::Failed(e),
        }
    }

    /// Report the entry under the pointer, this frame. It flies its camera
    /// path; the rest stay still. A list with nothing hovered says nothing,
    /// so two lists drawing in one frame can't clear each other's hover —
    /// [`Self::service`] takes what was reported and treats silence as none.
    pub fn hover(&mut self, key: ThumbKey) {
        self.hover_now = Some(key);
    }

    fn update_hovered(&mut self) {
        let now = self.hover_now.take();
        if self.hovered.as_ref().map(|(k, _)| k) == now.as_ref() {
            return;
        }
        if let Some((old, _)) = self.hovered.take() {
            self.settle = Some(old);
        }
        self.hovered = now.map(|k| (k, std::time::Instant::now()));
    }

    /// Build what was asked for, fly the hovered entry, settle the one that
    /// was. Called once a frame, before egui paints.
    pub fn service(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
    ) {
        self.frame += 1;
        self.update_hovered();
        let start = std::time::Instant::now();
        let mut built = 0;
        while !self.wanted.is_empty() && (built == 0 || start.elapsed() < BUILD_BUDGET) {
            let (key, modified, size, scene) = self.wanted.remove(0);
            let built_thumb = Thumbnail::new(device, queue, &self.pipes, scene, size.0, size.1)
                .map(|t| {
                    let id = egui.register_native_texture(device, t.view(), wgpu::FilterMode::Linear);
                    (t, id)
                });
            if let Some(Slot { built: Ok((_, id)), .. }) = self.slots.insert(
                key,
                Slot { modified, size, built: built_thumb, used: self.frame },
            ) {
                egui.free_texture(&id);
            }
            built += 1;
        }
        // Anything asked for and not reached is asked for again next frame by
        // its row, if it is still on screen; a row scrolled away shouldn't
        // keep its place in the queue.
        self.wanted.clear();

        if let Some(key) = self.settle.take()
            && let Some(Slot { built: Ok((t, _)), .. }) = self.slots.get_mut(&key)
        {
            t.render(device, queue, None);
        }
        if let Some((key, since)) = &self.hovered
            && let Some(Slot { built: Ok((t, _)), .. }) = self.slots.get_mut(key)
        {
            let t_path = (since.elapsed().as_secs_f32() / t.seconds.max(0.1)).fract();
            t.render(device, queue, Some(t_path));
        }

        if self.slots.len() > MAX_SLOTS {
            let mut by_age: Vec<(u64, ThumbKey)> =
                self.slots.iter().map(|(k, s)| (s.used, k.clone())).collect();
            by_age.sort_by_key(|(used, _)| *used);
            for (_, key) in by_age.into_iter().take(self.slots.len() - MAX_SLOTS) {
                if let Some(Slot { built: Ok((_, id)), .. }) = self.slots.remove(&key) {
                    egui.free_texture(&id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn wait(device: &wgpu::Device) {
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
    }

    fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, t: &Thumbnail) -> image::RgbaImage {
        let row = (t.width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * t.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            t.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        );
        queue.submit(std::iter::once(encoder.finish()));
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        wait(device);
        let data = buffer.slice(..).get_mapped_range();
        let mut img = image::RgbaImage::new(t.width, t.height);
        for y in 0..t.height {
            let src = &data[(y * row) as usize..][..(t.width * 4) as usize];
            img.as_mut()[(y * t.width * 4) as usize..][..src.len()].copy_from_slice(src);
        }
        img
    }

    /// Not a correctness test: the measurement behind the module docs, kept
    /// runnable. `cargo test --release thumbnail_study -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a GPU; writes renders/thumb-study/live-*.png"]
    fn thumbnail_study() {
        let (device, queue, adapter) = crate::offline::create_device().unwrap();
        println!("adapter: {adapter}");
        let t = Instant::now();
        let pipes = ThumbPipelines::new(&device, &ChaosPipelines::new(&device));
        wait(&device);
        println!("pipelines: {:.2} ms", t.elapsed().as_secs_f64() * 1e3);

        std::fs::create_dir_all("renders/thumb-study").unwrap();
        let names = [
            "ammonite", "censer", "fern_3d", "galaxy", "glasshouse", "lacewing", "reliquary",
            "jellyfish",
        ];
        let mut thumbs = Vec::new();
        let all = Instant::now();
        for name in names {
            let t = Instant::now();
            let scene = Scene::load(format!("scenes/{name}.toml")).unwrap();
            let parsed = t.elapsed();
            let thumb = Thumbnail::new(&device, &queue, &pipes, scene, 96, 96).unwrap();
            wait(&device);
            println!(
                "{name:12} parse {:.2} ms, build+converge+draw {:.2} ms",
                parsed.as_secs_f64() * 1e3,
                (t.elapsed() - parsed).as_secs_f64() * 1e3,
            );
            thumbs.push((name, thumb));
        }
        println!("all eight: {:.2} ms", all.elapsed().as_secs_f64() * 1e3);

        for (name, thumb) in &thumbs {
            read_back(&device, &queue, thumb)
                .save(format!("renders/thumb-study/live-{name}.png"))
                .unwrap();
        }

        // The hovered entry: one re-splat a frame along its path.
        let (name, thumb) = &mut thumbs[0];
        let t = Instant::now();
        for i in 0..120 {
            thumb.render(&device, &queue, Some(i as f32 / 120.0));
        }
        wait(&device);
        println!("{name}: 120 hover frames {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
    }
}
