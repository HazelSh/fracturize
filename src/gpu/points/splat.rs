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

use crate::gpu::buffers::{create_camera_buffer, Alpha, CameraUniforms};
use crate::gpu::points::downsample::{Downsampler, Filter, Source};
use crate::gpu::points::volume::{VolumeParams, BINS};
use crate::gpu::points::renderer::DEPTH_FORMAT;

/// HDR accumulation format, chosen per device.
///
/// `Rgba16Float` is the widest format with *guaranteed* blending support, and
/// it was the only option here until this became a measured problem. fp16
/// carries 11 bits of integer precision, so a texel that reaches **2048** in
/// one pass stops registering unit increments altogether — not a rounding
/// error but a hard stall, at a value a bright core passes easily.
///
/// In the ordinary ring-buffer path that stall only flattens the very hottest
/// cores, which is what the old comment here said and it was true. Under
/// **accumulation** it is far worse: each lap's batch clips at 2048, so the
/// finished image's brightest material is a function of *how many laps* the
/// ring happened to take. Measured on blossom at a fixed ~65M samples, the
/// peak density came out at exactly `2048 x laps` every time — 8,192 with a
/// 20M ring, 126,976 with a 1M one. `--points` was silently changing the
/// picture, which it must never do.
///
/// So: `Rgba32Float` wherever the device will blend into it, which is what
/// `Features::FLOAT32_BLENDABLE` reports. Not universal — it is an optional
/// feature and integrated parts often lack it — so fp16 remains the fallback
/// and `accumulation_is_exact` tells a caller which one it got.
fn accum_format(device: &Device, exact: bool) -> TextureFormat {
    if exact && device.features().contains(wgpu::Features::FLOAT32_BLENDABLE) {
        TextureFormat::Rgba32Float
    } else {
        TextureFormat::Rgba16Float
    }
}

/// Calibration so exposure 1.0 gives a sensible image: multiplies the
/// resolution/count-normalized density before the log
const EXPOSURE_K: f32 = 2.0;
/// Output gain applied after the log
const GAIN: f32 = 0.25;

/// Tonemap uniforms (48 bytes), must match SplatParams in splat.wgsl
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct SplatParams {
    background: [f32; 4],
    exposure_scale: f32,
    gain: f32,
    /// `Alpha::shader_value`: 1.0 = write straight-alpha coverage instead of
    /// compositing over `background`, 2.0 = the same coverage premultiplied
    /// for a compositor. Takes one of the two pad slots, so the uniform's size
    /// and alignment are unchanged.
    transparent: f32,
    /// `1/gamma`, inverted here so the shader never divides.
    gamma_inv: f32,
    gamma_threshold: f32,
    vibrancy: f32,
    /// LSBs of triangular dither before the 8-bit quantize. 0 is off, and off
    /// takes the shader's early return, so an undithered render is untouched.
    dither: f32,
    /// Volume mode's tonemap only; see `volume_n` in splat.wgsl. 0 otherwise.
    volume_n: f32,
}

/// The tonemap grade, as the CLI and the record speak about it.
///
/// A struct rather than three loose floats because they are one decision —
/// "how does density become a picture" — and because the defaults have to
/// travel together: [`Grade::NEUTRAL`] is exactly the tonemap that existed
/// before any of this, so an unspecified grade cannot change an old render.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Grade {
    /// > 1 lifts the dim end. 1 is off.
    pub gamma: f32,
    /// Coverage below which gamma is blended toward a straight line, so the
    /// background speckle is not lifted into a grey veil. 0 is off.
    pub gamma_threshold: f32,
    /// 1 keeps hue through the gamma; 0 rolls highlights toward white.
    pub vibrancy: f32,
}

impl Grade {
    /// The identity grade — and it really is the identity, not merely close.
    ///
    /// At `gamma = 1` the curve is `x^1`, so both of the shader's vibrancy
    /// routes collapse to `mean_color * coverage` and the blend between them
    /// is inert whatever `vibrancy` says. Which makes this the same arithmetic
    /// the tonemap did before it had a grade at all, and is what a test pins.
    pub const NEUTRAL: Grade = Grade { gamma: 1.0, gamma_threshold: 0.0, vibrancy: 1.0 };

    /// Bounds, so a typo cannot produce a black frame after a long render.
    pub fn clamped(self) -> Grade {
        Grade {
            gamma: self.gamma.clamp(0.1, 10.0),
            gamma_threshold: self.gamma_threshold.clamp(0.0, 1.0),
            vibrancy: self.vibrancy.clamp(0.0, 1.0),
        }
    }

    pub fn is_neutral(self) -> bool {
        self == Grade::NEUTRAL
    }
}

impl Default for Grade {
    fn default() -> Self {
        Grade::NEUTRAL
    }
}

