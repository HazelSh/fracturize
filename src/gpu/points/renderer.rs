//! Simple point billboard renderer
//!
//! Renders points as axis-aligned billboard quads with depth-based sizing.

use wgpu::{BindGroup, Buffer, Device, RenderPipeline, TextureFormat};

use crate::gpu::buffers::{create_camera_buffer, CameraUniforms};

/// Depth buffer format
pub const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

/// Point renderer with two pipelines:
/// - billboard quads (4-vertex instanced strips) for visibly-sized points
/// - native 1px point primitives for subpixel points (~3x faster)
pub struct PointRenderer {
    pub quad_pipeline: RenderPipeline,
    pub point_pipeline: RenderPipeline,
    pub bind_group: BindGroup,
    pub camera_buffer: Buffer,
}

impl PointRenderer {
    pub fn new(
        device: &Device,
        format: TextureFormat,
        point_buffer: &Buffer,
        colormap_buffer: &Buffer,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point_render_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/points/render.wgsl").into()),
        });

        // Bind group layout:
        // 0: points (read)
        // 1: camera (uniform)
        // 2: colormap (read)
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Colormap
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let make_pipeline = |label: &str, entry: &str, topology: wgpu::PrimitiveTopology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
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
                    topology,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let quad_pipeline = make_pipeline(
            "point_quad_pipeline",
            "vs_main",
            wgpu::PrimitiveTopology::TriangleStrip,
        );
        let point_pipeline = make_pipeline(
            "point_pixel_pipeline",
            "vs_point",
            wgpu::PrimitiveTopology::PointList,
        );

        let camera_buffer = create_camera_buffer(device);

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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: colormap_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            quad_pipeline,
            point_pipeline,
            bind_group,
            camera_buffer,
        }
    }

    /// Upload camera uniforms
    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Draw points. When `use_point_primitives` is set, draws native 1px
    /// points (fast path); otherwise one 4-vertex quad instance per point.
    pub fn draw<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        point_count: u32,
        use_point_primitives: bool,
    ) {
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        if use_point_primitives {
            render_pass.set_pipeline(&self.point_pipeline);
            render_pass.draw(0..point_count, 0..1);
        } else {
            render_pass.set_pipeline(&self.quad_pipeline);
            render_pass.draw(0..4, 0..point_count);
        }
    }
}
