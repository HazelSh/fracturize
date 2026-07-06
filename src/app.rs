use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use winit::window::Window;

use crate::gpu::{CameraUniforms, GizmoRenderer, GpuContext, PointCompute, PointRenderer, TextRenderer, DEPTH_FORMAT};
use crate::gpu::text::TextEntry;
use crate::scene::{Scene, TransformSpec};

/// Project a world-space position to screen coordinates
fn world_to_screen(pos: Vec3, view_proj: Mat4, w: f32, h: f32) -> Option<(f32, f32)> {
    let clip = view_proj * pos.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(((ndc.x * 0.5 + 0.5) * w, (1.0 - (ndc.y * 0.5 + 0.5)) * h))
}

/// Clear color: dark blue-black
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.02,
    b: 0.05,
    a: 1.0,
};

/// Screenshot dimensions
const SCREENSHOT_WIDTH: u32 = 1280;
const SCREENSHOT_HEIGHT: u32 = 720;

/// Create a depth texture
fn create_depth_texture(device: &wgpu::Device, width: u32, height: u32, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// FPS tracking
pub struct FpsTracker {
    last_log_time: Instant,
    frames_since_log: u32,
    last_frame_time: Instant,
    frame_times: Vec<Duration>,
    pub current_fps: f32,
    pub current_frametime_ms: f32,
    pub should_update_display: bool,
}

impl FpsTracker {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            last_log_time: now,
            frames_since_log: 0,
            last_frame_time: now,
            frame_times: Vec::with_capacity(120),
            current_fps: 0.0,
            current_frametime_ms: 0.0,
            should_update_display: false,
        }
    }

    fn frame(&mut self) -> bool {
        let now = Instant::now();
        let frame_time = now.duration_since(self.last_frame_time);
        self.last_frame_time = now;
        self.frames_since_log += 1;
        self.should_update_display = false;

        self.frame_times.push(frame_time);
        if self.frame_times.len() > 60 {
            self.frame_times.remove(0);
        }

        if !self.frame_times.is_empty() {
            let total: Duration = self.frame_times.iter().sum();
            self.current_frametime_ms = total.as_secs_f32() * 1000.0 / self.frame_times.len() as f32;
            self.current_fps = self.frame_times.len() as f32 / total.as_secs_f32();
        }

        let elapsed = now.duration_since(self.last_log_time);
        if elapsed >= Duration::from_secs(1) {
            self.should_update_display = true;
            self.last_log_time = now;
            self.frames_since_log = 0;
        }

        self.should_update_display
    }
}

/// Main application state
pub struct App {
    pub gpu: GpuContext,
    pub window: Arc<Window>,
    pub frame_count: u32,
    pub rotation: f32,
    pub camera_distance: f32,
    pub camera_focus: Vec3,
    pub camera_offset: Vec3,
    pub point_size: f32,

    pub show_gizmos: bool,
    pub show_text: bool,
    pub show_help: bool,

    // Fog parameters
    pub fog_near: f32,
    pub fog_far: f32,
    pub fog_brightness: f32,
    pub fog_saturation: f32,

    fps_tracker: FpsTracker,

    // Simple point rendering pipeline
    point_compute: PointCompute,
    point_renderer: PointRenderer,
    gizmo_renderer: GizmoRenderer,
    text_renderer: TextRenderer,

    /// Scene name for HUD
    scene_name: String,
    /// Scene author for HUD
    scene_author: String,
    /// Point buffer capacity for HUD
    buffer_capacity: u32,

    /// Transform names for text overlay
    transform_names: Vec<Option<String>>,
    /// Cached scene transforms for text label projection
    scene_transforms: Vec<TransformSpec>,
    /// Colormap for transform label colors
    colormap: [[f32; 4]; 256],

    /// Selected transform index (Some when text overlay visible)
    selected_transform: Option<usize>,
    /// Per-transform enabled state
    transform_enabled: Vec<bool>,
    /// Original weights stashed from scene for restore
    #[allow(dead_code)]
    original_weights: Vec<f32>,

    depth_texture: wgpu::Texture,

    screenshot_texture: wgpu::Texture,
    screenshot_depth: wgpu::Texture,
    screenshot_buffer: wgpu::Buffer,
    pub pending_screenshot: bool,
}