pub struct SplatRenderer {
    /// The accumulation format this device got. See `accum_format`.
    accum_format: TextureFormat,
    quad_pipeline: RenderPipeline,
    point_pipeline: RenderPipeline,
    tonemap_pipeline: RenderPipeline,
    accum_bind_group: BindGroup,
    tonemap_layout: BindGroupLayout,
    pub camera_buffer: Buffer,
    params_buffer: Buffer,
    /// Volume mode's uniform. Bound to every splat draw, plain ones included
    /// (they never read it), so the layout is one layout; zero-initialised,
    /// which is `VolumeParams::OFF`.
    volume_params_buffer: Buffer,
    /// The volume passes' pipelines, `None` on a device that cannot blend into
    /// fp32 — where volume mode does not exist rather than running on fp16
    /// moments, which would be noise.
    volume_pipelines: Option<VolumePipelines>,
    /// Whether the next `render` draws the volume passes: opacity above zero
    /// and the device able to. At zero, nothing about the volume exists in the
    /// frame and the output is plain splat, byte for byte.
    volume_active: bool,
    /// The exact reference's pipelines, built the first time they are asked for.
    dump_pipelines: Option<DumpPipelines>,
    /// What the tonemap reads as `volume_n`: the supersample factor, or 0 when
    /// volume is off.
    volume_n: f32,
    /// The absorbance slices and each pixel's depth span, at accumulation
    /// resolution, recreated with it.
    moments: Option<BinTargets>,
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
    /// LSBs of dither the tonemap adds before the 8-bit quantize. 0 — the
    /// default, and what the window uses — is off.
    ///
    /// Opt-in rather than inferred from the target format, even though the
    /// format is what makes it necessary, because the two 8-bit surfaces want
    /// different answers: a `--bit-depth 8` file is kept and looked at closely,
    /// while the viewport is redrawn continuously and is not what anyone judges
    /// a gradient from. Inferring would quietly change the interactive look as
    /// a side effect of a file-format decision.
    dither: f32,
}

/// What supersampling adds: the factor, the kernel, and the output-sized
/// linear texture the filter resolves into for the tonemap to read.
struct Supersample {
    factor: u32,
    downsampler: Downsampler,
    resolved: Option<Resolved>,
}

struct Resolved {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    tonemap_bind_group: BindGroup,
    width: u32,
    height: u32,
}

/// Volume mode's per-pixel storage, at accumulation resolution: `BINS` slices of
/// absorbance, and the depth span they are measured against.
struct BinTargets {
    /// `BINS` u32 words a pixel, fixed point (`FIXED` to the unit).
    slices: Buffer,
    /// Two words a pixel: the farthest depth's bits and the complement of the
    /// nearest's.
    ranges: Buffer,
    /// Both, as group 1 of every volume pass.
    bind_group: BindGroup,
    /// Stands in as the colour target of the two passes that only write storage;
    /// nothing is written to it, but a pass needs one.
    front_target: wgpu::TextureView,
    width: u32,
    height: u32,
}

/// What volume mode adds to the compiled set: pass A (absorbance moments) and
/// pass B (emission weighted by transmittance), each in the two primitive
/// flavours splat has.
#[derive(Clone)]
struct VolumePipelines {
    abs_quad: RenderPipeline,
    abs_point: RenderPipeline,
    quad: RenderPipeline,
    point: RenderPipeline,
    bins_layout: BindGroupLayout,
    /// Fullscreen passes: the spans to log form, and the slices to running
    /// totals.
    range_pipe: RenderPipeline,
    prefix_pipe: RenderPipeline,
    /// Pass A0: each pixel's nearest and farthest depth.
    a0_quad: RenderPipeline,
    a0_point: RenderPipeline,
    /// The tonemap that also dims the background by the absorbance field, and
    /// the layout of its three bindings (input density, params, moments).
    tonemap: RenderPipeline,
    tonemap_layout: BindGroupLayout,
    /// What the exact reference needs to build its own pipelines on demand
    /// (`SplatRenderer::dump_fragments`): a device that cannot write storage
    /// from a fragment stage must not lose volume mode over a diagnostic.
    shader: wgpu::ShaderModule,
    accum_layout: BindGroupLayout,
}

struct DumpPipelines {
    quad: RenderPipeline,
    point: RenderPipeline,
    layout: BindGroupLayout,
}

/// Every fragment the volume passes would draw, as the exact reference reads
/// them: see `fs_dump` in splat.wgsl for the record layout.
pub struct FragmentDump {
    /// 8 words a record, `min(count, capacity)` records.
    pub words: Vec<u32>,
    /// How many fragments the draw produced. More than `words.len() / 8` means
    /// the buffer overflowed and the dump is a truncated sample.
    pub count: u64,
}

