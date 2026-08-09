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
/// Dots, in build order: the origin, then the X, Y and Z endpoints. The three
/// endpoints are the scale handles; see [`crate::pick::GizmoPart::Tip`].
const DOT_COUNT: u32 = 4;
const DOT_VERTS: u32 = DOT_COUNT * 6; // billboard quads
/// One instance's worth of edges and dots.
///
/// **`shaders/gizmo.wgsl` carries this number too**, as the modulus that turns
/// a vertex index back into a part id, and the two must agree or every
/// highlight lands on the wrong part. `the_shader_agrees_about_the_vertex_block`
/// checks it. That is a test rather than a compile error — WGSL is opaque to
/// the Rust compiler — so it is the one place here where drift is caught late
/// rather than prevented.
const EDGE_DOT_VERTS: u32 = EDGE_VERTS + DOT_VERTS; // 60

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

    // === DOTS (4 billboards, 6 vertices each) ===
    // Build order is [origin, x, y, z] and the shader reads part ids straight
    // off it, so this order is load-bearing.
    let corners: [[f32; 3]; 6] = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [-1.0, -1.0, 0.0],
    ];
    let tip_alpha: f32 = 1.0;
    let dots: [([f32; 3], [f32; 4]); DOT_COUNT as usize] = [
        (O, dot_color),
        (X, [edge_ox[0], edge_ox[1], edge_ox[2], tip_alpha]),
        (Y, [edge_oy[0], edge_oy[1], edge_oy[2], tip_alpha]),
        (Z, [edge_oz[0], edge_oz[1], edge_oz[2], tip_alpha]),
    ];
    for (i, (pos, color)) in dots.iter().enumerate() {
        for corner in &corners {
            // The reference tetrahedron's endpoints are not handles — it is the
            // identity drawn for comparison, and nothing about it can be
            // dragged. Its tip dots are built degenerate (every corner at the
            // centre) so they rasterize no fragments at all. Alpha 0 would not
            // do: this pipeline writes depth, so a fully transparent quad would
            // still punch an invisible hole in whatever is behind it.
            let degenerate = is_reference && i > 0;
            edges_dots.push(GizmoVertex {
                position: *pos,
                color: *color,
                edge_other: if degenerate { [0.0; 3] } else { *corner },
                vertex_type: 2,
            });
        }
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
    samples: u32,
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
            depth_write_enabled: Some(depth_write_enabled),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
        multiview_mask: None,
        cache: None,
    })
}

/// The x-ray pipeline: same shader, same vertex layout, but it neither tests
/// nor writes depth.
///
/// This is what puts a buried gizmo back on screen. The point cloud writes
/// depth and the gizmos test against it, so a dense attractor hides them
/// completely — while `pick.rs`, which is pure screen-space projection with no
/// depth awareness at all, goes on offering them to the cursor. That gap is the
/// bug behind "zooming breaks horribly as things move to under the scrollwheel
/// without visibility": you were dragging things you could not see.
///
/// Depth *write* stays off as well as depth test. A pass that ignored depth but
/// still wrote it would stamp the gizmo's depth over the fractal and punch a
/// hole in everything drawn afterwards.
fn create_xray_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    vertex_buffer_layout: wgpu::VertexBufferLayout<'_>,
    samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gizmo_xray_pipeline"),
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
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
        multiview_mask: None,
        cache: None,
    })
}

/// Instance slots kept spare for symmetry orbit ghosts.
///
/// Sized for the largest group this can draw — icosahedral with a mirror, 120,
/// less the motif itself, which is drawn solid as instance `i + 1` already.
/// Fixed rather than grown to fit so that changing a group, or changing which
/// motif is selected, is a buffer write instead of a pipeline rebuild: these
/// are things you do by dragging.
const MAX_GHOSTS: u32 = 120;

/// One extra instance slot, past the ghosts, holding a copy of the selected
/// transform's matrix for the x-ray pass.
///
/// A slot rather than a new binding or a dynamic offset: the ghost mechanism
/// already proves the pattern, and "draw the same geometry from another matrix
/// at another alpha" is exactly what an instance slot is for. Costs one matrix
/// and one float.
const XRAY_SLOT: u32 = 1;

/// How faint the buried part of the selected gizmo reads.
///
/// Low enough that it can't be mistaken for the real thing sitting in front of
/// the fractal, high enough to aim at.
const XRAY_ALPHA: f32 = 0.3;

