//! Gizmo renderer: unit right-angled tetrahedron for each IFS transform
//!
//! Renders a reference (identity) tetrahedron in grey plus one colored
//! tetrahedron per transform, showing where the transform maps the unit shape.
//!
//! ## Colour
//!
//! Still six colours — RGB on the three shafts, CMY on the three far edges —
//! because that is what the geometry is: three axes, and three parts spanning
//! a pair of them. What changed is where those six come from. They used to be
//! the corners of the RGB cube, all as loud as the display can be, and with
//! several transforms on screen the result was a cyan/magenta/yellow test card
//! laid over the artwork a dozen triangles deep. Now they are one family: three
//! muted axis hues shared with the rest of the interface, and three secondaries
//! *derived* from them at the same lightness and colourfulness. See
//! [`crate::palette::axes`].
//!
//! The other two changes:
//!
//! - **Faces gradient, edges don't.** A face runs from mostly-neutral at the
//!   origin corner every face shares out to each axis's own hue at that axis's
//!   corner, so a hue is stated where it identifies something. Edges are flat:
//!   two pixels is nowhere to put a transition, and a line that changes colour
//!   along its length reads as unsure which axis it belongs to.
//! - **The faces stopped being uniformly loud.** They carry the area, so the
//!   opacity is spent by rank rather than spread evenly — see [`face_alpha`].
//!   The selected transform gets *more* than the old flat value and everything
//!   else gets a wash, which turns the biggest thing on screen into the answer
//!   to "which one am I editing".
//!
//! See [`Style`] for how the three variants are laid out in the vertex buffer.
//!
//! Uses two pipelines to avoid z-fighting and occlusion issues:
//! 1. Edges + dots: depth-write ON (always visible)
//! 2. Faces: depth-write OFF (semi-transparent, blend over everything)

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::gpu::buffers::{create_camera_buffer, CameraUniforms};
use crate::gpu::points::DEPTH_FORMAT;
use crate::palette::axes;

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
/// The three tips, indexed by axis, so a colour and a position can be looked
/// up by the same `k`.
const TIP: [[f32; 3]; 3] = [X, Y, Z];

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

/// Which tetrahedron a block of vertices belongs to.
///
/// Faces come in three; edges and dots in two. A selected gizmo already says
/// so with its tip handles, its x-ray copy, its roll ring and its label
/// backdrop — lighting its edges as well would be a fifth signal for one fact.
/// What it did *not* have was any claim on the biggest thing on screen, which
/// is what the face blocks are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    /// The identity tetrahedron, drawn once beside everything else.
    Reference,
    /// A transform you are not editing.
    Idle,
    /// The transform you are editing.
    Selected,
}

impl Style {
    fn is_reference(self) -> bool {
        self == Style::Reference
    }

    /// This style's hue for the corner on axis `k`, linear.
    fn tip_rgb(self, k: usize) -> Vec3 {
        if self.is_reference() {
            axes::reference_linear()
        } else {
            axes::axis_linear(k)
        }
    }

    /// This style's hue for the part spanning axes `ka` and `kb` — the far
    /// edge, and the direction a face's origin corner leans.
    fn pair_rgb(self, ka: usize, kb: usize) -> Vec3 {
        if self.is_reference() {
            axes::reference_linear()
        } else {
            axes::pair_linear(ka, kb)
        }
    }

    /// This style's colour at the origin — the corner all three axes share,
    /// and so the one corner that cannot belong to any of them.
    ///
    /// `toward` is the hue this particular part is heading for, which the
    /// origin end keeps a fraction of: pulled all the way to the neutral, a
    /// face had no colour at the one corner where all three of them overlap
    /// and so carried the most opacity, and read as a grey wedge with hues you
    /// had to hunt for out at the far edge.
    fn origin_rgb(self, toward: Vec3, keep: f32) -> Vec3 {
        if self.is_reference() {
            axes::reference_linear()
        } else {
            axes::neutral_linear().lerp(toward, keep)
        }
    }
}