struct AccumTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    tonemap_bind_group: BindGroup,
    width: u32,
    height: u32,
}

/// The splat renderer's compiled pipelines, for one device and one output
/// format. Build once and hand to [`SplatRenderer::with_pipelines`]; the
/// handles are reference counted, so every renderer shares one compile.
#[derive(Clone)]
pub struct SplatPipelines {
    accum_format: TextureFormat,
    quad_pipeline: RenderPipeline,
    point_pipeline: RenderPipeline,
    tonemap_pipeline: RenderPipeline,
    accum_layout: BindGroupLayout,
    tonemap_layout: BindGroupLayout,
    volume: Option<VolumePipelines>,
}

impl SplatPipelines {
    /// Pipelines that accumulate in fp32 wherever the device will blend into
    /// it: what every offline render and thumbnail has always used.
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        Self::with_precision(device, format, true)
    }

    /// `exact = false` keeps the emission accumulation at fp16 even on a device
    /// that could do better, which is what the *live window* wants.
    ///
    /// The window's device asks for `FLOAT32_BLENDABLE` now, for volume mode's
    /// moment targets — and without this, that request would have quietly moved
    /// the window's ordinary splat from an 8-byte to a 16-byte accumulation
    /// texel, which measured about 17% of the frame rate at 8M points
    /// (28 -> 23 FPS on the GTX 1080). The fp16 stall at a density of 2048 is a
    /// ring-buffer curiosity in the window, where the picture is redrawn every
    /// frame; it is the accumulating renders that cannot have it. The moments
    /// are fp32 regardless — they are a separate pair of targets.
    pub fn with_precision(device: &Device, format: TextureFormat, exact: bool) -> Self {
        let accum_format = accum_format(device, exact);
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
                // The camera is read by the fragment stage too in volume mode:
                // the per-pixel span buffer is indexed by the frame's width.
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
                // Volume mode's depth warp, absorbance scale and blur width.
                // The vertex stage reads the first two, the blur's fragment
                // stage the third.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
                        format: accum_format,
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

        let volume = (device.limits().max_storage_buffers_per_shader_stage >= 6)
            .then(|| {
                // Group 1 of every volume pass: the slices and the depth spans,
                // both storage the fragment stage reads and (atomically) writes.
                let storage_entry = |binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                };
                let bins_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("volume_bins_bind_group_layout"),
                        entries: &[storage_entry(0), storage_entry(1)],
                    });
                let front_pipeline_layout =
                    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("volume_pipeline_layout"),
                        bind_group_layouts: &[Some(&accum_layout), Some(&bins_layout)],
                        immediate_size: 0,
                    });
                let resolve_layout = &front_pipeline_layout;
                let primitive = |topology| wgpu::PrimitiveState {
                    topology,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                };
                let make = |label: &str,
                            layout: &wgpu::PipelineLayout,
                            vs: &str,
                            fs: &str,
                            topology: wgpu::PrimitiveTopology,
                            targets: &[Option<wgpu::ColorTargetState>]| {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(label),
                        layout: Some(layout),
                        vertex: wgpu::VertexState {
                            module: &shader,
                            entry_point: Some(vs),
                            buffers: &[],
                            compilation_options: Default::default(),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some(fs),
                            targets,
                            compilation_options: Default::default(),
                        }),
                        primitive: primitive(topology),
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview_mask: None,
                        cache: None,
                    })
                };
                let target = |format| {
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(additive),
                        write_mask: wgpu::ColorWrites::ALL,
                    })
                };
                let front_targets = [Some(wgpu::ColorTargetState {
                    format: TextureFormat::R8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })];
                let emit_targets = [target(accum_format)];
                use wgpu::PrimitiveTopology::{PointList, TriangleList, TriangleStrip};
                let tonemap_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("volume_tonemap_bind_group_layout"),
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
                            wgpu::BindGroupLayoutEntry {
                                binding: 2,
                                visibility: wgpu::ShaderStages::FRAGMENT,
                                ty: wgpu::BindingType::Buffer {
                                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                                    has_dynamic_offset: false,
                                    min_binding_size: None,
                                },
                                count: None,
                            },
                        ],
                    });
                let tonemap_layout_pl =
                    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("volume_tonemap_pipeline_layout"),
                        bind_group_layouts: &[Some(&tonemap_layout)],
                        immediate_size: 0,
                    });
                // Same state as the plain tonemap: it carries the frame's depth
                // attachment so the overlay passes can Load it.
                let tonemap = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("volume_tonemap_pipeline"),
                    layout: Some(&tonemap_layout_pl),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_tonemap"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_tonemap_volume"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: primitive(TriangleList),
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
                VolumePipelines {
                    abs_quad: make("volume_abs_quad", &front_pipeline_layout, "vs_splat", "fs_abs", TriangleStrip, &front_targets),
                    abs_point: make("volume_abs_point", &front_pipeline_layout, "vs_splat_point", "fs_abs", PointList, &front_targets),
                    a0_quad: make("volume_a0_quad", &front_pipeline_layout, "vs_splat", "fs_front", TriangleStrip, &front_targets),
                    a0_point: make("volume_a0_point", &front_pipeline_layout, "vs_splat_point", "fs_front", PointList, &front_targets),
                    quad: make("volume_quad", resolve_layout, "vs_splat", "fs_splat_volume", TriangleStrip, &emit_targets),
                    point: make("volume_point", resolve_layout, "vs_splat_point", "fs_splat_volume", PointList, &emit_targets),
                    bins_layout,
                    range_pipe: make("volume_range", &front_pipeline_layout, "vs_tonemap", "fs_range", TriangleList, &front_targets),
                    prefix_pipe: make("volume_prefix", &front_pipeline_layout, "vs_tonemap", "fs_prefix", TriangleList, &front_targets),
                    tonemap,
                    tonemap_layout,
                    shader: shader.clone(),
                    accum_layout: accum_layout.clone(),
                }
            });

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

        Self {
            accum_format,
            quad_pipeline,
            point_pipeline,
            tonemap_pipeline,
            accum_layout,
            tonemap_layout,
            volume,
        }
    }

    /// Whether this device can run volume mode at all. It needs the fragment
    /// stage to write storage (atomically) from six buffers; nothing needs fp32
    /// blending any more — the absorbance is fixed point in u32 words.
    pub fn volume_available(&self) -> bool {
        self.volume.is_some()
    }
}

