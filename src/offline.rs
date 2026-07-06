//! Headless single-frame renderer
//!
//! Renders one frame of a scene at arbitrary resolution without opening a
//! window (no surface, no event loop, no focus stealing). Runs the chaos
//! game until the point buffer is full, renders once, saves a PNG.

use std::path::Path;

use glam::{Mat4, Vec3};

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

pub struct OfflineParams<'a> {
    pub scene: Scene,
    pub view: Option<View>,
    pub width: u32,
    pub height: u32,
    pub out_path: &'a Path,
    /// Extra chaos-game frames after the point buffer is full
    pub accumulate: u32,
    pub fog_enabled: bool,
}

pub fn render(params: OfflineParams) -> Result<(), String> {
    let OfflineParams { scene, view, width, height, out_path, accumulate, fog_enabled } = params;

    // === Headless device (no surface) ===
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("fracturize_offline_device"),
        required_features: wgpu::Features::empty(),
        required_limits,
        memory_hints: wgpu::MemoryHints::Performance,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: Default::default(),
    }))
    .map_err(|e| format!("Failed to create device: {}", e))?;

    // === Pipelines ===
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let compute = PointCompute::new(&device, &scene.transforms, &scene.colormap, scene.point_count as u32);
    let renderer = PointRenderer::new(&device, format, &compute.point_buffer, &compute.colormap_buffer);

    // === Run the chaos game until the buffer is full, plus accumulation ===
    let mut compute = compute;
    let total_frames = compute.warmup_frames + accumulate.max(1);
    log::info!(
        "Filling {} point buffer: {} warmup + {} accumulation frames",
        scene.point_count, compute.warmup_frames, accumulate.max(1)
    );
    let mut point_count = 0;
    for i in 0..total_frames {
        point_count = compute.advance_frame(&queue);
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

    // === Camera (from view file, or scene defaults at orbit angle 0) ===
    let (rotation, distance, focus, offset, point_size, fog) = match &view {
        Some(v) => (
            v.rotation,
            v.distance,
            Vec3::from(v.focus),
            Vec3::from(v.offset),
            v.point_size,
            (v.fog_near, v.fog_far, v.fog_brightness, v.fog_saturation),
        ),
        None => {
            let (fb, fs) = if fog_enabled { (0.4, 0.3) } else { (1.0, 1.0) };
            (
                0.0,
                scene.camera_distance,
                scene.camera_focus,
                scene.camera_offset,
                scene.point_size,
                (3.0, 4.5, fb, fs),
            )
        }
    };

    let aspect = width as f32 / height as f32;
    let cam_pos = focus + offset + Vec3::new(rotation.sin() * distance, 0.0, rotation.cos() * distance);
    let projection = Mat4::perspective_rh(45.0_f32.to_radians(), aspect, 0.1, 100.0);
    let mvp = projection * Mat4::look_at_rh(cam_pos, focus, Vec3::Y);

    let camera = CameraUniforms::new(
        mvp, height as f32, point_size, aspect, 1.0,
        fog.0, fog.1, fog.2, fog.3,
    );
    renderer.upload_camera(&queue, &camera);
    let use_point_primitives = point_size * height as f32 / distance <= 1.5;

    // === Render one frame to an offscreen texture ===
    let color_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offline_color"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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

    let bytes_per_row = width * 4;
    let padded_bytes_per_row = (bytes_per_row + 255) & !255;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offline_readback"),
        size: (padded_bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("offline_render_encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offline_pass"),
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
        renderer.draw(&mut pass, point_count, use_point_primitives);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));

    // === Read back and save ===
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });

    {
        let data = slice.get_mapped_range();
        let mut pixels = vec![0u8; (bytes_per_row * height) as usize];
        for y in 0..height as usize {
            let src = y * padded_bytes_per_row as usize;
            let dst = y * bytes_per_row as usize;
            pixels[dst..dst + bytes_per_row as usize]
                .copy_from_slice(&data[src..src + bytes_per_row as usize]);
        }

        if let Some(dir) = out_path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
            }
        }
        image::save_buffer(out_path, &pixels, width, height, image::ColorType::Rgba8)
            .map_err(|e| format!("Failed to save {}: {}", out_path.display(), e))?;
    }
    readback.unmap();

    log::info!("Rendered {} points at {}x{} to {}", point_count, width, height, out_path.display());
    println!("Rendered {}x{} ({} points) -> {}", width, height, point_count, out_path.display());
    Ok(())
}
