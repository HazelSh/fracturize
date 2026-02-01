//! Gizmo renderer: unit right-angled tetrahedron for each IFS transform
//!
//! Renders a reference (identity) tetrahedron in grey plus one colored
//! tetrahedron per transform, showing where the transform maps the unit shape.
//!
//! Uses two pipelines to avoid z-fighting and occlusion issues:
//! 1. Edges + dots: depth-write ON (always visible)
//! 2. Faces: depth-write OFF (semi-transparent, blend over everything)

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::gpu::buffers::{create_camera_buffer, CameraUniforms};
use crate::gpu::points::DEPTH_FORMAT;

/// Vertex for gizmo geometry
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GizmoVertex {
    /// Position in unit-tetrahedron space (or endpoint A for edges)
    position: [f32; 3],
    /// RGBA color. For edge vertices, sign of alpha encodes side direction.
    color: [f32; 4],
    /// Other endpoint for edges, billboard corner for dots, unused for faces
    edge_other: [f32; 3],
    /// 0=face, 1=edge, 2=dot
    vertex_type: u32,
}

// Tetrahedron vertices
const O: [f32; 3] = [0.0, 0.0, 0.0];
const X: [f32; 3] = [1.0, 0.0, 0.0];
const Y: [f32; 3] = [0.0, 1.0, 0.0];
const Z: [f32; 3] = [0.0, 0.0, 1.0];

/// Vertex counts per gizmo variant
const FACE_VERTS: u32 = 9;   // 3 faces × 3 verts
const EDGE_VERTS: u32 = 36;  // 6 edges × 6 verts
const DOT_VERTS: u32 = 6;    // 1 dot billboard
const EDGE_DOT_VERTS: u32 = EDGE_VERTS + DOT_VERTS; // 42

/// Build gizmo vertices, returning (edges_and_dots, faces) separately
fn build_gizmo_vertices(is_reference: bool) -> (Vec<GizmoVertex>, Vec<GizmoVertex>) {
    let mut edges_dots = Vec::with_capacity(EDGE_DOT_VERTS as usize);
    let mut faces = Vec::with_capacity(FACE_VERTS as usize);

    let (face_xy, face_yz, face_xz): ([f32; 4], [f32; 4], [f32; 4]);
    let (edge_ox, edge_oy, edge_oz): ([f32; 3], [f32; 3], [f32; 3]);
    let (edge_xy, edge_yz, edge_xz): ([f32; 3], [f32; 3], [f32; 3]);
    let dot_color: [f32; 4];

    if is_reference {
        let grey_face = [0.5, 0.5, 0.5, 0.1];
        face_xy = grey_face;
        face_yz = grey_face;
        face_xz = grey_face;
        let grey_edge = [0.6, 0.6, 0.6];
        edge_ox = grey_edge;
        edge_oy = grey_edge;
        edge_oz = grey_edge;
        edge_xy = grey_edge;
        edge_yz = grey_edge;
        edge_xz = grey_edge;
        dot_color = [1.0, 1.0, 1.0, 1.0];
    } else {
        face_xy = [1.0, 1.0, 0.0, 0.15]; // Yellow
        face_yz = [0.0, 1.0, 1.0, 0.15]; // Cyan
        face_xz = [1.0, 0.0, 1.0, 0.15]; // Magenta
        edge_ox = [1.0, 0.0, 0.0]; // Red
        edge_oy = [0.0, 1.0, 0.0]; // Green
        edge_oz = [0.0, 0.0, 1.0]; // Blue
        edge_xy = [1.0, 1.0, 0.0]; // Yellow
        edge_yz = [0.0, 1.0, 1.0]; // Cyan
        edge_xz = [1.0, 0.0, 1.0]; // Magenta
        dot_color = [1.0, 1.0, 1.0, 1.0];
    }

    let edge_alpha: f32 = if is_reference { 0.6 } else { 0.8 };
    let zero3 = [0.0f32; 3];

    // === FACES (9 vertices) ===
    let face_defs: [([f32; 3], [f32; 3], [f32; 3], [f32; 4]); 3] = [
        (O, X, Y, face_xy),
        (O, Y, Z, face_yz),
        (O, X, Z, face_xz),
    ];
    for (a, b, c, color) in &face_defs {
        for pos in [a, b, c] {
            faces.push(GizmoVertex {
                position: *pos,
                color: *color,
                edge_other: zero3,
                vertex_type: 0,
            });
        }
    }

    // === EDGES (36 vertices) ===
    let edge_defs: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        (O, X, edge_ox),
        (O, Y, edge_oy),
        (O, Z, edge_oz),
        (X, Y, edge_xy),
        (Y, Z, edge_yz),
        (X, Z, edge_xz),
    ];
    for (a, b, rgb) in &edge_defs {
        let color_pos = [rgb[0], rgb[1], rgb[2], edge_alpha];
        let color_neg = [rgb[0], rgb[1], rgb[2], -edge_alpha];

        // 6 vertices: two triangles forming a screen-space quad
        // Triangle 1: A+, B+, A-
        // Triangle 2: A-, B+, B-
        edges_dots.push(GizmoVertex { position: *a, color: color_pos, edge_other: *b, vertex_type: 1 });
        edges_dots.push(GizmoVertex { position: *b, color: color_pos, edge_other: *a, vertex_type: 1 });
        edges_dots.push(GizmoVertex { position: *a, color: color_neg, edge_other: *b, vertex_type: 1 });
        edges_dots.push(GizmoVertex { position: *a, color: color_neg, edge_other: *b, vertex_type: 1 });
        edges_dots.push(GizmoVertex { position: *b, color: color_pos, edge_other: *a, vertex_type: 1 });
        edges_dots.push(GizmoVertex { position: *b, color: color_neg, edge_other: *a, vertex_type: 1 });
    }

    // === ORIGIN DOT (6 vertices) ===
    let corners: [[f32; 3]; 6] = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [-1.0, -1.0, 0.0],
    ];
    for corner in &corners {
        edges_dots.push(GizmoVertex {
            position: O,
            color: dot_color,
            edge_other: *corner,
            vertex_type: 2,
        });
    }

    assert_eq!(edges_dots.len(), EDGE_DOT_VERTS as usize);
    assert_eq!(faces.len(), FACE_VERTS as usize);
    (edges_dots, faces)
}