/// Face opacity.
///
/// Where the weight went. The faces used to be a flat 0.15 for every transform
/// at the full-saturation secondaries, which by area was nearly the whole
/// gizmo — four transforms put twelve saturated triangles over the artwork.
/// The fix was never to make faces faint; it was to stop spending the same
/// opacity on all of them. The selected transform now gets *more* than the old
/// flat value, because it is one tetrahedron and it is the one you are working
/// on, and the rest get a wash.
///
/// Flat across the triangle rather than ramped. A ramp was tried both ways and
/// both are worse: heavier at the origin puts the opacity exactly where all
/// three faces overlap, and heavier at the tips puts it in the widest part of
/// the triangle, which is where the area is.
fn face_alpha(style: Style) -> f32 {
    match style {
        // A landmark, always drawn, never editable, and the same size as
        // everything else — so it sits with the idle transforms, not above
        // them.
        Style::Reference => 0.06,
        Style::Idle => 0.08,
        Style::Selected => 0.38,
    }
}

/// How much of its own hue a face keeps at the origin corner.
///
/// Hue grows outward across a face: mostly the interface neutral where all
/// three faces meet, and each axis's own colour at that axis's corner. Faces
/// can carry this and edges can't. A face is wide enough for a gradient to be
/// a gradient, and it is a *surface* — reading its shading as depth is what
/// eyes do anyway. An edge that changes colour along its length just looks
/// unsure which axis it belongs to, and a two-pixel line has nowhere to put
/// the transition.
const FACE_ROOT_KEEP: f32 = 0.55;

fn rgba(c: Vec3, a: f32) -> [f32; 4] {
    [c.x, c.y, c.z, a]
}

/// Build one style's three faces (9 vertices).
///
/// Colour is per *vertex* now rather than per face, which costs nothing —
/// the rasterizer interpolates it — and buys the whole gradient: each face
/// runs from a mostly-neutral origin corner out to the two axis hues at the
/// corners it spans. No face ever states a secondary hue flatly; the blend
/// between two axes appears only where the face is genuinely between them.
fn build_faces(style: Style) -> Vec<GizmoVertex> {
    let mut faces = Vec::with_capacity(FACE_VERTS as usize);
    let alpha = face_alpha(style);

    // The three faces, each named by the two axes it spans. Order is
    // [xy, yz, xz] to match the edge build order below.
    for (ka, kb) in [(0, 1), (1, 2), (0, 2)] {
        // A face's origin corner keeps a little of the secondary it belongs to
        // — enough to say which face it is where the face is most opaque, and
        // the same hue as the far edge closing it, so the triangle reads as one
        // part rather than two axes and a stripe.
        let origin = style.origin_rgb(style.pair_rgb(ka, kb), FACE_ROOT_KEEP);
        faces.push(GizmoVertex {
            position: O,
            color: rgba(origin, alpha),
            edge_other: [0.0; 3],
            vertex_type: 0,
        });
        for k in [ka, kb] {
            faces.push(GizmoVertex {
                position: TIP[k],
                color: rgba(style.tip_rgb(k), alpha),
                edge_other: [0.0; 3],
                vertex_type: 0,
            });
        }
    }

    assert_eq!(faces.len(), FACE_VERTS as usize);
    faces
}

