use wgpu::{BindGroup, BindGroupLayout, Buffer, Device, RenderPipeline, TextureFormat};

use super::buffers::{create_camera_buffer, CameraUniforms};

/// Depth buffer format
pub const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

/// Point render pipeline with depth testing, no blending
pub struct PointRenderer {
    pub pipeline: RenderPipeline,
    pub bind_group_layout: BindGroupLayout,
    pub bind_group: BindGroup,
    pub camera_buffer: Buffer,
}

impl PointRenderer {
    /// Create a new point renderer using an existing point buffer (from compute)
    pub fn new(device: &Device, format: TextureFormat, point_buffer: &Buffer) -> Self {
        // Load shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("points_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/points.wgsl").into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("point_bind_group_layout"),
            entries: &[
                // Points storage buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Camera uniforms
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline with depth testing, no blending
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("point_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[], // No vertex buffers - using vertex pulling
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // No blending - opaque rendering
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None, // Don't cull - we want to see all triangles
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            // Enable depth testing - closer points occlude farther ones
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Create camera buffer
        let camera_buffer = create_camera_buffer(device);

        // Create bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: point_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: camera_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            pipeline,
            bind_group_layout,
            bind_group,
            camera_buffer,
        }
    }

    /// Upload camera uniforms
    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Draw points (call within a render pass)
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, point_count: u32) {
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        // 6 vertices per point (2 triangles for quad)
        render_pass.draw(0..point_count * 6, 0..1);
    }
}
