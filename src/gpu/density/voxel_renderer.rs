use wgpu::{BindGroup, BindGroupLayout, Buffer, Device, RenderPipeline, TextureFormat};

use crate::gpu::buffers::{create_camera_buffer, CameraUniforms};

/// Depth buffer format (32-bit float for precision)
pub const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

/// Voxel render pipeline
/// Renders billboard quads from compacted voxel buffer
pub struct VoxelRenderer {
    pub pipeline: RenderPipeline,
    pub bind_group_layout: BindGroupLayout,
    pub bind_group: BindGroup,
    pub camera_buffer: Buffer,
}

impl VoxelRenderer {
    pub fn new(
        device: &Device,
        format: TextureFormat,
        voxel_buffer: &Buffer,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel_render_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/density/voxel_render.wgsl").into()),
        });

        // Bind group layout:
        // 0: voxels (read)
        // 1: camera (uniform)
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voxel_bind_group_layout"),
            entries: &[
                // Voxels storage buffer
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None, // Opaque rendering
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let camera_buffer = create_camera_buffer(device);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxel_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: voxel_buffer.as_entire_binding(),
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

    /// Draw voxels - 6 vertices per voxel (2 triangles for quad billboards)
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, voxel_count: u32) {
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.draw(0..voxel_count * 6, 0..1);
    }
}
