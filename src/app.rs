use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::gpu::{
    CameraUniforms, ChaosCompute, CompactPipeline, GpuContext, HashGrid, HashGridParams,
    ReprojectPipeline, VoxelRenderer, DEPTH_FORMAT,
};
use crate::scene::Scene;

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
    pub point_size: f32,
    pub points_per_frame: usize,
    pub decay: f32,

    // Fog parameters
    pub fog_near: f32,
    pub fog_far: f32,
    pub fog_brightness: f32,
    pub fog_saturation: f32,

    fps_tracker: FpsTracker,

    // Hash grid rendering pipeline
    hash_grid: HashGrid,
    reproject: ReprojectPipeline,
    compact: CompactPipeline,
    chaos_compute: ChaosCompute,
    voxel_renderer: VoxelRenderer,
    colormap_buffer: wgpu::Buffer,

    // Previous frame's inverse view-projection for reprojection
    prev_inv_view_proj: Mat4,
    prev_view_proj: Mat4,

    depth_texture: wgpu::Texture,

    screenshot_texture: wgpu::Texture,
    screenshot_depth: wgpu::Texture,
    screenshot_buffer: wgpu::Buffer,
    pub pending_screenshot: bool,
}

impl App {
    /// Create a new App
    pub async fn new(window: Arc<Window>, scene: Scene, fog_enabled: bool) -> Self {
        let gpu = GpuContext::new(window.clone()).await;

        log::info!("Loaded scene: {} by {}", scene.name, scene.author);
        log::info!(
            "Hash grid mode: 4M entries × 16 bytes = {:.1}MB per grid",
            (4 * 1024 * 1024 * 16) as f64 / 1_000_000.0
        );

        let point_size = scene.point_size;
        log::info!("Point size from scene: {}", point_size);

        let points_per_frame = scene.points_per_frame;
        log::info!("Points per frame: {}", points_per_frame);
        log::info!("Decay factor: {}", scene.decay);

        // Create hash grid (double-buffered)
        let hash_grid = HashGrid::new(&gpu.device);

        // Create compute pipelines
        let reproject = ReprojectPipeline::new(&gpu.device);
        let compact = CompactPipeline::new(&gpu.device);
        let chaos_compute = ChaosCompute::new(&gpu.device, &scene.transforms, points_per_frame);

        // Create colormap buffer (shared with compact pipeline)
        let colormap_buffer = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("colormap_buffer"),
            contents: bytemuck::cast_slice(&scene.colormap),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Create voxel renderer
        let voxel_renderer = VoxelRenderer::new(&gpu.device, gpu.format, &hash_grid.render_voxels);

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

        // Fog settings
        let (fog_brightness, fog_saturation) = if fog_enabled {
            (0.4, 0.3)
        } else {
            (1.0, 1.0)
        };

        // Initial view-projection (will be updated on first frame)
        let initial_vp = Mat4::IDENTITY;

        Self {
            gpu,
            window,
            frame_count: 0,
            rotation: 0.0,
            camera_distance: 3.0,
            point_size,
            points_per_frame,
            decay: scene.decay,
            fog_near: 3.0,
            fog_far: 4.5,
            fog_brightness,
            fog_saturation,
            fps_tracker: FpsTracker::new(),
            hash_grid,
            reproject,
            compact,
            chaos_compute,
            voxel_renderer,
            colormap_buffer,
            prev_inv_view_proj: initial_vp.inverse(),
            prev_view_proj: initial_vp,
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
        self.chaos_compute.reset(&self.gpu.queue);
        self.frame_count = 0;
        // Clear grids on reset
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("reset_encoder"),
        });
        self.hash_grid.clear_current(&mut encoder, &self.gpu.device);
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        self.hash_grid.swap();
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("reset_encoder2"),
        });
        self.hash_grid.clear_current(&mut encoder, &self.gpu.device);
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
    }

    pub fn zoom_in(&mut self) {
        self.camera_distance *= 0.9;
    }

    pub fn zoom_out(&mut self) {
        self.camera_distance *= 1.1;
    }

    pub fn adjust_point_count(&mut self, increase: bool) {
        let factor = if increase { 1.2 } else { 0.833 };
        self.points_per_frame = ((self.points_per_frame as f32 * factor) as usize).clamp(10_000, 100_000_000);
        self.chaos_compute.set_iterations(&self.gpu.queue, self.points_per_frame);
        log::info!("Points per frame: {:.2}M", self.points_per_frame as f32 / 1_000_000.0);
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

    pub fn request_screenshot(&mut self) {
        self.pending_screenshot = true;
    }

    pub fn update(&mut self) {
        let should_log = self.fps_tracker.frame();
        self.frame_count += 1;
        self.rotation += 0.005;

        if should_log {
            let voxel_count = self.hash_grid.current_voxel_count;
            let title = format!(
                "Fracturize | {:.0} FPS | {:.1}ms | {}k voxels | {:.1}M pts/frame",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                voxel_count / 1000,
                self.points_per_frame as f32 / 1_000_000.0,
            );
            self.window.set_title(&title);
            log::info!(
                "FPS: {:.1} | Frametime: {:.2}ms | Voxels: {}",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                voxel_count,
            );
        }
    }

    pub fn take_screenshot(&mut self) {
        let voxel_count = self.hash_grid.current_voxel_count;
        if voxel_count == 0 {
            log::warn!("No voxels to screenshot");
            return;
        }

        let aspect = SCREENSHOT_WIDTH as f32 / SCREENSHOT_HEIGHT as f32;
        let cam_pos = Vec3::new(
            self.rotation.sin() * self.camera_distance,
            1.0,
            self.rotation.cos() * self.camera_distance,
        );
        let projection = Mat4::perspective_rh(45.0_f32.to_radians(), aspect, 0.1, 1000.0);
        let view = Mat4::look_at_rh(cam_pos, Vec3::ZERO, Vec3::Y);
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
        self.voxel_renderer.upload_camera(&self.gpu.queue, &camera);

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

            self.voxel_renderer.draw(&mut render_pass, voxel_count);
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

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        if self.pending_screenshot {
            self.take_screenshot();
        }

        let (width, height) = self.gpu.size();
        let aspect = width as f32 / height as f32;
        let cam_pos = Vec3::new(
            self.rotation.sin() * self.camera_distance,
            1.0,
            self.rotation.cos() * self.camera_distance,
        );

        let projection = Mat4::perspective_rh(45.0_f32.to_radians(), aspect, 0.1, 1000.0);
        let view = Mat4::look_at_rh(cam_pos, Vec3::ZERO, Vec3::Y);
        let curr_view_proj = projection * view;

        // Create grid params for this frame
        let grid_params = HashGridParams::new(
            self.prev_inv_view_proj,
            curr_view_proj,
            width,  // screen resolution x
            height, // screen resolution y
            0.1,    // near plane
            1000.0, // far plane
            self.decay,
            self.frame_count,
        );
        self.hash_grid.upload_params(&self.gpu.queue, &grid_params);

        // Reset voxel counter for compaction
        self.hash_grid.reset_counter(&self.gpu.queue);

        // Clear current grid before reprojection/accumulation
        // We do this with a GPU fill rather than a full copy
        let mut clear_encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("clear_encoder"),
        });
        clear_encoder.clear_buffer(self.hash_grid.curr_grid(), 0, None);
        self.gpu.queue.submit(std::iter::once(clear_encoder.finish()));

        let output = self.gpu.surface.get_current_texture()?;
        let color_view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame_encoder"),
        });

        // === STEP 1: REPROJECT ===
        // Reproject previous frame's voxels to current view
        let reproject_bind_group = self.reproject.create_bind_group(
            &self.gpu.device,
            self.hash_grid.prev_grid(),
            self.hash_grid.curr_grid(),
            &self.hash_grid.params_buffer,
        );
        self.reproject.dispatch(&mut encoder, &reproject_bind_group);

        // === STEP 2: ACCUMULATE ===
        // Run chaos game, projecting new points into current grid
        let chaos_bind_group = self.chaos_compute.create_bind_group(
            &self.gpu.device,
            self.hash_grid.curr_grid(),
            &self.hash_grid.params_buffer,
        );
        self.chaos_compute.dispatch(&mut encoder, &chaos_bind_group);

        // === STEP 3: COMPACT ===
        // Extract non-empty cells to dense render buffer
        let compact_bind_group = self.compact.create_bind_group(
            &self.gpu.device,
            self.hash_grid.curr_grid(),
            &self.hash_grid.render_voxels,
            &self.hash_grid.voxel_counter,
            &self.hash_grid.params_buffer,
            &self.colormap_buffer,
        );
        self.compact.dispatch(&mut encoder, &compact_bind_group);

        // Copy counter to staging for readback
        self.hash_grid.copy_counter_to_staging(&mut encoder);

        // === STEP 4: RENDER ===
        // Upload camera uniforms for voxel rendering
        let camera = CameraUniforms::new(
            curr_view_proj,
            height as f32,
            self.point_size,
            aspect,
            1.0,
            self.fog_near,
            self.fog_far,
            self.fog_brightness,
            self.fog_saturation,
        );
        self.voxel_renderer.upload_camera(&self.gpu.queue, &camera);

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxel_pass"),
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

            // Use previous frame's voxel count (to avoid stall)
            let voxel_count = self.hash_grid.current_voxel_count;
            if voxel_count > 0 {
                self.voxel_renderer.draw(&mut render_pass, voxel_count);
            }
        }

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        // === STEP 5: SWAP and READ BACK ===
        // Read back the voxel count for next frame
        self.hash_grid.read_counter(&self.gpu.device);

        // Swap grids for next frame
        self.hash_grid.swap();

        // Store current matrices for next frame's reprojection
        self.prev_view_proj = curr_view_proj;
        self.prev_inv_view_proj = curr_view_proj.inverse();

        Ok(())
    }
}