/// Build one variant's edges and dots (60 vertices).
///
/// Two variants, not three: see [`Style`].
fn build_edges_dots(is_reference: bool) -> Vec<GizmoVertex> {
    let mut edges_dots = Vec::with_capacity(EDGE_DOT_VERTS as usize);
    let style = if is_reference { Style::Reference } else { Style::Idle };
    let edge_alpha: f32 = if is_reference { 0.6 } else { 1.0 };

    // === EDGES (36 vertices) ===
    //
    // Build order is load-bearing: the shader turns a vertex index into a part
    // id by dividing by six, so [ox oy oz xy yz xz] is the order `set_highlight`
    // and `pick.rs` agree on.
    //
    // Six edges, six flat colours: the three axis hues on the shafts, and on
    // the far triangle the secondary derived from the two axes each of those
    // edges spans (see [`axes::pair_linear`]). Structurally this is the palette
    // the gizmo always had — RGB inside, CMY outside — and it is the right one:
    // the far edges *are* the pairwise parts, and saying so with the pairwise
    // colours is what makes six parts identifiable at a glance. What was wrong
    // with it was only that all six sat at the corners of the RGB cube.
    //
    // Flat, not gradiented. A gradient down an edge was tried and it reads as
    // an edge unsure which axis it belongs to; there is no room in two pixels
    // for the transition to be anything but a smear.
    let mut edge_defs: Vec<([f32; 3], [f32; 3], Vec3)> = Vec::with_capacity(6);
    for k in 0..3 {
        edge_defs.push((O, TIP[k], style.tip_rgb(k)));
    }
    for (ka, kb) in [(0, 1), (1, 2), (0, 2)] {
        edge_defs.push((TIP[ka], TIP[kb], style.pair_rgb(ka, kb)));
    }

    for (a, b, rgb) in &edge_defs {
        let color_pos = rgba(*rgb, edge_alpha);
        let color_neg = rgba(*rgb, -edge_alpha);

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
    // The origin dot stays white on both variants. It is the one handle every
    // gizmo offers whether or not it is selected, and it belongs to no axis.
    let mut dots: Vec<([f32; 3], [f32; 4])> = vec![(O, [1.0, 1.0, 1.0, 1.0])];
    for k in 0..3 {
        dots.push((TIP[k], rgba(style.tip_rgb(k), 1.0)));
    }
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
    edges_dots
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

/// How faint the buried part of a gizmo reads, selected and otherwise.
///
/// Written **negative** into the per-instance alpha buffer, which is how the
/// shader tells "x-ray" from "disabled". Both are less than fully opaque, but
/// only the disabled one should desaturate — an x-ray gizmo is the same object
/// seen through something, so it keeps its colours.
///
/// *Every* transform gets one now, not just the selected one. The point cloud
/// writes depth and the gizmos test against it, so a dense attractor swallows
/// them whole — while `pick.rs`, which is pure screen-space projection with no
/// depth awareness at all, goes on offering them to the cursor. Showing only
/// the selected gizmo through the fractal fixed that for the transform you had
/// already found and left every other one invisible but clickable, which is the
/// worse half of the same bug: you cannot select what you cannot see.
///
/// Idle copies are fainter than the selected one and carry no tip handles, so
/// twenty transforms read as a faint wireframe behind the picture rather than
/// as twenty things competing with it.
const XRAY_ALPHA: f32 = 0.75;
const XRAY_ALPHA_IDLE: f32 = 0.45;

pub struct GizmoRenderer {
    /// Pipeline for edges + dots (depth write ON)
    edge_dot_pipeline: wgpu::RenderPipeline,
    /// Pipeline for faces (depth write OFF, read-only depth test)
    face_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
    /// Vertex buffer layout:
    /// [ref_edges_dots | xform_edges_dots | ref_faces | idle_faces | selected_faces]
    vertex_buffer: wgpu::Buffer,
    /// Instance 0 = identity reference, then one matrix per transform, then
    /// [`MAX_GHOSTS`] slots for the selected motif's symmetry orbit, then a
    /// second copy of the whole reference-plus-transforms run for the x-ray
    /// pass (see [`GizmoRenderer::xray_base`]).
    transform_buffer: wgpu::Buffer,
    /// Per-instance alpha multiplier (1.0 = full, <1.0 = greyed out)
    alpha_buffer: wgpu::Buffer,
    /// Hovered part uniform: [instance (0 = none), part id, 0, 0]
    highlight_buffer: wgpu::Buffer,
    /// Draws every transform's wireframe ignoring depth, so a buried gizmo is
    /// still visible. See [`GizmoRenderer::draw`].
    xray_pipeline: wgpu::RenderPipeline,
    /// Reference + one per transform. Ghosts live past this.
    instance_count: u32,
    /// Ghosts currently written, from `set_ghosts`.
    ghost_count: u32,
    /// Instance index of the selected transform, if any — the only instance
    /// that draws tip handles. See [`GizmoRenderer::draw`].
    selected_instance: Option<u32>,
    /// Which transforms are enabled, as last given to `update_alpha`. Kept
    /// because the x-ray alphas depend on both this and the selection, and the
    /// two arrive from different callers at different times.
    enabled: Vec<bool>,
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
        // transform, then the ghost slots (written by `set_ghosts`), then the
        // x-ray run — the same matrices again, drawn at a negative alpha
        // through whatever is in front of them.
        let instance_count = 1 + transforms.len() as u32;
        let slots = (2 * instance_count + MAX_GHOSTS) as usize;
        let mut matrices: Vec<[[f32; 4]; 4]> = Vec::with_capacity(slots);
        matrices.push(Mat4::IDENTITY.to_cols_array_2d());
        for spec in transforms {
            matrices.push(spec.matrix.to_cols_array_2d());
        }
        matrices.resize(slots, Mat4::IDENTITY.to_cols_array_2d());
        for i in 0..instance_count as usize {
            matrices[(instance_count + MAX_GHOSTS) as usize + i] = matrices[i];
        }
        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gizmo_transforms"),
            contents: bytemuck::cast_slice(&matrices),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Per-instance alpha (all 1.0 initially), except the x-ray run, which
        // is negative by definition — see `write_xray_alphas`, which owns it
        // from here on. Seeded here rather than through that method so `new`
        // doesn't need a queue it otherwise has no use for.
        let mut alphas = vec![1.0f32; slots];
        let xray_base = (instance_count + MAX_GHOSTS) as usize;
        alphas[xray_base] = 0.0; // the reference is never x-rayed
        for a in &mut alphas[xray_base + 1..xray_base + instance_count as usize] {
            *a = -XRAY_ALPHA_IDLE;
        }
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

        // Build vertex buffer. See `draw` for the offsets this layout implies:
        // [ref_edges_dots | xform_edges_dots | ref_faces | idle_faces | selected_faces]
        let mut all_verts = Vec::new();
        all_verts.extend_from_slice(&build_edges_dots(true));
        all_verts.extend_from_slice(&build_edges_dots(false));
        for style in [Style::Reference, Style::Idle, Style::Selected] {
            all_verts.extend_from_slice(&build_faces(style));
        }

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
            selected_instance: None,
            bind_group,
            camera_buffer,
            vertex_buffer,
            transform_buffer,
            alpha_buffer,
            highlight_buffer,
            instance_count,
            ghost_count: 0,
            enabled: vec![true; transforms.len()],
        }
    }

    /// First slot of the x-ray run: a second copy of the whole
    /// reference-plus-transforms run, past the ghosts.
    ///
    /// Slots rather than a new binding or a dynamic offset: the ghost mechanism
    /// already proves the pattern, and "draw the same geometry from another
    /// matrix at another alpha" is exactly what an instance slot is for. Costs
    /// one matrix and one float per transform.
    fn xray_base(&self) -> u32 {
        self.instance_count + MAX_GHOSTS
    }

    /// Push the x-ray run's per-instance alphas, which depend on both the
    /// selection and the enabled flags — two things that arrive from different
    /// callers at different times, so either changing rewrites the whole run.
    ///
    /// Negative, because that is how the shader tells an x-ray copy from a
    /// disabled transform; see [`XRAY_ALPHA`]. A *disabled* transform gets no
    /// x-ray copy at all: it is contributing nothing to the picture in front of
    /// it, so showing it through that picture is clutter, and the greyed solid
    /// gizmo still says where it is.
    fn write_xray_alphas(&self, queue: &wgpu::Queue) {
        let mut alphas = vec![0.0f32; self.instance_count as usize];
        for i in 1..self.instance_count {
            if !self.enabled.get(i as usize - 1).copied().unwrap_or(true) {
                continue;
            }
            alphas[i as usize] = if self.selected_instance == Some(i) {
                -XRAY_ALPHA
            } else {
                -XRAY_ALPHA_IDLE
            };
        }
        let stride = std::mem::size_of::<f32>() as u64;
        queue.write_buffer(
            &self.alpha_buffer,
            self.xray_base() as u64 * stride,
            bytemuck::cast_slice(&alphas),
        );
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

    /// Tell the x-ray pass which transform is selected, so it can be drawn
    /// through the fractal more strongly than the rest and keep its tip
    /// handles.
    ///
    /// Every transform is x-rayed; only the weight differs. This used to be the
    /// selected one alone, on the reasoning that always-on-top gizmos would
    /// defeat the G/Tab toggle and be soup at twenty transforms. The toggle
    /// argument doesn't hold — G removes the gizmos entirely, x-ray included —
    /// and the soup argument is answered by weight rather than by absence:
    /// idle copies are faint and carry no handles. What the old rule left
    /// behind was worse than soup, which was every unselected transform being
    /// invisible and clickable at the same time.
    ///
    /// `selected` is `(transform index, its matrix)`; the matrix is no longer
    /// needed here, since the x-ray run mirrors `update_transforms`.
    pub fn set_xray(&mut self, queue: &wgpu::Queue, selected: Option<(usize, Mat4)>) {
        // Instance 0 is the reference, so transform i is instance i + 1.
        self.selected_instance = selected.map(|(i, _)| i as u32 + 1);
        self.write_xray_alphas(queue);
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
    ///
    /// Written twice, to the solid run and to the x-ray copy of it. They are
    /// the same matrices by construction — an x-ray gizmo that lagged its own
    /// solid one by a frame would smear during exactly the drags this exists to
    /// make possible.
    pub fn update_transforms(&self, queue: &wgpu::Queue, transforms: &[crate::scene::TransformSpec]) {
        debug_assert_eq!(1 + transforms.len() as u32, self.instance_count);
        let mut matrices: Vec<[[f32; 4]; 4]> = Vec::with_capacity(self.instance_count as usize);
        matrices.push(Mat4::IDENTITY.to_cols_array_2d());
        for spec in transforms {
            matrices.push(spec.matrix.to_cols_array_2d());
        }
        queue.write_buffer(&self.transform_buffer, 0, bytemuck::cast_slice(&matrices));
        let stride = std::mem::size_of::<[[f32; 4]; 4]>() as u64;
        queue.write_buffer(
            &self.transform_buffer,
            self.xray_base() as u64 * stride,
            bytemuck::cast_slice(&matrices),
        );
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
    pub fn update_alpha(&mut self, queue: &wgpu::Queue, enabled: &[bool]) {
        let mut alphas = vec![1.0f32; self.instance_count as usize];
        for (i, &on) in enabled.iter().enumerate() {
            if !on {
                alphas[i + 1] = 0.25; // instance 0 is reference, transforms start at 1
            }
        }
        queue.write_buffer(&self.alpha_buffer, 0, bytemuck::cast_slice(&alphas));
        // The x-ray run is a separate write, past the ghosts, and it needs this
        // same enabled state — a disabled transform gets no x-ray copy.
        self.enabled = enabled.to_vec();
        self.write_xray_alphas(queue);
    }

    /// Draw gizmos in a render pass (should be called after the point cloud pass).
    ///
    /// Draw order: the x-ray pass first (no depth at all), then edges+dots
    /// (depth-write ON), then faces (depth-write OFF). X-ray goes first so the
    /// solid pass paints over it wherever the gizmo is genuinely visible — the
    /// gizmo you can see looks exactly as it always did, and only the part
    /// buried in the attractor shows through, faintly.
    ///
    /// Vertex buffer layout:
    /// [ref_edges_dots(60) | xform_edges_dots(60) | ref_faces(9) | idle_faces(9) | selected_faces(9)]
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

        // Pass 0: x-ray — every transform's wireframe, ignoring depth, so a
        // gizmo buried in a dense attractor is still visible and still
        // selectable. Weight comes from the per-instance alphas
        // (`write_xray_alphas`), not from what is drawn: the selected transform
        // reads stronger and is the only one to get its tip handles here, the
        // same split the solid pass makes below.
        let solid = ed + EDGE_VERTS + 6; // edges + the origin dot
        let xray = self.xray_base();
        render_pass.set_pipeline(&self.xray_pipeline);
        if self.instance_count > 1 {
            render_pass.draw(ed..solid, xray + 1..xray + self.instance_count);
        }
        if let Some(sel) = self.selected_instance {
            render_pass.draw(solid..ed * 2, xray + sel..xray + sel + 1);
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
        //
        // Three blocks, one per `Style`. The selected transform draws from the
        // shaded block and everything else from the near-transparent one, so
        // "which transform am I editing" is answered by the largest thing on
        // screen rather than by the tip dots alone. Drawing the idle block
        // under the selected one instead would have saved a draw call and cost
        // the exactness: two washes stacked is a third colour.
        let f_ref = ed * 2;
        let f_idle = f_ref + f;
        let f_sel = f_idle + f;
        render_pass.set_pipeline(&self.face_pipeline);
        render_pass.draw(f_ref..f_ref + f, 0..1); // reference
        if self.instance_count > 1 {
            let sel = self.selected_instance.filter(|s| (1..self.instance_count).contains(s));
            // The selected instance sits somewhere in the middle of the run, so
            // the idle block is drawn as the two ranges either side of it.
            let (below, above) = match sel {
                Some(s) => (1..s, s + 1..self.instance_count),
                None => (1..self.instance_count, 0..0),
            };
            for range in [below, above] {
                if !range.is_empty() {
                    render_pass.draw(f_idle..f_idle + f, range);
                }
            }
            if let Some(s) = sel {
                render_pass.draw(f_sel..f_sel + f, s..s + 1);
            }
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

    /// Every variant has to lay out identically: `draw` slices the vertex
    /// buffer by multiples of one block, so a block of a different size would
    /// shift every later variant's geometry by that difference.
    #[test]
    fn both_variants_fill_the_same_block() {
        for is_reference in [true, false] {
            let edges_dots = build_edges_dots(is_reference);
            assert_eq!(edges_dots.len(), EDGE_DOT_VERTS as usize, "reference={is_reference}");
        }
        for style in [Style::Reference, Style::Idle, Style::Selected] {
            assert_eq!(build_faces(style).len(), FACE_VERTS as usize, "{style:?}");
        }
    }

    /// The design in one assertion: the transform you are editing is the only
    /// one with shaded faces, and an idle gizmo's faces are a wash rather than
    /// a colour laid over the artwork.
    ///
    #[test]
    fn only_the_selected_transform_has_shaded_faces() {
        let idle = face_alpha(Style::Idle);
        let selected = face_alpha(Style::Selected);
        assert!(selected > idle * 3.0, "selected {selected} vs idle {idle}");
        // Both bounds are against the old flat 0.15 that *every* transform used
        // to get, which is the value the complaint was about. An idle gizmo has
        // to sit well under it, because several of them overlap in any busy
        // scene and they stack; the selected one has to sit above it, because
        // it is one tetrahedron and it is the one being edited.
        assert!(idle < 0.15 * 0.7, "an idle gizmo's faces must stay a wash, got {idle}");
        assert!(selected > 0.15, "the selected gizmo must be solider than the old flat value");
        // The reference is drawn in every scene and is never editable, so it
        // is quieter than a transform you might be about to select.
        assert!(face_alpha(Style::Reference) < selected);
    }

    /// No part of a gizmo states a full-saturation hue any more. The old
    /// palette's problem was not which six colours it picked but that it
    /// picked them at the corners of the RGB cube, where two of the three
    /// channels are pinned and the result is as loud as the display can be.
    #[test]
    fn nothing_is_drawn_at_full_saturation() {
        let mut verts = build_edges_dots(false);
        verts.extend(build_faces(Style::Idle));
        verts.extend(build_faces(Style::Selected));
        for v in &verts {
            let [r, g, b, _] = v.color;
            // The origin dot is deliberately white: it is a handle that belongs
            // to no axis, and white is the one "saturated" value that reads as
            // neutral rather than as a hue.
            if r == 1.0 && g == 1.0 && b == 1.0 {
                continue;
            }
            let lo = r.min(g).min(b);
            let hi = r.max(g).max(b);
            assert!(
                hi < 1.0 && (hi - lo) < 0.85,
                "gizmo colour {:?} is a cube corner",
                [r, g, b]
            );
        }
    }

    /// One edge, one colour, all the way along it. A gradient down an edge was
    /// tried and rejected by eye: two pixels is nowhere to put a transition,
    /// and the result reads as a line unsure which axis it belongs to.
    ///
    /// The check is on the *sign-stripped* colour, because an edge quad encodes
    /// which side of the line a vertex is on in the sign of its alpha.
    #[test]
    fn each_edge_is_one_flat_colour() {
        for is_reference in [true, false] {
            let verts = build_edges_dots(is_reference);
            for e in 0..6usize {
                let quad = &verts[e * 6..e * 6 + 6];
                for v in quad {
                    assert_eq!(v.vertex_type, 1, "edge {e} is edge geometry");
                    assert_eq!(
                        &v.color[..3],
                        &quad[0].color[..3],
                        "edge {e} (reference={is_reference}) must be one colour"
                    );
                    assert_eq!(v.color[3].abs(), quad[0].color[3].abs(), "edge {e} alpha");
                }
            }
        }
    }

    /// The three shafts wear the axis hues and the three far edges wear the
    /// secondary derived from the pair each of them spans — the RGB-inside,
    /// CMY-outside scheme the gizmo always had, now drawn from one family
    /// instead of from the corners of the colour cube.
    #[test]
    fn shafts_take_the_axes_and_far_edges_take_the_pairs() {
        let verts = build_edges_dots(false);
        for k in 0..3usize {
            assert_eq!(verts[k * 6].position, O, "shaft {k} starts at the origin");
            assert_eq!(verts[k * 6 + 1].position, TIP[k], "shaft {k} ends at its tip");
            assert_eq!(&verts[k * 6].color[..3], &axes::axis_linear(k).to_array()[..]);
        }
        for (e, (ka, kb)) in [(0, 1), (1, 2), (0, 2)].into_iter().enumerate() {
            let quad = &verts[(3 + e) * 6];
            assert_eq!(quad.position, TIP[ka], "far edge {e} runs from tip {ka}");
            assert_eq!(&quad.color[..3], &axes::pair_linear(ka, kb).to_array()[..]);
        }
    }

    /// The three tip dots are handles, and the identity tetrahedron has no
    /// handles — nothing about it can be dragged. Its tips are built degenerate
    /// so they rasterize nothing; alpha would not do, because this pipeline
    /// writes depth and an invisible quad would still occlude.
    #[test]
    fn the_reference_gizmo_draws_no_tip_handles() {
        let reference = build_edges_dots(true);
        let transform = build_edges_dots(false);
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

    /// A tip dot is the same colour as the end of the shaft it sits on, so the
    /// handle and the shaft leading to it read as one axis rather than two
    /// things that happen to be near each other. Compared against the shaft
    /// itself rather than against a written-down colour, so the two can't drift
    /// apart when the palette moves.
    #[test]
    fn tip_dots_match_their_axis_colours() {
        let verts = build_edges_dots(false);
        let dots: Vec<&GizmoVertex> = verts.iter().filter(|d| d.vertex_type == 2).collect();
        for k in 0..3usize {
            let dot = dots[6 * (k + 1)];
            // Vertex 1 of shaft k is its tip end (see the quad build order).
            let shaft_tip = verts[k * 6 + 1];
            assert_eq!(&dot.color[..3], &shaft_tip.color[..3], "tip {k} colour");
            assert_eq!(dot.position, TIP[k], "tip {k} position");
            // And that colour is still recognisably this axis's.
            let axis = axes::axis_linear(k).to_array();
            let dominant = axis.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
            assert_eq!(dominant, k, "axis {k} hue must lead on channel {k}");
            assert_eq!(
                dot.color[..3].iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0,
                k,
                "tip {k} must lead on the same channel as axis {k}"
            );
        }
    }
}
