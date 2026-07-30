//! Line-segment renderer for the in-world UI: chaos traces, camera paths and
//! the selected transform's indicators.
//!
//! Callers hand over pairs of [`LineVertex`] — one segment per pair — and this
//! expands each into a six-vertex screen-space quad that `shaders/trace.wgsl`
//! widens and anti-aliases. Hardware `LineList` primitives were the obvious
//! thing and were what this did first, but wgpu gives them no width and no
//! coverage: a 1px aliased hairline over a point cloud made of 1px aliased
//! points reads as more of the point cloud. The expansion costs 3x the
//! vertices of geometry that is at most a few thousand segments.
//!
//! Alpha-blended over the point cloud with read-only depth (these overlay,
//! never occlude).

use bytemuck::{Pod, Zeroable};

use crate::gpu::buffers::{create_camera_buffer, CameraUniforms};
use crate::gpu::points::DEPTH_FORMAT;

/// One endpoint of a line segment, as callers build them: two per segment.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct LineVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

/// One corner of the expanded ribbon, as the GPU sees it: this endpoint, the
/// other endpoint (for the screen-space direction), and which side to offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RibbonVertex {
    position: [f32; 3],
    color: [f32; 4],
    other: [f32; 3],
    side: f32,
}

pub struct LineRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_count: u32,
}

impl LineRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trace_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/trace.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trace_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trace_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trace_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RibbonVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 28,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 40,
                            shader_location: 3,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let camera_buffer = create_camera_buffer(device);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trace_bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            bind_group,
            camera_buffer,
            vertex_buffer: None,
            vertex_count: 0,
        }
    }

    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Replace the line geometry. `vertices` is pairs — two per segment —
    /// which this expands into the ribbon quads the shader widens.
    pub fn set_lines(&mut self, device: &wgpu::Device, vertices: &[LineVertex]) {
        use wgpu::util::DeviceExt;
        let ribbon = expand(vertices);
        self.vertex_count = ribbon.len() as u32;
        self.vertex_buffer = if ribbon.is_empty() {
            None
        } else {
            Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("line_ribbon_vertices"),
                contents: bytemuck::cast_slice(&ribbon),
                usage: wgpu::BufferUsages::VERTEX,
            }))
        };
    }

    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let Some(vb) = &self.vertex_buffer else { return };
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.set_vertex_buffer(0, vb.slice(..));
        render_pass.draw(0..self.vertex_count, 0..1);
    }
}

/// Expand `[a, b, a, b, ...]` endpoint pairs into two triangles per segment.
///
/// Corner order is (a-, a+, b-) then (b-, a+, b+): each corner carries its own
/// endpoint plus the opposite one, so the vertex shader can work out the
/// screen-space direction without an index buffer or a storage binding. An odd
/// trailing vertex is dropped — half a segment has no direction.
fn expand(vertices: &[LineVertex]) -> Vec<RibbonVertex> {
    let mut out = Vec::with_capacity(vertices.len() / 2 * 6);
    for pair in vertices.chunks_exact(2) {
        let (a, b) = (pair[0], pair[1]);
        let corner = |v: LineVertex, other: LineVertex, side: f32| RibbonVertex {
            position: v.position,
            color: v.color,
            other: other.position,
            side,
        };
        out.extend_from_slice(&[
            corner(a, b, -1.0),
            corner(a, b, 1.0),
            corner(b, a, -1.0),
            corner(b, a, -1.0),
            corner(a, b, 1.0),
            corner(b, a, 1.0),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32) -> LineVertex {
        LineVertex { position: [x, 0.0, 0.0], color: [1.0, 1.0, 1.0, 1.0] }
    }

    #[test]
    fn each_segment_becomes_two_triangles() {
        let out = expand(&[v(0.0), v(1.0), v(2.0), v(3.0)]);
        assert_eq!(out.len(), 12);
        // Every corner knows both ends, which is what lets the shader find the
        // screen-space perpendicular.
        assert_eq!(out[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(out[0].other, [1.0, 0.0, 0.0]);
        assert_eq!(out[2].position, [1.0, 0.0, 0.0]);
        assert_eq!(out[2].other, [0.0, 0.0, 0.0]);
        // Both sides of the ribbon are present, twice each (the shared edge).
        let plus = out[..6].iter().filter(|c| c.side > 0.0).count();
        assert_eq!(plus, 3);
    }

    #[test]
    fn an_odd_trailing_endpoint_is_dropped() {
        // Half a segment has no direction to be perpendicular to; better to
        // drop it than to emit a quad pointing nowhere.
        assert!(expand(&[v(0.0)]).is_empty());
        assert_eq!(expand(&[v(0.0), v(1.0), v(2.0)]).len(), 6);
    }
}
