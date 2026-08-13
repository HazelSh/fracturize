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
use crate::gpu::points::downsample::{Downsampler, Filter, Source};
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
    /// Supersampling, for offline renders. `None` is 1x: the accumulation is
    /// the output size and the tonemap reads it directly, byte for byte as
    /// before this existed.
    ///
    /// The filter runs on the **linear** accumulation, between accumulate and
    /// tonemap. That position is the whole design: filtering after the log
    /// would blur in a perceptually compressed space and muddy bright cores.
    supersample: Option<Supersample>,
}

/// What supersampling adds: the factor, the kernel, and the output-sized
/// linear texture the filter resolves into for the tonemap to read.
struct Supersample {
    factor: u32,
    downsampler: Downsampler,
    resolved: Option<Resolved>,
}

struct Resolved {
    view: wgpu::TextureView,
    tonemap_bind_group: BindGroup,
    width: u32,
    height: u32,
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
            supersample: None,
        }
    }

    /// Turn on supersampling for this renderer. Offline only — see
    /// `gpu::points::downsample` for why the live window doesn't want it.
    ///
    /// A factor of 1 leaves the renderer exactly as it was, rather than
    /// installing a filter pass that would be an identity with rounding.
    pub fn set_supersample(
        &mut self,
        device: &Device,
        queue: &wgpu::Queue,
        factor: u32,
        filter: Filter,
        radius: f32,
    ) {
        if factor <= 1 {
            self.supersample = None;
            return;
        }
        let downsampler = Downsampler::new(device, ACCUM_FORMAT);
        // The splat accumulation is additive `(colour*weight, weight)`, so it
        // filters channel by channel and the tonemap's `rgb / a` still
        // recovers the density-weighted mean.
        downsampler.upload_params(queue, factor, filter, radius, Source::Additive);
        self.supersample = Some(Supersample { factor, downsampler, resolved: None });
    }

    /// The factor in force, 1 when off. Callers need it to size the
    /// accumulation and to scale anything measured in target pixels.
    pub fn supersample(&self) -> u32 {
        self.supersample.as_ref().map_or(1, |s| s.factor)
    }


    pub fn upload_camera(&self, queue: &wgpu::Queue, camera: &CameraUniforms) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Upload tonemap parameters. `samples` normalizes exposure: it is the
    /// buffer's *capacity* (not the currently valid count) in the ring-buffer
    /// modes, so points keep constant brightness while the buffer refills —
    /// matching how the plain renderer sparsens — and the total accumulated
    /// sample count when there is a persistent histogram behind the tonemap.
    ///
    /// `f64` because that total is unbounded: an accumulating render passes
    /// 2^32 samples in minutes, and a `u32` here would wrap a long render's
    /// exposure to something arbitrary.
    pub fn upload_params(
        &self,
        queue: &wgpu::Queue,
        exposure: f32,
        samples: f64,
        screen_height: f32,
        background: wgpu::Color,
        transparent: bool,
    ) {
        let exposure_scale = if samples > 0.0 {
            (exposure as f64 * EXPOSURE_K as f64 * screen_height as f64 * screen_height as f64
                / samples) as f32
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

    /// Splat the points and tonemap into `target`.
    ///
    /// `width` and `height` are the **output** size. Under N x supersampling
    /// the accumulation is N times larger in each axis and three passes are
    /// encoded rather than two: accumulate (N x, own HDR texture), filter down
    /// to output size (still linear, still HDR), tonemap into `target`
    /// (clearing `depth` so later overlay passes can Load it as usual).
    ///
    /// Callers must size everything measured in target pixels — the camera's
    /// `screen_height`, and the subpixel `use_point_primitives` decision —
    /// against the *accumulation*, not the output. Getting the second one
    /// wrong is the failure that makes this feature look like it did nothing:
    /// points that are subpixel at output but not at accumulation resolution
    /// would keep taking the unfiltered 1px path, which is exactly the finest,
    /// most alias-prone material in the picture.
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
        let n = self.supersample();
        self.ensure_accum(device, width * n, height * n);
        if n > 1 {
            Self::ensure_resolved(
                self.supersample.as_mut().expect("n > 1 means it is set"),
                device,
                &self.tonemap_layout,
                &self.params_buffer,
                width,
                height,
            );
        }
        self.splat_pass(encoder, 0..point_count, use_point_primitives);

        let accum = self.accum.as_ref().unwrap();
        // Whichever texture holds output-sized linear density: the resolved
        // one under supersampling, the accumulation itself otherwise.
        let tonemap_bind_group = match self.supersample.as_ref() {
            Some(ss) => {
                let resolved = ss.resolved.as_ref().expect("ensured above");
                ss.downsampler.pass(device, encoder, &accum.view, &resolved.view);
                &resolved.tonemap_bind_group
            }
            None => &accum.tonemap_bind_group,
        };
        Self::encode_tonemap(&self.tonemap_pipeline, encoder, target, depth, tonemap_bind_group);
    }

    /// Splat `points` — a half-open range of point-buffer indices — into the
    /// accumulation texture, clearing it first.
    ///
    /// The clear is what makes this usable as a *batch*: the accumulating
    /// renderer wants each pass to hold one batch's contribution and nothing
    /// else, because the persistent histogram behind it is where sums live.
    /// The ordinary path passes the whole buffer and gets what it always got.
    ///
    /// The caller must have ensured the accumulation target (`render` does, and
    /// `ensure_accum` is what does it).
    pub fn splat_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        points: std::ops::Range<u32>,
        use_point_primitives: bool,
    ) {
        let accum = self.accum.as_ref().expect("accumulation target must be ensured first");
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
        if !points.is_empty() {
            pass.set_bind_group(0, &self.accum_bind_group, &[]);
            // Both entry points read the point index straight off a builtin
            // (`vertex_index` for points, `instance_index` for quads), and
            // WebGPU includes the draw's first index in both — so a range is a
            // range either way.
            if use_point_primitives {
                pass.set_pipeline(&self.point_pipeline);
                pass.draw(points, 0..1);
            } else {
                pass.set_pipeline(&self.quad_pipeline);
                pass.draw(0..4, points);
            }
        }
    }

    /// Size the accumulation target for an output of `width x height`, and
    /// return the view a caller can read the raw batch out of.
    ///
    /// The returned size is the *accumulation* size: N times larger per axis
    /// under supersampling.
    pub fn prepare_accum(
        &mut self,
        device: &Device,
        width: u32,
        height: u32,
    ) -> (&wgpu::TextureView, u32, u32) {
        let n = self.supersample();
        self.ensure_accum(device, width * n, height * n);
        let accum = self.accum.as_ref().expect("just ensured");
        (&accum.view, accum.width, accum.height)
    }

    /// A tonemap bind group over an arbitrary output-sized linear texture —
    /// the accumulating path's resolved histogram, rather than either of the
    /// two textures this renderer owns.
    pub fn tonemap_bind_group(&self, device: &Device, view: &wgpu::TextureView) -> BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat_external_tonemap_bind_group"),
            layout: &self.tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// Encode the log tonemap from `bind_group` into `target`.
    pub fn tonemap_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        bind_group: &BindGroup,
    ) {
        Self::encode_tonemap(&self.tonemap_pipeline, encoder, target, depth, bind_group);
    }

    fn encode_tonemap(
        pipeline: &RenderPipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        bind_group: &BindGroup,
    ) {
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// (Re)create the output-sized linear texture the filter resolves into,
    /// and the tonemap bind group that reads it.
    ///
    /// An associated function taking its pieces rather than a `&mut self`
    /// method, because it needs `self.supersample` mutably and the tonemap
    /// layout and params buffer immutably at the same time.
    fn ensure_resolved(
        ss: &mut Supersample,
        device: &Device,
        tonemap_layout: &BindGroupLayout,
        params_buffer: &Buffer,
        width: u32,
        height: u32,
    ) {
        if let Some(r) = &ss.resolved {
            if r.width == width && r.height == height {
                return;
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("splat_resolved_texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Still the HDR accumulation format: this holds filtered *linear
            // density*, not a picture. The log tonemap has not run yet.
            format: ACCUM_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let tonemap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat_resolved_tonemap_bind_group"),
            layout: tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        ss.resolved = Some(Resolved { view, tonemap_bind_group, width, height });
    }
}