impl SplatRenderer {
    pub fn new(
        device: &Device,
        format: TextureFormat,
        point_buffer: &Buffer,
        colormap_buffer: &Buffer,
    ) -> Self {
        Self::with_pipelines(device, &SplatPipelines::new(device, format), point_buffer, colormap_buffer)
    }

    /// [`SplatRenderer::new`] on pipelines compiled once and shared; see
    /// [`SplatPipelines`].
    pub fn with_pipelines(
        device: &Device,
        pipelines: &SplatPipelines,
        point_buffer: &Buffer,
        colormap_buffer: &Buffer,
    ) -> Self {
        let SplatPipelines {
            accum_format,
            quad_pipeline,
            point_pipeline,
            tonemap_pipeline,
            accum_layout,
            tonemap_layout,
            volume: volume_pipelines,
        } = pipelines.clone();

        let camera_buffer = create_camera_buffer(device);
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat_params_buffer"),
            size: std::mem::size_of::<SplatParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let volume_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_params_buffer"),
            size: std::mem::size_of::<VolumeParams>() as u64,
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: volume_params_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            volume_params_buffer,
            volume_pipelines,
            volume_active: false,
            volume_n: 0.0,
            dump_pipelines: None,
            moments: None,
            accum_format,
            quad_pipeline,
            point_pipeline,
            tonemap_pipeline,
            accum_bind_group,
            tonemap_layout,
            camera_buffer,
            params_buffer,
            accum: None,
            supersample: None,
            dither: 0.0,
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
        let downsampler = Downsampler::new(device, self.accum_format);
        // The splat accumulation is additive `(colour*weight, weight)`, so it
        // filters channel by channel and the tonemap's `rgb / a` still
        // recovers the density-weighted mean.
        downsampler.upload_params(queue, factor, filter, radius, Source::Additive);
        self.supersample = Some(Supersample { factor, downsampler, resolved: None });
    }

    /// Whether this device accumulates into fp32, and so whether a bright core
    /// is counted faithfully or stalls at 2048 per pass.
    ///
    /// Worth reporting rather than hiding: on a device without
    /// `FLOAT32_BLENDABLE` an accumulating render's peak density still depends
    /// on how many laps the ring took, and a caller that says nothing leaves
    /// the user comparing two renders that were never comparable.
    pub fn accumulation_is_exact(&self) -> bool {
        self.accum_format == TextureFormat::Rgba32Float
    }

    /// The factor in force, 1 when off. Callers need it to size the
    /// accumulation and to scale anything measured in target pixels.
    pub fn supersample(&self) -> u32 {
        self.supersample.as_ref().map_or(1, |s| s.factor)
    }