fn create_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    vertex_buffer_layout: wgpu::VertexBufferLayout<'_>,
    depth_write_enabled: bool,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[vertex_buffer_layout],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::SrcAlpha,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
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
            depth_write_enabled,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

pub struct GizmoRenderer {
    /// Pipeline for edges + dots (depth write ON)
    edge_dot_pipeline: wgpu::RenderPipeline,
    /// Pipeline for faces (depth write OFF, read-only depth test)
    face_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
    /// Vertex buffer layout: [ref_edges_dots | xform_edges_dots | ref_faces | xform_faces]
    vertex_buffer: wgpu::Buffer,
    /// Per-instance alpha multiplier (1.0 = full, <1.0 = greyed out)
    alpha_buffer: wgpu::Buffer,
    instance_count: u32,
}

impl GizmoRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        transforms: &[(Mat4, f32, f32, f32)],
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gizmo_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/gizmo.wgsl").into()),
        });

        // Build transform storage buffer: instance 0 = identity, then one per transform
        let instance_count = 1 + transforms.len() as u32;
        let mut matrices: Vec<[[f32; 4]; 4]> = Vec::with_capacity(instance_count as usize);
        matrices.push(Mat4::IDENTITY.to_cols_array_2d());
        for (mat, _, _, _) in transforms {
            matrices.push(mat.to_cols_array_2d());
        }
        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gizmo_transforms"),
            contents: bytemuck::cast_slice(&matrices),
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Per-instance alpha (all 1.0 initially)
        let alphas = vec![1.0f32; instance_count as usize];
        let alpha_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gizmo_alphas"),
            contents: bytemuck::cast_slice(&alphas),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Build vertex buffer: [ref_edges_dots | xform_edges_dots | ref_faces | xform_faces]
        let (ref_ed, ref_f) = build_gizmo_vertices(true);
        let (xform_ed, xform_f) = build_gizmo_vertices(false);

        let mut all_verts = Vec::new();
        all_verts.extend_from_slice(&ref_ed);
        all_verts.extend_from_slice(&xform_ed);
        all_verts.extend_from_slice(&ref_f);
        all_verts.extend_from_slice(&xform_f);

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gizmo_vertices"),
            contents: bytemuck::cast_slice(&all_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gizmo_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
            label: Some("gizmo_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let vertex_buffer_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GizmoVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 28, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 40, shader_location: 3 },
            ],
        };

        let edge_dot_pipeline = create_pipeline(
            device, format, &shader, &pipeline_layout,
            vertex_buffer_layout.clone(),
            true, "gizmo_edge_dot_pipeline",
        );
        let face_pipeline = create_pipeline(
            device, format, &shader, &pipeline_layout,
            vertex_buffer_layout,
            false, "gizmo_face_pipeline",
        );

        let camera_buffer = create_camera_buffer(device);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gizmo_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: transform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: alpha_buffer.as_entire_binding() },
            ],
        });

        Self {
            edge_dot_pipeline,
            face_pipeline,
            bind_group,
            camera_buffer,
            vertex_buffer,
            alpha_buffer,
            instance_count,
        }
    }

    /// Upload camera uniforms
    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Update per-instance alpha based on enabled state
    /// Instance 0 is the reference gizmo (always full alpha),
    /// instances 1..N correspond to transforms[0..N-1]
    pub fn update_alpha(&self, queue: &wgpu::Queue, enabled: &[bool]) {
        let mut alphas = vec![1.0f32; self.instance_count as usize];
        for (i, &on) in enabled.iter().enumerate() {
            if !on {
                alphas[i + 1] = 0.25; // instance 0 is reference, transforms start at 1
            }
        }
        queue.write_buffer(&self.alpha_buffer, 0, bytemuck::cast_slice(&alphas));
    }

    /// Draw gizmos in a render pass (should be called after the point cloud pass).
    ///
    /// Draw order: edges+dots first (depth-write ON), then faces (depth-write OFF).
    /// This ensures edges/dots are always visible and faces blend correctly.
    ///
    /// Vertex buffer layout: [ref_edges_dots(42) | xform_edges_dots(42) | ref_faces(9) | xform_faces(9)]
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_bind_group(0, &self.bind_group, &[]);

        let ed = EDGE_DOT_VERTS; // 42
        let f = FACE_VERTS;      // 9
        // Offsets in vertex buffer:
        //   ref_edges_dots:   0 .. 42
        //   xform_edges_dots: 42 .. 84
        //   ref_faces:        84 .. 93
        //   xform_faces:      93 .. 102

        // Pass 1: edges + dots (depth write ON)
        render_pass.set_pipeline(&self.edge_dot_pipeline);
        render_pass.draw(0..ed, 0..1); // reference
        if self.instance_count > 1 {
            render_pass.draw(ed..ed * 2, 1..self.instance_count); // transforms
        }

        // Pass 2: faces (depth write OFF — read-only depth test, no z-fighting)
        render_pass.set_pipeline(&self.face_pipeline);
        render_pass.draw(ed * 2..ed * 2 + f, 0..1); // reference
        if self.instance_count > 1 {
            render_pass.draw(ed * 2 + f..ed * 2 + f * 2, 1..self.instance_count); // transforms
        }
    }
}