pub struct GizmoRenderer {
    /// Pipeline for edges + dots (depth write ON)
    edge_dot_pipeline: wgpu::RenderPipeline,
    /// Pipeline for faces (depth write OFF, read-only depth test)
    face_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
    /// Vertex buffer layout: [ref_edges_dots | xform_edges_dots | ref_faces | xform_faces]
    vertex_buffer: wgpu::Buffer,
    /// Instance 0 = identity reference, then one matrix per transform, then
    /// [`MAX_GHOSTS`] slots for the selected motif's symmetry orbit.
    transform_buffer: wgpu::Buffer,
    /// Per-instance alpha multiplier (1.0 = full, <1.0 = greyed out)
    alpha_buffer: wgpu::Buffer,
    /// Hovered part uniform: [instance (0 = none), part id, 0, 0]
    highlight_buffer: wgpu::Buffer,
    /// Draws the selected gizmo, and every origin dot, ignoring depth so a
    /// buried gizmo is still visible. See [`GizmoRenderer::draw`].
    xray_pipeline: wgpu::RenderPipeline,
    /// Reference + one per transform. Ghosts live past this.
    instance_count: u32,
    /// Ghosts currently written, from `set_ghosts`.
    ghost_count: u32,
    /// Whether the x-ray slot currently holds a transform worth drawing.
    xray_active: bool,
    /// Instance index of the selected transform, if any — the only instance
    /// that draws tip handles. See [`GizmoRenderer::draw`].
    selected_instance: Option<u32>,
}