    /// Dither the tonemap's output by `lsbs` steps of the 8-bit code before
    /// the hardware quantizes it.
    ///
    /// One LSB of triangular noise is the standard choice and what
    /// `--bit-depth 8` asks for; 0 turns it off entirely and takes the shader's
    /// early return, so an undithered render is byte-for-byte what it was.
    /// Pointless above 8 bits — a 16-bit file's quantization is already far
    /// under the sampling noise — so the 16-bit path leaves this alone.
    pub fn set_dither(&mut self, lsbs: f32) {
        self.dither = lsbs.max(0.0);
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
        alpha: Alpha,
        grade: Grade,
    ) {
        let grade = grade.clamped();
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
            transparent: alpha.shader_value(),
            gamma_inv: 1.0 / grade.gamma,
            gamma_threshold: grade.gamma_threshold,
            vibrancy: grade.vibrancy,
            dither: self.dither,
            volume_n: self.volume_n,
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
            format: self.accum_format,
            // COPY_SRC so the grade buffer can be read back: this texture is
            // exactly the tonemap's input, which is what re-grading needs.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
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
        self.accum = Some(AccumTarget { texture, view, tonemap_bind_group, width, height });
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
                self.accum_format,
                &self.tonemap_layout,
                &self.params_buffer,
                width,
                height,
            );
        }
        if self.volume_active {
            // Volume mode: each pixel's depth span, then absorbance into slices
            // of it, then the same splat with each fragment's weight scaled by
            // the transmittance the slices give. All at accumulation
            // resolution, so a supersampled render has them where its emission
            // is.
            self.ensure_moments(device, width * n, height * n);
            let m = self.moments.as_ref().expect("just ensured");
            encoder.clear_buffer(&m.slices, 0, None);
            encoder.clear_buffer(&m.ranges, 0, None);
            self.storage_pass(encoder, 0..point_count, use_point_primitives, true);
            self.fullscreen_storage_pass(encoder, true);
            self.storage_pass(encoder, 0..point_count, use_point_primitives, false);
            self.fullscreen_storage_pass(encoder, false);
            self.draw_splats(encoder, 0..point_count, use_point_primitives, true, true);
        } else {
            self.splat_pass(encoder, 0..point_count, use_point_primitives);
        }

        let accum = self.accum.as_ref().unwrap();
        // Whichever texture holds output-sized linear density: the resolved
        // one under supersampling, the accumulation itself otherwise.
        let (tonemap_bind_group, input_view) = match self.supersample.as_ref() {
            Some(ss) => {
                let resolved = ss.resolved.as_ref().expect("ensured above");
                ss.downsampler.pass(device, encoder, &accum.view, &resolved.view);
                (&resolved.tonemap_bind_group, &resolved.view)
            }
            None => (&accum.tonemap_bind_group, &accum.view),
        };
        if let (true, Some(vp), Some(m)) =
            (self.volume_active, self.volume_pipelines.as_ref(), self.moments.as_ref())
        {
            // The same tonemap, reading the absorbance field as well so it can
            // dim the background by it.
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("volume_tonemap_bind_group"),
                layout: &vp.tonemap_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: self.params_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: m.slices.as_entire_binding() },
                ],
            });
            Self::encode_tonemap(&vp.tonemap, encoder, target, depth, &bind_group);
        } else {
            Self::encode_tonemap(&self.tonemap_pipeline, encoder, target, depth, tonemap_bind_group);
        }
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
        self.splat_chunk(encoder, points, use_point_primitives, true);
    }

    /// One slice of a lap's points, with control over whether the batch target
    /// is cleared first.
    ///
    /// Splatting a whole lap in a single submission is what a big render used
    /// to do, and on a card with a watchdog it is how a render *kills the
    /// device*: 100M points at 12x is one draw of 400M vertices into a texture
    /// the size of a wall, and the driver resets the GPU rather than wait —
    /// `Error in Device::poll: Parent device is lost`. It is also why the
    /// desktop stops responding, since there is no preemption point anywhere in
    /// it.
    ///
    /// Chunking gives the driver somewhere to breathe. `clear` is true for the
    /// first chunk of a lap and false for the rest, so the batch accumulates
    /// across them exactly as one big pass would — additive blending does not
    /// care where the submission boundaries fall, so the arithmetic is
    /// unchanged and only the scheduling differs.
    pub fn splat_chunk(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        points: std::ops::Range<u32>,
        use_point_primitives: bool,
        clear: bool,
    ) {
        debug_assert!(
            !self.volume_active,
            "volume mode is a ring-buffer render: a lap's splat cannot weight itself by transmittance"
        );
        self.draw_splats(encoder, points, use_point_primitives, clear, false);
    }

    /// The splat draw itself. `volume` swaps the fragment stage for the one
    /// that multiplies by transmittance; the moments must have been drawn first.
    fn draw_splats(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        points: std::ops::Range<u32>,
        use_point_primitives: bool,
        clear: bool,
        volume: bool,
    ) {
        let accum = self.accum.as_ref().expect("accumulation target must be ensured first");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("splat_accum_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &accum.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    } else {
                        wgpu::LoadOp::Load
                    },
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
            let vp = if volume { self.volume_pipelines.as_ref() } else { None };
            if vp.is_some() {
                let moments = self.moments.as_ref().expect("pass A ran first");
                pass.set_bind_group(1, &moments.bind_group, &[]);
            }
            // Both entry points read the point index straight off a builtin
            // (`vertex_index` for points, `instance_index` for quads), and
            // WebGPU includes the draw's first index in both — so a range is a
            // range either way.
            if use_point_primitives {
                pass.set_pipeline(vp.map_or(&self.point_pipeline, |v| &v.point));
                pass.draw(points, 0..1);
            } else {
                pass.set_pipeline(vp.map_or(&self.quad_pipeline, |v| &v.quad));
                pass.draw(0..4, points);
            }
        }
    }

    /// The (blurred) absorbance moments' first target, `(b0, b1, b2, b3)`, as the
    /// last volume render left it — what a measurement reads `b0` from. `None`
    /// before one has run.
    /// The volume tonemap over an arbitrary output-sized density texture and an
    /// arbitrary absorbance field (`x` = optical depth `b0`) — what the exact
    /// reference is drawn through, so it and the GPU path share one tonemap.
    pub fn tonemap_volume_external(
        &self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        input: &wgpu::TextureView,
        slices: &Buffer,
    ) {
        let vp = self.volume_pipelines.as_ref().expect("volume is available");
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("volume_external_tonemap_bind_group"),
            layout: &vp.tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) },
                wgpu::BindGroupEntry { binding: 1, resource: self.params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: slices.as_entire_binding() },
            ],
        });
        Self::encode_tonemap(&vp.tonemap, encoder, target, depth, &bind_group);
    }

    /// Size of the moment targets (the accumulation's), once they exist.
    pub fn moments_size(&self) -> Option<(u32, u32)> {
        self.moments.as_ref().map(|m| (m.width, m.height))
    }

    pub fn slices_buffer(&self) -> Option<&Buffer> {
        self.moments.as_ref().map(|m| &m.slices)
    }

    /// Draw every point through the volume vertex stage and write each fragment
    /// it produces to a list instead of blending it: the input of the exact
    /// reference (`volume_ref.rs`). Must follow a volume `render` with the same
    /// camera, since it reads the moments that render left to evaluate the
    /// GPU path's transmittance per fragment. `capacity` is in fragments.
    pub fn dump_fragments(
        &mut self,
        device: &Device,
        queue: &wgpu::Queue,
        points: u32,
        use_point_primitives: bool,
        capacity: u64,
    ) -> Result<FragmentDump, String> {
        let vp = self.volume_pipelines.as_ref().ok_or("volume mode is not available")?;
        let moments = self.moments.as_ref().ok_or("render a volume frame first")?;
        if self.dump_pipelines.is_none() {
            let storage = |binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            };
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("volume_dump_bind_group_layout"),
                entries: &[storage(0), storage(1)],
            });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("volume_dump_pipeline_layout"),
                bind_group_layouts: &[Some(&vp.accum_layout), Some(&vp.bins_layout), Some(&layout)],
                immediate_size: 0,
            });
            let make = |vs: &str, topology| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("volume_dump_pipeline"),
                    layout: Some(&pl),
                    vertex: wgpu::VertexState {
                        module: &vp.shader,
                        entry_point: Some(vs),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &vp.shader,
                        entry_point: Some("fs_dump"),
                        // A target is required; nothing is written to it.
                        targets: &[Some(wgpu::ColorTargetState {
                            format: TextureFormat::R8Unorm,
                            blend: None,
                            write_mask: wgpu::ColorWrites::empty(),
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
            self.dump_pipelines = Some(DumpPipelines {
                quad: make("vs_splat", wgpu::PrimitiveTopology::TriangleStrip),
                point: make("vs_splat_point", wgpu::PrimitiveTopology::PointList),
                layout,
            });
        }
        let dp = self.dump_pipelines.as_ref().unwrap();
        let bytes = capacity * 32;
        let list = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_dump"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let counter = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_dump_count"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&counter, 0, &[0u8; 4]);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("volume_dump_bind_group"),
            layout: &dp.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: list.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: counter.as_entire_binding() },
            ],
        });
        let dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("volume_dump_target"),
                size: wgpu::Extent3d { width: moments.width, height: moments.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let count_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_dump_count_read"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("volume_dump_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("volume_dump_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dummy,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.accum_bind_group, &[]);
            pass.set_bind_group(1, &moments.bind_group, &[]);
            pass.set_bind_group(2, &bind, &[]);
            if use_point_primitives {
                pass.set_pipeline(&dp.point);
                pass.draw(0..points, 0..1);
            } else {
                pass.set_pipeline(&dp.quad);
                pass.draw(0..4, 0..points);
            }
        }
        encoder.copy_buffer_to_buffer(&counter, 0, &count_read, 0, 4);
        queue.submit(std::iter::once(encoder.finish()));
        let map = |buf: &Buffer| {
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        };
        map(&count_read);
        let count = {
            let d = count_read.slice(..).get_mapped_range();
            u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as u64
        };
        let kept = count.min(capacity);
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_dump_read"),
            size: (kept * 32).max(4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("volume_dump_copy"),
        });
        if kept > 0 {
            encoder.copy_buffer_to_buffer(&list, 0, &out, 0, kept * 32);
        }
        queue.submit(std::iter::once(encoder.finish()));
        map(&out);
        let words: Vec<u32> = if kept > 0 {
            let d = out.slice(..).get_mapped_range();
            d.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
        } else {
            Vec::new()
        };
        Ok(FragmentDump { words, count })
    }

    /// Whether this device can run volume mode (see
    /// [`SplatPipelines::volume_available`]).
    pub fn volume_available(&self) -> bool {
        self.volume_pipelines.is_some()
    }

    /// Turn volume mode on for the following renders, or off.
    ///
    /// `VolumeParams::OFF` (or any opacity of zero) turns it off, and off means
    /// *absent*: no moment pass, no multiply, the plain splat pipelines — which
    /// is what makes `--volume --opacity 0` byte-identical to `--splat` instead
    /// of merely close. On a device without fp32 blending it stays off whatever
    /// it is asked.
    pub fn set_volume(&mut self, queue: &wgpu::Queue, params: VolumeParams) {
        self.volume_active = params.enabled > 0.5 && self.volume_pipelines.is_some();
        self.volume_n = if self.volume_active { self.supersample() as f32 } else { 0.0 };
        queue.write_buffer(
            &self.params_buffer,
            (std::mem::size_of::<SplatParams>() - 4) as u64,
            bytemuck::bytes_of(&self.volume_n),
        );
        let written = if self.volume_active { params } else { VolumeParams::OFF };
        queue.write_buffer(&self.volume_params_buffer, 0, bytemuck::bytes_of(&written));
    }

    /// (Re)create the slices and the span buffer at the accumulation's size.
    fn ensure_moments(&mut self, device: &Device, width: u32, height: u32) {
        if let Some(m) = &self.moments {
            if m.width == width && m.height == height {
                return;
            }
        }
        let vp = self.volume_pipelines.as_ref().expect("volume is active");
        let pixels = (width as u64) * (height as u64);
        let buffer = |label: &'static str, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let slices = buffer("volume_slices", pixels * BINS as u64 * 4);
        let ranges = buffer("volume_ranges", pixels * 8);
        let front_target = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("volume_front_target"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("volume_bins_bind_group"),
            layout: &vp.bins_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: slices.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: ranges.as_entire_binding() },
            ],
        });
        self.moments = Some(BinTargets { slices, ranges, bind_group, front_target, width, height });
    }

    /// The two passes that only write storage: A0 (each pixel's depth span) and
    /// A (absorbance into slices of it). The spans must be complete before the
    /// slices are filled, which two separate passes guarantee.
    fn storage_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        points: std::ops::Range<u32>,
        use_point_primitives: bool,
        spans: bool,
    ) {
        let vp = self.volume_pipelines.as_ref().expect("volume is active");
        let m = self.moments.as_ref().expect("ensured above");
        if points.is_empty() {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(if spans { "volume_span_pass" } else { "volume_slice_pass" }),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &m.front_target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Discard,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.accum_bind_group, &[]);
        pass.set_bind_group(1, &m.bind_group, &[]);
        let pipeline = match (spans, use_point_primitives) {
            (true, true) => &vp.a0_point,
            (true, false) => &vp.a0_quad,
            (false, true) => &vp.abs_point,
            (false, false) => &vp.abs_quad,
        };
        pass.set_pipeline(pipeline);
        if use_point_primitives {
            pass.draw(points, 0..1);
        } else {
            pass.draw(0..4, points);
        }
    }

    /// The two per-pixel housekeeping passes: `range` rewrites the depth spans
    /// into the form the later passes read; `prefix` turns the slices into
    /// running totals. A fullscreen triangle each, over the one-byte target.
    fn fullscreen_storage_pass(&self, encoder: &mut wgpu::CommandEncoder, range: bool) {
        let vp = self.volume_pipelines.as_ref().expect("volume is active");
        let m = self.moments.as_ref().expect("ensured above");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(if range { "volume_range_pass" } else { "volume_prefix_pass" }),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &m.front_target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Discard,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.accum_bind_group, &[]);
        pass.set_bind_group(1, &m.bind_group, &[]);
        pass.set_pipeline(if range { &vp.range_pipe } else { &vp.prefix_pipe });
        pass.draw(0..3, 0..1);
    }

    /// Total optical depth `b0` per pixel — the sum of the slices — as
    /// `[b0, 0, 0, 0]` texels, with the size they are at. Reads the buffer back:
    /// a measuring instrument.
    pub fn read_total_absorbance(
        &self,
        device: &Device,
        queue: &wgpu::Queue,
    ) -> Option<(Vec<f32>, u32, u32)> {
        let m = self.moments.as_ref()?;
        let bytes = m.slices.size();
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_slices_read"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("volume_slices_copy"),
        });
        encoder.copy_buffer_to_buffer(&m.slices, 0, &read, 0, bytes);
        queue.submit(std::iter::once(encoder.finish()));
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let d = read.slice(..).get_mapped_range();
        let words: &[u32] = bytemuck::cast_slice(&d);
        let mut out = vec![0.0f32; (m.width * m.height * 4) as usize];
        for p in 0..(m.width * m.height) as usize {
            // The running total's last word is the whole of it.
            out[p * 4] = f32::from_bits(words[(p + 1) * BINS as usize - 1]);
        }
        Some((out, m.width, m.height))
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

    /// The output-sized linear density the tonemap reads: the filter's
    /// resolve under supersampling, the accumulation itself otherwise.
    ///
    /// `None` before the first `render`, which is what allocates them. This is
    /// what the grade buffer is read back from — it is bit-for-bit the
    /// tonemap's input, which is what makes re-grading exact rather than an
    /// approximation of a re-render.
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        match self.supersample.as_ref() {
            Some(ss) => ss.resolved.as_ref().map(|r| &r.texture),
            None => self.accum.as_ref().map(|a| &a.texture),
        }
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
        format: TextureFormat,
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
            format,
            // COPY_SRC so the grade buffer can be read back: this texture is
            // exactly the tonemap's input, which is what re-grading needs.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
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
        ss.resolved = Some(Resolved { texture, view, tonemap_bind_group, width, height });
    }
}

