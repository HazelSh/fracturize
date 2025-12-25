use std::fs;
use std::path::Path;
use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};
use winit::window::Window;

use crate::gpu::{CameraUniforms, ChaosCompute, GpuContext, PointRenderer, DEPTH_FORMAT};
use crate::scene::Scene;

/// Clear color: dark blue-black [0.02, 0.02, 0.05, 1.0]
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.02,
    b: 0.05,
    a: 1.0,
};

/// Screenshot dimensions (fixed for consistent output)
const SCREENSHOT_WIDTH: u32 = 1280;
const SCREENSHOT_HEIGHT: u32 = 720;

/// Create a depth texture of the given size
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

/// Main application state
pub struct App {
    pub gpu: GpuContext,
    pub window: Arc<Window>,
    pub frame_count: u32,
    pub rotation: f32,
    pub camera_distance: f32,
    pub point_size: f32,

    // GPU compute
    compute: ChaosCompute,
    renderer: PointRenderer,
    max_points: u32,

    // Depth buffer for main render
    depth_texture: wgpu::Texture,

    // Screenshot support
    screenshot_texture: wgpu::Texture,
    screenshot_depth: wgpu::Texture,
    screenshot_buffer: wgpu::Buffer,
    pub pending_screenshot: bool,
}

impl App {
    /// Create a new App with the given window and scene
    pub async fn new(window: Arc<Window>, scene: Scene) -> Self {
        let gpu = GpuContext::new(window.clone()).await;

        log::info!("Loaded scene: {}", scene.name);

        let point_size = scene.point_size;
        let max_points = scene.max_points;
        let iterations_per_frame = scene.iters;

        // Convert transforms to GPU format
        let transforms: Vec<(Mat4, Vec4, f32)> = scene
            .transforms
            .into_iter()
            .map(|(matrix, color, weight)| (matrix, color.extend(1.0), weight))
            .collect();

        // Create compute pipeline
        let compute = ChaosCompute::new(
            &gpu.device,
            &transforms,
            max_points,
            iterations_per_frame,
        );

        // Create renderer using compute's point buffer
        let renderer = PointRenderer::new(&gpu.device, gpu.format, &compute.point_buffer);

        // Create depth texture for main render
        let (width, height) = gpu.size();
        let depth_texture = create_depth_texture(&gpu.device, width, height, "main_depth");

        // Create screenshot texture
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

        // Create screenshot depth texture
        let screenshot_depth = create_depth_texture(&gpu.device, SCREENSHOT_WIDTH, SCREENSHOT_HEIGHT, "screenshot_depth");

        // Create buffer for reading back screenshot data
        // Row must be aligned to 256 bytes for copy
        let bytes_per_row = SCREENSHOT_WIDTH * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let screenshot_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot_buffer"),
            size: (padded_bytes_per_row * SCREENSHOT_HEIGHT) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            gpu,
            window,
            frame_count: 0,
            rotation: 0.0,
            camera_distance: 3.0,
            point_size,
            compute,
            renderer,
            max_points: max_points as u32,
            depth_texture,
            screenshot_texture,
            screenshot_depth,
            screenshot_buffer,
            pending_screenshot: false,
        }
    }

    /// Handle window resize
    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        // Recreate depth texture for new size
        if width > 0 && height > 0 {
            self.depth_texture = create_depth_texture(&self.gpu.device, width, height, "main_depth");
        }
    }

    /// Reset the IFS state
    pub fn reset(&mut self) {
        self.compute.reset(&self.gpu.queue);
        self.frame_count = 0;
    }

    /// Zoom in
    pub fn zoom_in(&mut self) {
        self.camera_distance *= 0.9;
    }

    /// Zoom out
    pub fn zoom_out(&mut self) {
        self.camera_distance *= 1.1;
    }

    /// Request a screenshot to be taken
    pub fn request_screenshot(&mut self) {
        self.pending_screenshot = true;
    }

    /// Update state (called each frame)
    pub fn update(&mut self) {
        self.frame_count += 1;
        self.rotation += 0.005;
    }

    /// Take a screenshot (renders to texture, copies to CPU, saves PNG)
    pub fn take_screenshot(&mut self) {
        let point_count = (self.frame_count * self.compute.iterations_per_dispatch).min(self.max_points);
        if point_count == 0 {
            log::warn!("No points to screenshot");
            return;
        }

        // Calculate camera for fixed screenshot size
        let aspect = SCREENSHOT_WIDTH as f32 / SCREENSHOT_HEIGHT as f32;
        let cam_pos = Vec3::new(
            self.rotation.sin() * self.camera_distance,
            1.0,
            self.rotation.cos() * self.camera_distance,
        );
        let projection = Mat4::perspective_rh(45.0_f32.to_radians(), aspect, 0.1, 1000.0);
        let view = Mat4::look_at_rh(cam_pos, Vec3::ZERO, Vec3::Y);
        let mvp = projection * view;

        let camera = CameraUniforms::new(mvp, SCREENSHOT_HEIGHT as f32, self.point_size, aspect, 1.0);
        self.renderer.upload_camera(&self.gpu.queue, &camera);

        // Create command encoder
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("screenshot_encoder"),
        });

        // Render to screenshot texture with depth
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
            });

            self.renderer.draw(&mut render_pass, point_count);
        }

        // Copy texture to buffer
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

        // Map the buffer and read data
        let buffer_slice = self.screenshot_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        self.gpu.device.poll(wgpu::Maintain::Wait);

        {
            let data = buffer_slice.get_mapped_range();

            // Copy to properly sized buffer (removing row padding)
            let mut pixels = vec![0u8; (SCREENSHOT_WIDTH * SCREENSHOT_HEIGHT * 4) as usize];
            for y in 0..SCREENSHOT_HEIGHT as usize {
                let src_start = y * padded_bytes_per_row as usize;
                let src_end = src_start + bytes_per_row as usize;
                let dst_start = y * bytes_per_row as usize;
                let dst_end = dst_start + bytes_per_row as usize;
                pixels[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
            }

            // Convert BGRA to RGBA (if using Bgra8UnormSrgb format)
            for chunk in pixels.chunks_mut(4) {
                chunk.swap(0, 2); // Swap B and R
            }

            // Ensure screenshots directory exists
            let screenshot_dir = Path::new("screenshots");
            if !screenshot_dir.exists() {
                fs::create_dir_all(screenshot_dir).expect("Failed to create screenshots directory");
            }

            // Save as PNG
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

    /// Render a frame
    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        // Handle pending screenshot
        if self.pending_screenshot {
            self.take_screenshot();
        }

        // Calculate how many points we have so far
        let total_iterations = self.frame_count * self.compute.iterations_per_dispatch;
        let point_count = total_iterations.min(self.max_points);

        // Calculate camera matrices
        let (width, height) = self.gpu.size();
        let aspect = width as f32 / height as f32;
        let cam_pos = Vec3::new(
            self.rotation.sin() * self.camera_distance,
            1.0,
            self.rotation.cos() * self.camera_distance,
        );
        let cam_target = Vec3::ZERO;

        let projection = Mat4::perspective_rh(45.0_f32.to_radians(), aspect, 0.1, 1000.0);
        let view = Mat4::look_at_rh(cam_pos, cam_target, Vec3::Y);
        let mvp = projection * view;

        // Upload camera uniforms
        // min_point_pixels = 1.0 ensures distant points stay visible (Apophysis-style fizz)
        let camera = CameraUniforms::new(mvp, height as f32, self.point_size, aspect, 1.0);
        self.renderer.upload_camera(&self.gpu.queue, &camera);

        // Get the next frame's texture
        let output = self.gpu.surface.get_current_texture()?;
        let color_view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create command encoder
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame_encoder"),
        });

        // Compute pass - run chaos game iterations
        self.compute.dispatch(&mut encoder);

        // Render pass with depth testing
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
            });

            if point_count > 0 {
                self.renderer.draw(&mut render_pass, point_count);
            }
        }

        // Submit commands
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