impl GizmoRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
        transforms: &[crate::scene::TransformSpec],
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gizmo_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/gizmo.wgsl").into()),
        });

        // Build transform storage buffer: instance 0 = identity, then one per
        // transform, then the ghost slots (written by `set_ghosts`).
        let instance_count = 1 + transforms.len() as u32;
        let slots = (instance_count + MAX_GHOSTS + XRAY_SLOT) as usize;
        let mut matrices: Vec<[[f32; 4]; 4]> = Vec::with_capacity(slots);
        matrices.push(Mat4::IDENTITY.to_cols_array_2d());
        for spec in transforms {
            matrices.push(spec.matrix.to_cols_array_2d());
        }
        matrices.resize(slots, Mat4::IDENTITY.to_cols_array_2d());
        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gizmo_transforms"),
            contents: bytemuck::cast_slice(&matrices),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Per-instance alpha (all 1.0 initially)
        let alphas = vec![1.0f32; slots];
        let alpha_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gizmo_alphas"),
            contents: bytemuck::cast_slice(&alphas),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Hover highlight (instance 0 = nothing hovered)
        let highlight_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gizmo_highlight"),
            contents: bytemuck::cast_slice(&[0u32; 4]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gizmo_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
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
            true, samples, "gizmo_edge_dot_pipeline",
        );
        let face_pipeline = create_pipeline(
            device, format, &shader, &pipeline_layout,
            vertex_buffer_layout.clone(),
            false, samples, "gizmo_face_pipeline",
        );
        let xray_pipeline = create_xray_pipeline(
            device, format, &shader, &pipeline_layout, vertex_buffer_layout, samples,
        );

        let camera_buffer = create_camera_buffer(device);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gizmo_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: transform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: alpha_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: highlight_buffer.as_entire_binding() },
            ],
        });

        Self {
            edge_dot_pipeline,
            face_pipeline,
            xray_pipeline,
            xray_active: false,
            selected_instance: None,
            bind_group,
            camera_buffer,
            vertex_buffer,
            transform_buffer,
            alpha_buffer,
            highlight_buffer,
            instance_count,
            ghost_count: 0,
        }
    }

    /// Draw the selected motif's symmetry orbit as dimmed copies of its gizmo.
    ///
    /// This is what makes a live group visible: the motif's `|G| − 1` images
    /// under its own group, drawn faint beside the solid one. It costs nothing
    /// but instances — the matrices are `g · matrix`, the same product the
    /// chaos game composes, so the ghosts follow a drag for free and can never
    /// disagree with what the walk is doing.
    ///
    /// Element 0 of a group is the identity, so the caller passes the orbit
    /// *without* it: instance `i + 1` is already drawn solid there.
    ///
    /// Ghosts are deliberately not pickable (`pick.rs` never sees them). A
    /// ghost is not an editable object — it is an image of one — so clicking
    /// one would select something you cannot change.
    pub fn set_ghosts(&mut self, queue: &wgpu::Queue, ghosts: &[Mat4]) {
        let n = ghosts.len().min(MAX_GHOSTS as usize);
        self.ghost_count = n as u32;
        if n == 0 {
            return;
        }
        let matrices: Vec<[[f32; 4]; 4]> =
            ghosts[..n].iter().map(|m| m.to_cols_array_2d()).collect();
        let offset = self.instance_count as u64 * std::mem::size_of::<[[f32; 4]; 4]>() as u64;
        queue.write_buffer(&self.transform_buffer, offset, bytemuck::cast_slice(&matrices));

        // Faint, and fainter the more of them there are: five ghosts can each
        // be a legible arrow, sixty would be a ball of wool at that weight.
        let alpha = if n <= 8 { 0.34 } else { 0.10 + 1.9 / n as f32 };
        let alphas = vec![alpha; n];
        let offset = self.instance_count as u64 * std::mem::size_of::<f32>() as u64;
        queue.write_buffer(&self.alpha_buffer, offset, bytemuck::cast_slice(&alphas));
    }

    /// Point the x-ray slot at the selected transform's matrix, or clear it.
    ///
    /// Only the selected one. Making every gizmo always-on-top would defeat the
    /// G/Tab toggle that exists so the art can be looked at without wireframe
    /// over it — and at twenty transforms it would be soup. The one you are
    /// working on is the one you need to see through the fractal.
    /// `selected` is `(transform index, its matrix)`.
    pub fn set_xray(&mut self, queue: &wgpu::Queue, selected: Option<(usize, Mat4)>) {
        self.xray_active = selected.is_some();
        // Instance 0 is the reference, so transform i is instance i + 1.
        self.selected_instance = selected.map(|(i, _)| i as u32 + 1);
        let Some((_, m)) = selected else { return };
        let slot = self.instance_count + MAX_GHOSTS;

        let stride = std::mem::size_of::<[[f32; 4]; 4]>() as u64;
        queue.write_buffer(
            &self.transform_buffer,
            slot as u64 * stride,
            bytemuck::cast_slice(&[m.to_cols_array_2d()]),
        );
        let stride = std::mem::size_of::<f32>() as u64;
        queue.write_buffer(
            &self.alpha_buffer,
            slot as u64 * stride,
            bytemuck::cast_slice(&[XRAY_ALPHA]),
        );
    }

    /// Set (or clear) the highlighted gizmo part. Instance 0 is the reference
    /// gizmo, so transform i maps to instance i+1.
    ///
    /// `held` separates "the pointer is over this" from "you have hold of this".
    /// They used to render identically, and not by choice: `App::update_hover`
    /// is the only writer and is deliberately not called during a drag, because
    /// by then the pointer has left the part and recomputing would un-highlight
    /// the very thing being dragged. So "held" was only ever "a hover that
    /// stopped being recomputed", with no feedback at the instant a press
    /// landed. Now the grab says so itself.
    pub fn set_highlight(
        &self,
        queue: &wgpu::Queue,
        hover: Option<(usize, crate::pick::GizmoPart)>,
        held: bool,
    ) {
        use crate::pick::GizmoPart;
        // Part ids follow the vertex build order: edges [ox oy oz xy yz xz],
        // then the dots [origin, x, y, z] as 6..9.
        let data: [u32; 4] = match hover {
            Some((t, part)) => {
                let part_id = match part {
                    GizmoPart::Axis(k) => k as u32,
                    GizmoPart::RotEdge(2) => 3, // x-y edge rotates around z
                    GizmoPart::RotEdge(0) => 4, // y-z edge rotates around x
                    GizmoPart::RotEdge(_) => 5, // x-z edge rotates around y
                    GizmoPart::Origin => 6,
                    GizmoPart::Tip(k) => 7 + k as u32,
                    // The ring isn't part of the tetrahedron geometry -- it is
                    // painted in screen space by `ui::gizmo_ring`, which reads
                    // the hover state from `App::hovered` directly. Nothing in
                    // this vertex buffer should light up for it.
                    GizmoPart::Roll => u32::MAX,
                };
                [(t + 1) as u32, part_id, held as u32, 0]
            }
            None => [0, 0xFFFF, 0, 0],
        };
        queue.write_buffer(&self.highlight_buffer, 0, bytemuck::cast_slice(&data));
    }

    /// Re-upload transform matrices after live edits. The transform count
    /// must match construction (rebuild the renderer when it changes).
    pub fn update_transforms(&self, queue: &wgpu::Queue, transforms: &[crate::scene::TransformSpec]) {
        debug_assert_eq!(1 + transforms.len() as u32, self.instance_count);
        let mut matrices: Vec<[[f32; 4]; 4]> = Vec::with_capacity(self.instance_count as usize);
        matrices.push(Mat4::IDENTITY.to_cols_array_2d());
        for spec in transforms {
            matrices.push(spec.matrix.to_cols_array_2d());
        }
        queue.write_buffer(&self.transform_buffer, 0, bytemuck::cast_slice(&matrices));
    }

    /// Upload camera uniforms
    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Update per-instance alpha based on enabled state
    /// Instance 0 is the reference gizmo (always full alpha),
    /// instances 1..N correspond to transforms[0..N-1]
    /// Writes only the reference-plus-transforms run, deliberately: the ghost
    /// slots past it carry their own alpha from `set_ghosts`, and a full-length
    /// write here would reset them to opaque on every enable/disable.
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
    /// Draw order: the x-ray pass first (no depth at all), then edges+dots
    /// (depth-write ON), then faces (depth-write OFF). X-ray goes first so the
    /// solid pass paints over it wherever the gizmo is genuinely visible — the
    /// gizmo you can see looks exactly as it always did, and only the part
    /// buried in the attractor shows through, faintly.
    ///
    /// Vertex buffer layout: [ref_edges_dots(60) | xform_edges_dots(60) | ref_faces(9) | xform_faces(9)]
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_bind_group(0, &self.bind_group, &[]);

        let ed = EDGE_DOT_VERTS; // 60
        let f = FACE_VERTS;      // 9
        // Offsets in vertex buffer:
        //   ref_edges_dots:   0 .. 60
        //   xform_edges_dots: 60 .. 120
        //   ref_faces:        120 .. 129
        //   xform_faces:      129 .. 138

        // Pass 0: x-ray. Two draws, both ignoring depth.
        render_pass.set_pipeline(&self.xray_pipeline);
        if self.instance_count > 1 {
            // Every transform's origin dot, at its normal alpha. The origin dot
            // is the one part of an unselected gizmo you can grab, so it is the
            // one part that must never be invisible while still taking clicks.
            let dot = ed + EDGE_VERTS;
            render_pass.draw(dot..dot + 6, 1..self.instance_count);
        }
        if self.xray_active {
            // The selected transform's whole gizmo, faint.
            let slot = self.instance_count + MAX_GHOSTS;
            render_pass.draw(ed..ed * 2, slot..slot + 1);
        }

        // Pass 1: edges + dots (depth write ON)
        //
        // Tip handles are split off from the rest and drawn only for the
        // selected transform, because they are only *grabbable* on the selected
        // transform (`pick::pick_gizmo`). Drawing a handle on something that
        // won't respond to it is the same lie the reference tetrahedron avoids
        // by building its tip dots degenerate — the difference is only that
        // this one changes with the selection, so it's a draw range rather than
        // geometry.
        let solid = ed + EDGE_VERTS + 6; // edges + the origin dot
        render_pass.set_pipeline(&self.edge_dot_pipeline);
        // The reference keeps its full range: its tip dots are degenerate
        // geometry, so it shows no handles for its own reason (one that a test
        // can check without a GPU) rather than by sharing this one.
        render_pass.draw(0..ed, 0..1); // reference
        if self.instance_count > 1 {
            render_pass.draw(ed..solid, 1..self.instance_count); // transforms
        }
        if let Some(sel) = self.selected_instance {
            render_pass.draw(solid..ed * 2, sel..sel + 1);
        }
        // Symmetry orbit ghosts, using the transforms' own geometry so a copy
        // reads as the same object. Edges and dots only, no faces: sixty
        // translucent faces stacked through each other is a fog, and it is the
        // *placement* of a copy you need to see, which the edges give.
        if self.ghost_count > 0 {
            let first = self.instance_count;
            // No tip handles on a ghost either — a ghost is an image of a map,
            // not an editable one, and `pick.rs` never sees them.
            render_pass.draw(ed..solid, first..first + self.ghost_count);
        }

        // Pass 2: faces (depth write OFF — read-only depth test, no z-fighting)
        render_pass.set_pipeline(&self.face_pipeline);
        render_pass.draw(ed * 2..ed * 2 + f, 0..1); // reference
        if self.instance_count > 1 {
            render_pass.draw(ed * 2 + f..ed * 2 + f * 2, 1..self.instance_count); // transforms
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader turns a vertex index back into a part id by taking it modulo
    /// one instance's block of edge+dot vertices. That number lives in two
    /// languages, and if they disagree every highlight silently lands on the
    /// wrong part — the gizmo would glow somewhere you aren't pointing.
    ///
    /// WGSL is opaque to the Rust compiler, so this can't be a build error the
    /// way an exhaustive match is. It is the residual gap, and this test is
    /// what stands in for it.
    #[test]
    fn the_shader_agrees_about_the_vertex_block() {
        let src = include_str!("../../shaders/gizmo.wgsl");
        let modulus = format!("% {}u", EDGE_DOT_VERTS);
        assert!(
            src.contains(&modulus),
            "gizmo.wgsl must take vertex_index {modulus} to match EDGE_DOT_VERTS"
        );
        // The dot block starts where the edges end, and the shader offsets into
        // it by that same number.
        let dot_start = format!("{}u", EDGE_VERTS);
        assert!(
            src.contains(&dot_start),
            "gizmo.wgsl must split edges from dots at {dot_start}"
        );
    }

    /// Both variants have to lay out identically: `draw` slices the vertex
    /// buffer by multiples of one block, so a reference block of a different
    /// size would shift every transform's geometry by that difference.
    #[test]
    fn both_variants_fill_the_same_block() {
        for is_reference in [true, false] {
            let (edges_dots, faces) = build_gizmo_vertices(is_reference);
            assert_eq!(edges_dots.len(), EDGE_DOT_VERTS as usize, "reference={is_reference}");
            assert_eq!(faces.len(), FACE_VERTS as usize, "reference={is_reference}");
        }
    }

    /// The three tip dots are handles, and the identity tetrahedron has no
    /// handles — nothing about it can be dragged. Its tips are built degenerate
    /// so they rasterize nothing; alpha would not do, because this pipeline
    /// writes depth and an invisible quad would still occlude.
    #[test]
    fn the_reference_gizmo_draws_no_tip_handles() {
        let (reference, _) = build_gizmo_vertices(true);
        let (transform, _) = build_gizmo_vertices(false);
        let dots = |v: &[GizmoVertex]| -> Vec<GizmoVertex> {
            v.iter().filter(|d| d.vertex_type == 2).copied().collect()
        };

        let ref_dots = dots(&reference);
        let xform_dots = dots(&transform);
        assert_eq!(ref_dots.len(), DOT_VERTS as usize);
        assert_eq!(xform_dots.len(), DOT_VERTS as usize);

        // The origin dot (first six) is real in both.
        assert!(
            ref_dots[..6].iter().any(|d| d.edge_other != [0.0; 3]),
            "the reference origin dot must still be drawn"
        );
        // Every tip corner past it collapses to a point.
        assert!(
            ref_dots[6..].iter().all(|d| d.edge_other == [0.0; 3]),
            "reference tip dots must be degenerate"
        );
        assert!(
            xform_dots[6..].iter().any(|d| d.edge_other != [0.0; 3]),
            "a transform's tip dots must actually be drawn"
        );
    }

    /// Tip dots wear their axis's colour, so the handle and the shaft it ends
    /// read as the same axis.
    #[test]
    fn tip_dots_match_their_axis_colours() {
        let (verts, _) = build_gizmo_vertices(false);
        let dots: Vec<&GizmoVertex> = verts.iter().filter(|d| d.vertex_type == 2).collect();
        for (i, expected) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].iter().enumerate() {
            let dot = dots[6 * (i + 1)];
            assert_eq!(&dot.color[..3], expected, "tip {i} colour");
            assert_eq!(dot.position, [expected[0], expected[1], expected[2]], "tip {i} position");
        }
    }
}
