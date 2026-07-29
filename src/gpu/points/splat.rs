//! Additive log-density splat renderer ("flame" accumulation)
//!
//! Third rendering option, alongside the plain point renderer (opaque
//! depth-tested points) and the shelved density hash-grid. Points are
//! splatted into an HDR accumulation texture with additive blending —
//! gaussian kernels carrying unit energy each — then a fullscreen pass
//! log-tonemaps the accumulated density. Isolated points remain visible
//! grit; overlapping ones build smooth log-density gradients instead of
//! clipping at full brightness. No occlusion: the fractal is emission.

use wgpu::{BindGroup, BindGroupLayout, Buffer, Device, RenderPipeline, TextureFormat};

use crate::gpu::buffers::{create_camera_buffer, CameraUniforms};
use crate::gpu::points::renderer::DEPTH_FORMAT;

/// HDR accumulation format. rgba16float is the widest format with
/// guaranteed blending support (float32 blending is not exposed by wgpu);
/// its precision fades for tiny increments into large sums, which after
/// the log tonemap only flattens the very hottest cores.
const ACCUM_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Calibration so exposure 1.0 gives a sensible image: multiplies the
/// resolution/count-normalized density before the log
const EXPOSURE_K: f32 = 2.0;
/// Output gain applied after the log
const GAIN: f32 = 0.25;

/// Tonemap uniforms (32 bytes), must match SplatParams in splat.wgsl
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct SplatParams {
    background: [f32; 4],
    exposure_scale: f32,
    gain: f32,
    /// 1.0 = write straight-alpha coverage instead of compositing over
    /// `background`. Takes one of the two pad slots, so the uniform's size
    /// and alignment are unchanged.
    transparent: f32,
    _pad: f32,
}

pub struct SplatRenderer {
    quad_pipeline: RenderPipeline,
    point_pipeline: RenderPipeline,
    tonemap_pipeline: RenderPipeline,
    accum_bind_group: BindGroup,
    tonemap_layout: BindGroupLayout,
    pub camera_buffer: Buffer,
    params_buffer: Buffer,
    /// Accumulation target + matching tonemap bind group, recreated when
    /// the render size changes (window resize, screenshots)
    accum: Option<AccumTarget>,
}

struct AccumTarget {
    view: wgpu::TextureView,
    tonemap_bind_group: BindGroup,
    width: u32,
    height: u32,
}

impl SplatRenderer {
    pub fn new(
        device: &Device,
        format: TextureFormat,
        point_buffer: &Buffer,
        colormap_buffer: &Buffer,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("splat_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/points/splat.wgsl").into()),
        });

        // Accumulation bind group: points, camera, colormap (same shape as
        // the point renderer's)
        let accum_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat_accum_bind_group_layout"),
            entries: &[
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

        let accum_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splat_accum_pipeline_layout"),
            bind_group_layouts: &[Some(&accum_layout)],
            immediate_size: 0,
        });

        // Pure additive accumulation into the HDR target
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let make_accum_pipeline = |label: &str, entry: &str, topology: wgpu::PrimitiveTopology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&accum_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_splat"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: ACCUM_FORMAT,
                        blend: Some(additive),
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
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let quad_pipeline = make_accum_pipeline(
            "splat_quad_pipeline",
            "vs_splat",
            wgpu::PrimitiveTopology::TriangleStrip,
        );
        let point_pipeline = make_accum_pipeline(
            "splat_point_pipeline",
            "vs_splat_point",
            wgpu::PrimitiveTopology::PointList,
        );

        // Tonemap bind group: accumulation texture + params
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat_tonemap_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let tonemap_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splat_tonemap_pipeline_layout"),
            bind_group_layouts: &[Some(&tonemap_layout)],
            immediate_size: 0,
        });

        // The tonemap pass carries the frame's depth attachment (cleared,
        // never tested) so overlay passes that Load depth work unchanged
        let tonemap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("splat_tonemap_pipeline"),
            layout: Some(&tonemap_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_tonemap"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_tonemap"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
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
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat_params_buffer"),
            size: std::mem::size_of::<SplatParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let accum_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat_accum_bind_group"),
            layout: &accum_layout,
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
            tonemap_pipeline,
            accum_bind_group,
            tonemap_layout,
            camera_buffer,
            params_buffer,
            accum: None,
        }
    }

    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Upload tonemap parameters. `point_capacity` (not the currently valid
    /// count) normalizes exposure, so points keep constant brightness while
    /// the buffer refills — matching how the plain renderer sparsens.
    pub fn upload_params(
        &self,
        queue: &wgpu::Queue,
        exposure: f32,
        point_capacity: u32,
        screen_height: f32,
        background: wgpu::Color,
        transparent: bool,
    ) {
        let exposure_scale = if point_capacity > 0 {
            exposure * EXPOSURE_K * screen_height * screen_height / point_capacity as f32
        } else {
            0.0
        };
        let params = SplatParams {
            background: [
                background.r as f32,
                background.g as f32,
                background.b as f32,
                1.0,
            ],
            exposure_scale,
            gain: GAIN,
            transparent: transparent as u32 as f32,
            _pad: 0.0,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    /// (Re)create the accumulation target when the render size changes
    fn ensure_accum(&mut self, device: &Device, width: u32, height: u32) {
        if let Some(t) = &self.accum {
            if t.width == width && t.height == height {
                return;
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("splat_accum_texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ACCUM_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let tonemap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat_tonemap_bind_group"),
            layout: &self.tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });
        self.accum = Some(AccumTarget { view, tonemap_bind_group, width, height });
    }

    /// Splat the points and tonemap into `target`. Encodes two passes:
    /// accumulate (own HDR texture) and tonemap (into `target`, clearing
    /// `depth` so later overlay passes can Load it as usual).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
        point_count: u32,
        use_point_primitives: bool,
    ) {
        self.ensure_accum(device, width, height);
        let accum = self.accum.as_ref().unwrap();

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("splat_accum_pass"),
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
            if point_count > 0 {
                pass.set_bind_group(0, &self.accum_bind_group, &[]);
                if use_point_primitives {
                    pass.set_pipeline(&self.point_pipeline);
                    pass.draw(0..point_count, 0..1);
                } else {
                    pass.set_pipeline(&self.quad_pipeline);
                    pass.draw(0..4, 0..point_count);
                }
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("splat_tonemap_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Fullscreen overwrite; the clear value is irrelevant
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, &accum.tonemap_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
