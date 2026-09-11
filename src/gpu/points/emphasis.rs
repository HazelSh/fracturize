//! The emphasis pass: a glow over the material the selected transform moves.
//!
//! Two passes over whichever frame the point or splat renderer produced — one
//! that accumulates sensitivity and coverage per pixel, one that composites
//! them — so that lighting up the sensitive material costs neither render mode
//! a line of shader and costs nothing at all when nothing is selected. What it
//! draws is measured by the chaos pass (see the emphasis block in
//! `shaders/points/chaos.wgsl`) and normalized against the mean over the same
//! points, which `PointCompute::dispatch_emphasis_stats` re-measures each frame.
//!
//! `shaders/points/emphasis.wgsl` carries the reasoning for why a *mean* is the
//! only summary of a pixel that works here; it is the heart of the thing.

use bytemuck::{Pod, Zeroable};
use wgpu::{BindGroup, Buffer, Device, RenderPipeline, TextureFormat};

use crate::gpu::buffers::{create_camera_buffer, CameraUniforms};

/// Must match `EmphasisParams` in `shaders/points/emphasis.wgsl` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct EmphasisParams {
    pub run_start: u32,
    pub run_len: u32,
    pub capacity: u32,
    pub knee: f32,
    pub strength: f32,
    pub min_pixels: f32,
    pub size_scale: f32,
    /// std140 puts the `vec3` below on a 16-byte boundary, and seven scalars
    /// do not reach one.
    pub _pad0: f32,
    pub color: [f32; 3],
    pub dim: f32,
    pub background: [f32; 3],
    pub _pad: f32,
}

/// Multiple of the mean sensitivity at which a point is half lit.
///
/// The populations being separated are one application of the transform apart,
/// so the useful knee sits a little above the mean: at 1.0 the whole attractor
/// glows faintly and the answer to "what moves" is "everything".
pub const DEFAULT_KNEE: f32 = 1.5;

/// A cool white: the glow reads as light rather than as paint, and the slight
/// blue keeps it from being mistaken for the scene's own highlights.
pub const GLOW_COLOR: [f32; 3] = [0.80, 0.90, 1.00];

/// The accumulator's format. Float, because it holds a sum of weights with no
/// bound worth guessing, and the ratio of two of its channels is the answer.
const ACCUM_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// The accumulator runs at half the frame's width and height.
///
/// The glow is a soft field over a cloud of millions of dots — it has no detail
/// at the pixel level to lose — and this pass is one quad per point, so what it
/// costs is fragments. A quarter of them, for a difference that needs a
/// screenshot and a difference blend to find. Point sizes are in NDC by the
/// time they reach the rasterizer, so nothing has to be scaled to suit it.
const ACCUM_DIVISOR: u32 = 2;

impl Default for EmphasisParams {
    fn default() -> Self {
        Self {
            run_start: 0,
            run_len: 0,
            capacity: 1,
            knee: DEFAULT_KNEE,
            strength: 1.0,
            // A point can be subpixel and still be the most sensitive thing in
            // the frame, so the footprint has a floor the dots themselves do not.
            min_pixels: 1.5,
            size_scale: 1.2,
            _pad0: 0.0,
            color: GLOW_COLOR,
            dim: 0.7,
            background: [0.0; 3],
            _pad: 0.0,
        }
    }
}

struct Accumulator {
    view: wgpu::TextureView,
    bind_group: BindGroup,
    width: u32,
    height: u32,
}

pub struct EmphasisRenderer {
    accum_pipeline: RenderPipeline,
    composite_pipeline: RenderPipeline,
    layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    bind_group: BindGroup,
    camera_buffer: Buffer,
    params_buffer: Buffer,
    sampler: wgpu::Sampler,
    accum: Option<Accumulator>,
    /// Size of the sensitivity buffer the bind group was built against. The
    /// real one is allocated the first time a transform is emphasized, long
    /// after this renderer exists, and this is how the swap is noticed.
    bound_size: u64,
}