#[cfg(test)]
mod grade_tests {
    use super::*;

    /// The whole safety property of slice 6a in one assertion: an unspecified
    /// grade is the tonemap that existed before grading did.
    ///
    /// The shader's two vibrancy routes are `mean*gamma(coverage)` and
    /// `gamma(mean*coverage)`. At `gamma = 1` the curve is the identity, so
    /// both are `mean*coverage` and the blend between them cannot matter —
    /// which is why the neutral vibrancy can be 1 without that being a choice.
    #[test]
    fn the_neutral_grade_is_the_old_tonemap() {
        let g = Grade::NEUTRAL;
        assert_eq!(g.gamma, 1.0, "gamma 1 makes the curve the identity");
        assert_eq!(g.gamma_threshold, 0.0, "no toe means no piecewise branch");
        assert!(g.is_neutral());
        assert!(Grade::default().is_neutral());
        // And clamping must not perturb it, or the default would drift.
        assert_eq!(g.clamped(), g);
    }

    /// The shader reads `1/gamma`; the CPU is where that inversion happens, so
    /// the floor has to keep it finite.
    #[test]
    fn clamping_keeps_the_gamma_inversion_finite() {
        for gamma in [0.0, -1.0, f32::MIN_POSITIVE, 1e9] {
            let c = Grade { gamma, ..Grade::NEUTRAL }.clamped();
            let inv = 1.0 / c.gamma;
            assert!(inv.is_finite() && inv > 0.0, "gamma {} gave 1/g = {}", gamma, inv);
        }
    }

    /// Vibrancy and the threshold are blend factors the shader uses raw.
    #[test]
    fn clamping_keeps_the_blend_factors_in_range() {
        let wild = Grade { gamma: 2.0, gamma_threshold: 9.0, vibrancy: -3.0 }.clamped();
        assert!((0.0..=1.0).contains(&wild.gamma_threshold));
        assert!((0.0..=1.0).contains(&wild.vibrancy));
    }

    /// The uniform's size is a contract with WGSL that the compiler cannot see.
    #[test]
    fn the_tonemap_uniform_matches_the_shader() {
        assert_eq!(
            std::mem::size_of::<SplatParams>(),
            48,
            "SplatParams must be 48 bytes to match splat.wgsl"
        );
        let wgsl = include_str!("../../../shaders/points/splat.wgsl");
        for field in ["gamma_inv", "gamma_threshold", "vibrancy"] {
            assert!(wgsl.contains(field), "splat.wgsl must declare `{}`", field);
        }
    }
}