impl App {
    /// Create a new App
    pub async fn new(window: Arc<Window>, scene: Scene, fog_enabled: bool, vsync: bool) -> Self {
        let gpu = GpuContext::new(window.clone(), vsync).await;

        log::info!("Loaded scene: {} by {}", scene.name, scene.author);

        let point_size = scene.point_size;
        log::info!("Point size from scene: {}", point_size);

        let buffer_capacity = scene.point_count as u32;
        log::info!("Point buffer capacity: {}", buffer_capacity);

        // Create point compute pipeline
        let point_compute = PointCompute::new(
            &gpu.device,
            &scene.transforms,
            &scene.colormap,
            buffer_capacity,
        );

        // Create point renderer
        let point_renderer = PointRenderer::new(
            &gpu.device,
            gpu.format,
            &point_compute.point_buffer,
            &point_compute.colormap_buffer,
        );

        // Create gizmo renderer
        let gizmo_renderer = GizmoRenderer::new(
            &gpu.device,
            gpu.format,
            &scene.transforms,
        );

        // Create depth texture
        let (width, height) = gpu.size();
        let depth_texture = create_depth_texture(&gpu.device, width, height, "main_depth");

        // Screenshot resources
        let screenshot_texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot_texture"),
            size: wgpu::Extent3d {
                width: SCREENSHOT_WIDTH,
                height: SCREENSHOT_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let screenshot_depth = create_depth_texture(&gpu.device, SCREENSHOT_WIDTH, SCREENSHOT_HEIGHT, "screenshot_depth");

        let bytes_per_row = SCREENSHOT_WIDTH * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let screenshot_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot_buffer"),
            size: (padded_bytes_per_row * SCREENSHOT_HEIGHT) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Create text renderer
        let text_renderer = TextRenderer::new(&gpu.device, &gpu.queue, gpu.format);

        // Save scene data for text overlay
        let scene_name = scene.name.clone();
        let scene_author = scene.author.clone();
        let transform_names = scene.transform_names.clone();
        let scene_transforms = scene.transforms.clone();
        let colormap = scene.colormap;
        let num_transforms = scene_transforms.len();
        let original_weights: Vec<f32> = scene_transforms.iter().map(|t| t.weight).collect();

        // Fog settings
        let (fog_brightness, fog_saturation) = if fog_enabled {
            (0.4, 0.3)
        } else {
            (1.0, 1.0)
        };

        Self {
            gpu,
            window,
            frame_count: 0,
            rotation: 0.0,
            camera_distance: scene.camera_distance,
            camera_focus: scene.camera_focus,
            camera_offset: scene.camera_offset,
            point_size,
            show_gizmos: true,
            show_text: true,
            // Env override lets automated captures verify the help overlay
            show_help: std::env::var("FRACTURIZE_SHOW_HELP").is_ok(),
            fog_near: 3.0,
            fog_far: 4.5,
            fog_brightness,
            fog_saturation,
            fps_tracker: FpsTracker::new(),
            point_compute,
            point_renderer,
            gizmo_renderer,
            text_renderer,
            scene_name,
            scene_author,
            buffer_capacity,
            transform_names,
            scene_transforms,
            colormap,
            selected_transform: Some(0),
            transform_enabled: vec![true; num_transforms],
            original_weights,
            depth_texture,
            screenshot_texture,
            screenshot_depth,
            screenshot_buffer,
            pending_screenshot: false,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        if width > 0 && height > 0 {
            self.depth_texture = create_depth_texture(&self.gpu.device, width, height, "main_depth");
        }
    }

    pub fn reset(&mut self) {
        self.point_compute.reset(&self.gpu.queue);
        self.frame_count = 0;
    }

    pub fn zoom_in(&mut self) {
        self.camera_distance *= 0.9;
    }

    pub fn zoom_out(&mut self) {
        self.camera_distance *= 1.1;
    }

    pub fn adjust_point_size(&mut self, increase: bool) {
        let factor = if increase { 1.1 } else { 0.909 };
        self.point_size = (self.point_size * factor).clamp(0.0001, 0.1);
        log::info!("Point size: {:.5}", self.point_size);
    }

    pub fn adjust_fog_intensity(&mut self, more_fog: bool) {
        let factor = if more_fog { 0.7 } else { 1.4 };
        let old_b = self.fog_brightness;
        let old_s = self.fog_saturation;
        self.fog_brightness = (self.fog_brightness * factor).clamp(0.05, 1.0);
        self.fog_saturation = (self.fog_saturation * factor).clamp(0.05, 1.0);
        log::info!("Fog intensity: brightness {:.2}->{:.2}, saturation {:.2}->{:.2}",
            old_b, self.fog_brightness, old_s, self.fog_saturation);
    }

    pub fn adjust_fog_near(&mut self, closer: bool) {
        let delta = if closer { -0.5 } else { 0.5 };
        self.fog_near = (self.fog_near + delta).clamp(0.1, self.fog_far - 1.0);
        log::info!("Fog near: {:.1}", self.fog_near);
    }

    pub fn adjust_fog_far(&mut self, closer: bool) {
        let delta = if closer { -1.0 } else { 1.0 };
        self.fog_far = (self.fog_far + delta).clamp(self.fog_near + 1.0, 30.0);
        log::info!("Fog far: {:.1}", self.fog_far);
    }

    pub fn toggle_gizmos(&mut self) {
        self.show_gizmos = !self.show_gizmos;
        log::info!("Gizmos: {}", if self.show_gizmos { "on" } else { "off" });
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    pub fn toggle_text_overlay(&mut self) {
        self.show_text = !self.show_text;
        self.selected_transform = if self.show_text { Some(0) } else { None };
        log::info!("Text overlay: {}", if self.show_text { "on" } else { "off" });
    }

    pub fn select_prev_transform(&mut self) {
        if let Some(idx) = self.selected_transform {
            let n = self.scene_transforms.len();
            self.selected_transform = Some(if idx == 0 { n - 1 } else { idx - 1 });
        }
    }

    pub fn select_next_transform(&mut self) {
        if let Some(idx) = self.selected_transform {
            let n = self.scene_transforms.len();
            self.selected_transform = Some((idx + 1) % n);
        }
    }

    pub fn toggle_selected_transform(&mut self) {
        let Some(idx) = self.selected_transform else { return };
        if idx >= self.transform_enabled.len() { return }

        // Guard: don't disable the last enabled transform
        let enabled_count = self.transform_enabled.iter().filter(|&&e| e).count();
        if self.transform_enabled[idx] && enabled_count <= 1 {
            log::warn!("Cannot disable last remaining transform");
            return;
        }

        self.transform_enabled[idx] = !self.transform_enabled[idx];
        log::info!(
            "Transform {} {}", idx,
            if self.transform_enabled[idx] { "enabled" } else { "disabled" },
        );

        self.point_compute.update_weights(
            &self.gpu.queue,
            &self.scene_transforms,
            &self.transform_enabled,
        );
        self.gizmo_renderer.update_alpha(&self.gpu.queue, &self.transform_enabled);
        self.reset();
    }

    pub fn request_screenshot(&mut self) {
        self.pending_screenshot = true;
    }

    /// Use native 1px point primitives (~3x faster) when the projected point
    /// size at the orbit distance would be subpixel anyway
    fn use_point_primitives(&self, screen_height: f32) -> bool {
        self.point_size * screen_height / self.camera_distance <= 1.5
    }

    pub fn update(&mut self) {
        let should_log = self.fps_tracker.frame();
        self.frame_count += 1;
        self.rotation += 0.003;

        if should_log {
            let point_count = self.point_compute.valid_point_count();
            let warmup_done = self.point_compute.current_frame >= self.point_compute.warmup_frames;
            let title = format!(
                "Fracturize | {:.0} FPS | {:.1}ms | {}k points{}",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                point_count / 1000,
                if warmup_done { "" } else { " (warming up)" },
            );
            self.window.set_title(&title);
            log::info!(
                "FPS: {:.1} | Frametime: {:.2}ms | Points: {}",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                point_count,
            );
        }
    }

    pub fn take_screenshot(&mut self) {
        let point_count = self.point_compute.valid_point_count();
        if point_count == 0 {
            log::warn!("No points to screenshot");
            return;
        }

        let aspect = SCREENSHOT_WIDTH as f32 / SCREENSHOT_HEIGHT as f32;
        let cam_pos = self.camera_focus + self.camera_offset + Vec3::new(
            self.rotation.sin() * self.camera_distance,
            0.0,
            self.rotation.cos() * self.camera_distance,
        );
        let projection = Mat4::perspective_rh(45.0_f32.to_radians(), aspect, 0.1, 100.0);
        let view = Mat4::look_at_rh(cam_pos, self.camera_focus, Vec3::Y);
        let mvp = projection * view;

        let camera = CameraUniforms::new(
            mvp,
            SCREENSHOT_HEIGHT as f32,
            self.point_size,
            aspect,
            1.0,
            self.fog_near,
            self.fog_far,
            self.fog_brightness,
            self.fog_saturation,
        );
        self.point_renderer.upload_camera(&self.gpu.queue, &camera);

        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("screenshot_encoder"),
        });

        let color_view = self.screenshot_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.screenshot_depth.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("screenshot_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
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

            self.point_renderer.draw(
                &mut render_pass,
                point_count,
                self.use_point_primitives(SCREENSHOT_HEIGHT as f32),
            );
        }

        let bytes_per_row = SCREENSHOT_WIDTH * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.screenshot_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.screenshot_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(SCREENSHOT_HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: SCREENSHOT_WIDTH,
                height: SCREENSHOT_HEIGHT,
                depth_or_array_layers: 1,
            },
        );

        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = self.screenshot_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });

        {
            let data = buffer_slice.get_mapped_range();

            let mut pixels = vec![0u8; (SCREENSHOT_WIDTH * SCREENSHOT_HEIGHT * 4) as usize];
            for y in 0..SCREENSHOT_HEIGHT as usize {
                let src_start = y * padded_bytes_per_row as usize;
                let src_end = src_start + bytes_per_row as usize;
                let dst_start = y * bytes_per_row as usize;
                let dst_end = dst_start + bytes_per_row as usize;
                pixels[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
            }

            for chunk in pixels.chunks_mut(4) {
                chunk.swap(0, 2);
            }

            let screenshot_dir = Path::new("screenshots");
            if !screenshot_dir.exists() {
                fs::create_dir_all(screenshot_dir).expect("Failed to create screenshots directory");
            }

            let path = screenshot_dir.join("capture.png");
            image::save_buffer(
                &path,
                &pixels,
                SCREENSHOT_WIDTH,
                SCREENSHOT_HEIGHT,
                image::ColorType::Rgba8,
            ).expect("Failed to save screenshot");
            log::info!("Screenshot saved to {}", path.display());
        }

        self.screenshot_buffer.unmap();
        self.pending_screenshot = false;
    }

    /// Keybind help panel, shown when `show_help` is on
    fn build_help_entries(&self, height: f32) -> Vec<TextEntry> {
        const HELP: &[(&str, &str)] = &[
            ("H / ?", "toggle this help"),
            ("Esc", "quit"),
            ("Space", "re-seed points (reset)"),
            ("Up / Down", "zoom in / out"),
            ("", "(select transform when overlay is on)"),
            ("Enter", "enable/disable selected transform"),
            ("T", "toggle info overlay"),
            ("G", "toggle transform gizmos"),
            ("S", "save screenshot to screenshots/capture.png"),
            ("[ / ]", "shrink / grow point size"),
            ("F / Shift+F", "more / less fog"),
            ("N / Shift+N", "fog start closer / farther"),
            ("M / Shift+M", "fog end closer / farther"),
        ];

        let font_size = 13.0;
        let line_height = font_size * 1.2;
        // +2 lines: title and trailing spacer
        let panel_lines = HELP.len() + 2;
        let panel_y = (height - panel_lines as f32 * line_height) * 0.5;

        let text: String = std::iter::once("Keybinds".to_string())
            .chain(std::iter::once(String::new()))
            .chain(HELP.iter().map(|(key, desc)| format!("{:<13} {}", key, desc)))
            .collect::<Vec<_>>()
            .join("\n");

        // Dark block-character backdrop so the panel reads over a bright fractal
        let bg_width = 4 + text.lines().map(|l| l.len()).max().unwrap_or(0);
        let bg: String = vec!["\u{2588}".repeat(bg_width); panel_lines].join("\n");

        vec![
            TextEntry {
                text: bg,
                x: 6.0,
                y: panel_y,
                color: [10, 10, 20, 215],
                font_size,
            },
            TextEntry {
                text,
                x: 6.0 + font_size, // ~2 block chars of left padding
                y: panel_y + line_height * 0.5,
                color: [235, 235, 245, 255],
                font_size,
            },
        ]
    }

    fn build_text_entries(&self, view_proj: Mat4, width: f32, height: f32) -> Vec<TextEntry> {
        let mut entries = Vec::new();

        if self.show_help {
            entries.extend(self.build_help_entries(height));
        } else {
            // Discoverability hint, bottom-left
            entries.push(TextEntry {
                text: "[H] keybinds".to_string(),
                x: 10.0,
                y: height - 24.0,
                color: [160, 160, 170, 180],
                font_size: 12.0,
            });
        }

        if !self.show_text {
            return entries;
        }

        let white = [255, 255, 255, 255];
        let grey = [180, 180, 180, 220];

        // === Top-left HUD ===
        let point_count = self.point_compute.valid_point_count();

        // Scene name + author
        entries.push(TextEntry {
            text: format!("{} — {}", self.scene_name, self.scene_author),
            x: 10.0,
            y: 10.0,
            color: grey,
            font_size: 12.0,
        });

        // Performance + point stats
        entries.push(TextEntry {
            text: format!(
                "{:.0} FPS | {:.1}ms | {}k / {}k points",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                point_count / 1000,
                self.buffer_capacity / 1000,
            ),
            x: 10.0,
            y: 26.0,
            color: white,
            font_size: 14.0,
        });

        // Camera params
        let mut param_y = 48.0;
        entries.push(TextEntry {
            text: format!(
                "cam: d={:.1} focus=({:.1},{:.1},{:.1}) off=({:.1},{:.1},{:.1})",
                self.camera_distance,
                self.camera_focus.x, self.camera_focus.y, self.camera_focus.z,
                self.camera_offset.x, self.camera_offset.y, self.camera_offset.z,
            ),
            x: 10.0,
            y: param_y,
            color: grey,
            font_size: 12.0,
        });
        param_y += 16.0;

        // Point size
        entries.push(TextEntry {
            text: format!("pt size={:.4}", self.point_size),
            x: 10.0,
            y: param_y,
            color: grey,
            font_size: 12.0,
        });
        param_y += 16.0;

        // Fog params (only if not default)
        if self.fog_brightness < 1.0 || self.fog_saturation < 1.0 {
            entries.push(TextEntry {
                text: format!(
                    "fog: near={:.1} far={:.1} b={:.2} s={:.2}",
                    self.fog_near, self.fog_far, self.fog_brightness, self.fog_saturation,
                ),
                x: 10.0,
                y: param_y,
                color: grey,
                font_size: 12.0,
            });
        }

        // === Right-side transform list ===
        let right_x = width - 250.0;
        let mut y = 10.0;
        entries.push(TextEntry {
            text: "Transforms".to_string(),
            x: right_x,
            y,
            color: white,
            font_size: 14.0,
        });
        y += 20.0;

        for (i, spec) in self.scene_transforms.iter().enumerate() {
            let name = self.transform_names
                .get(i)
                .and_then(|n| n.as_deref())
                .unwrap_or("");
            let label = if name.is_empty() {
                format!("T{}", i)
            } else {
                format!("T{}: {}", i, name)
            };

            let translation = spec.matrix.w_axis.truncate();
            let scale = spec.matrix.x_axis.length();

            let is_selected = self.selected_transform == Some(i);
            let is_enabled = self.transform_enabled.get(i).copied().unwrap_or(true);

            let sel = if is_selected { ">" } else { " " };
            let on = if is_enabled { " " } else { "×" };

            let summary = spec.variation_summary();
            let var_info = if summary == "linear" {
                String::new()
            } else {
                format!(" [{}]", summary)
            };
            let line = format!(
                "{}{} {} p=({:.2},{:.2},{:.2}) s={:.2} w={:.1}{}",
                sel, on, label, translation.x, translation.y, translation.z, scale, spec.weight, var_info,
            );

            // Use colormap color for this transform, dim if disabled
            let cm_idx = (spec.color_value * 255.0).clamp(0.0, 255.0) as usize;
            let cm = self.colormap[cm_idx];
            let alpha: u8 = if is_enabled { 220 } else { 80 };
            let color = [
                (cm[0] * 255.0) as u8,
                (cm[1] * 255.0) as u8,
                (cm[2] * 255.0) as u8,
                alpha,
            ];

            entries.push(TextEntry {
                text: line,
                x: right_x,
                y,
                color,
                font_size: 12.0,
            });
            y += 16.0;
        }

        // === World-space gizmo labels ===
        for (i, spec) in self.scene_transforms.iter().enumerate() {
            let is_enabled = self.transform_enabled.get(i).copied().unwrap_or(true);
            let is_selected = self.selected_transform == Some(i);
            let origin = spec.matrix.w_axis.truncate();
            if let Some((sx, sy)) = world_to_screen(origin, view_proj, width, height) {
                let name = self.transform_names
                    .get(i)
                    .and_then(|n| n.as_deref())
                    .unwrap_or("");
                let label = if name.is_empty() {
                    format!("T{}", i)
                } else {
                    name.to_string()
                };

                if is_selected {
                    // Draw background highlight for selected label
                    let bg_chars: String = "\u{2588}".repeat(label.len() + 2);
                    entries.push(TextEntry {
                        text: bg_chars,
                        x: sx + 4.0,
                        y: sy - 7.0,
                        color: [255, 255, 255, 200],
                        font_size: 13.0,
                    });
                    entries.push(TextEntry {
                        text: format!(" {} ", label),
                        x: sx + 4.0,
                        y: sy - 6.0,
                        color: [0, 0, 0, 255],
                        font_size: 13.0,
                    });
                } else {
                    let alpha: u8 = if is_enabled { 200 } else { 80 };
                    entries.push(TextEntry {
                        text: label,
                        x: sx + 8.0,
                        y: sy - 6.0,
                        color: [255, 255, 255, alpha],
                        font_size: 13.0,
                    });
                }
            }
        }

        entries
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        if self.pending_screenshot {
            self.take_screenshot();
        }

        let (width, height) = self.gpu.size();
        let aspect = width as f32 / height as f32;
        let cam_pos = self.camera_focus + self.camera_offset + Vec3::new(
            self.rotation.sin() * self.camera_distance,
            0.0,
            self.rotation.cos() * self.camera_distance,
        );

        let projection = Mat4::perspective_rh(45.0_f32.to_radians(), aspect, 0.1, 100.0);
        let view = Mat4::look_at_rh(cam_pos, self.camera_focus, Vec3::Y);
        let view_proj = projection * view;

        // Advance circular buffer and get valid point count
        let point_count = self.point_compute.advance_frame(&self.gpu.queue);

        let output = self.gpu.surface.get_current_texture()?;
        let color_view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame_encoder"),
        });

        // === STEP 1: RUN CHAOS GAME ===
        self.point_compute.dispatch(&mut encoder);

        // === STEP 2: RENDER POINTS ===
        let camera = CameraUniforms::new(
            view_proj,
            height as f32,
            self.point_size,
            aspect,
            1.0,
            self.fog_near,
            self.fog_far,
            self.fog_brightness,
            self.fog_saturation,
        );
        self.point_renderer.upload_camera(&self.gpu.queue, &camera);
        self.gizmo_renderer.upload_camera(&self.gpu.queue, &camera);

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("point_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if point_count > 0 {
                self.point_renderer.draw(
                    &mut render_pass,
                    point_count,
                    self.use_point_primitives(height as f32),
                );
            }
        }

        // === STEP 3: RENDER GIZMOS ===
        if self.show_gizmos {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gizmo_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.gizmo_renderer.draw(&mut render_pass);
        }

        // === STEP 4: RENDER TEXT OVERLAY ===
        {
            let entries = self.build_text_entries(view_proj, width as f32, height as f32);
            self.text_renderer.prepare(&self.gpu.device, &self.gpu.queue, width, height, &entries);

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.text_renderer.render(&mut render_pass);
        }

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