impl EmphasisRenderer {
    pub fn new(
        device: &Device,
        format: TextureFormat,
        point_buffer: &Buffer,
        sensitivity_buffer: &Buffer,
        stats_buffer: &Buffer,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("emphasis_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/points/emphasis.wgsl").into(),
            ),
        });

        let storage = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("emphasis_bind_group_layout"),
            entries: &[
                storage(0),
                uniform(1, wgpu::ShaderStages::VERTEX),
                storage(2),
                storage(3),
                uniform(4, wgpu::ShaderStages::VERTEX_FRAGMENT),
            ],
        });

        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("emphasis_composite_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let accum_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("emphasis_accum_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("emphasis_composite_pipeline_layout"),
                bind_group_layouts: &[Some(&layout), Some(&composite_layout)],
                immediate_size: 0,
            });

        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };

        let accum_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("emphasis_accum_pipeline"),
            layout: Some(&accum_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_glow"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_accumulate"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ACCUM_FORMAT,
                    blend: Some(wgpu::BlendState { color: additive, alpha: additive }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            // No depth. Both halves of the ratio have to come from the same
            // population, and a depth test would keep the near points and drop
            // the far ones from the denominator only.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("emphasis_composite_pipeline"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_composite"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_composite"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Premultiplied: `fs_composite` writes the receding and the
                    // lighting as one colour and one alpha.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::COLOR,
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
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let camera_buffer = create_camera_buffer(device);
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("emphasis_params_buffer"),
            size: std::mem::size_of::<EmphasisParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("emphasis_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group = Self::make_bind_group(
            device,
            &layout,
            point_buffer,
            &camera_buffer,
            sensitivity_buffer,
            stats_buffer,
            &params_buffer,
        );

        Self {
            accum_pipeline,
            composite_pipeline,
            layout,
            composite_layout,
            bind_group,
            camera_buffer,
            params_buffer,
            sampler,
            accum: None,
            bound_size: sensitivity_buffer.size(),
        }
    }

    fn make_bind_group(
        device: &Device,
        layout: &wgpu::BindGroupLayout,
        point_buffer: &Buffer,
        camera_buffer: &Buffer,
        sensitivity_buffer: &Buffer,
        stats_buffer: &Buffer,
        params_buffer: &Buffer,
    ) -> BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("emphasis_bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: point_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: sensitivity_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry { binding: 3, resource: stats_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: params_buffer.as_entire_binding() },
            ],
        })
    }

    /// Pick up the real sensitivity buffer once the compute side has allocated
    /// it. A no-op on every frame but the one where that happens.
    pub fn rebind(
        &mut self,
        device: &Device,
        point_buffer: &Buffer,
        sensitivity_buffer: &Buffer,
        stats_buffer: &Buffer,
    ) {
        if self.bound_size == sensitivity_buffer.size() {
            return;
        }
        self.bind_group = Self::make_bind_group(
            device,
            &self.layout,
            point_buffer,
            &self.camera_buffer,
            sensitivity_buffer,
            stats_buffer,
            &self.params_buffer,
        );
        self.bound_size = sensitivity_buffer.size();
    }

    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    pub fn upload_params(&self, queue: &wgpu::Queue, params: &EmphasisParams) {
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(params));
    }

    fn ensure_accumulator(&mut self, device: &Device, width: u32, height: u32) {
        let w = (width / ACCUM_DIVISOR).max(1);
        let h = (height / ACCUM_DIVISOR).max(1);
        if self.accum.as_ref().is_some_and(|a| a.width == w && a.height == h) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("emphasis_accum_texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ACCUM_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("emphasis_composite_bind_group"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.accum = Some(Accumulator { view, bind_group, width: w, height: h });
    }

    /// Accumulate, then composite over `target`.
    ///
    /// `instances` is one per point of the run — including the unlit ones,
    /// which are the denominator of the per-pixel mean. Most of them leave the
    /// vertex stage as a tiny quad and cost a handful of fragments of a
    /// quarter-size target; see `ACCUM_DIVISOR`.
    pub fn render(
        &mut self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        instances: u32,
    ) {
        if instances == 0 {
            return;
        }
        self.ensure_accumulator(device, width, height);
        let Some(accum) = &self.accum else { return };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("emphasis_accumulate_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &accum.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.accum_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..4, 0..instances);
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("emphasis_composite_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_bind_group(1, &accum.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
