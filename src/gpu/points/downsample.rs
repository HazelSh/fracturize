//! Reconstruction filter + downsample, for supersampled offline renders.
//!
//! Renders an `N·W x N·H` accumulation down to `W x H` through a real filter
//! kernel. This is flam3's `oversample` + `filter_radius`, and it is the single
//! largest visible improvement per unit of effort available to this renderer:
//! measured on the reference desktop, rendering 4x and filtering down beat
//! native rendering at the *same total sample count*, decisively, for +40% wall
//! clock.
//!
//! The reason it wins at equal sample count is that the thing wrong with the
//! image was never shot noise. Every point is deposited into exactly one pixel
//! with no kernel, so the picture is a raw histogram read out through a log
//! curve — and past ~33 samples per pixel it stops getting visibly better while
//! staying hard-edged and crunchy. More samples fix noise; filtering fixes
//! aliasing, and only one of those was missing.
//!
//! Offline only. The live window's aliasing *is* the look (see AGENTS.md on why
//! the artwork is not multisampled), and N x supersampling costs N² fill.

use wgpu::{BindGroupLayout, Buffer, Device, RenderPipeline, TextureFormat};

/// Reconstruction kernels, in increasing sharpness.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    /// Flat average over the support. At the default radius this is exactly an
    /// N x N block average — the arrangement the baseline measurements used.
    Box,
    /// Linear falloff. Cheap, and softer than box without being blurry.
    Triangle,
    /// The default, and what this lineage means by "the filter".
    #[default]
    Gaussian,
    /// Mitchell-Netravali (B = C = 1/3): the classic blur/ringing compromise.
    /// Worth an A/B against gaussian as a possibly-better default.
    Mitchell,
    /// Sharpest, and deliberately **not** the default: its negative lobes ring
    /// around small bright cores, which is what a flame image is made of.
    Lanczos,
}

impl Filter {
    fn index(self) -> u32 {
        match self {
            Filter::Box => 0,
            Filter::Triangle => 1,
            Filter::Gaussian => 2,
            Filter::Mitchell => 3,
            Filter::Lanczos => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Filter::Box => "box",
            Filter::Triangle => "triangle",
            Filter::Gaussian => "gaussian",
            Filter::Mitchell => "mitchell",
            Filter::Lanczos => "lanczos",
        }
    }
}

/// Supersampling bounds. 1 is off; past 4 the accumulation textures get large
/// enough to be the thing that decides whether a render runs at all, for a
/// difference nobody can see.
pub const MAX_SUPERSAMPLE: u32 = 4;

/// Filter half-width in *output* pixels.
///
/// The floor is 0.5 and not lower: below half an output pixel the kernel cannot
/// reach every source texel of the block, so some samples would be dropped
/// outright — supersampling that throws away three quarters of what it computed.
/// At exactly 0.5 the box kernel is the N x N block average.
pub const MIN_FILTER_RADIUS: f32 = 0.5;
/// Past this the blur is doing more than reconstruction and the tap count grows
/// as its square.
pub const MAX_FILTER_RADIUS: f32 = 2.0;
pub const DEFAULT_FILTER_RADIUS: f32 = 0.5;

/// Uniforms (16 bytes), must match `DownsampleParams` in downsample.wgsl
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct DownsampleParams {
    scale: u32,
    kernel: u32,
    support: f32,
    straight_alpha: f32,
}

/// What kind of values the source texture holds, which decides how alpha is
/// treated across the filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// Splat accumulation: `(colour * weight, weight)`, already additive.
    /// Averaging channel by channel is correct, and the tonemap's `rgb / a`
    /// recovers the density-weighted mean.
    Additive,
    /// The points renderer's colour target: straight (non-premultiplied) alpha.
    /// Premultiply, filter, unpremultiply — otherwise an opaque pixel filtered
    /// against a transparent one comes out darker rather than unchanged.
    StraightAlpha,
}

pub struct Downsampler {
    pipeline: RenderPipeline,
    layout: BindGroupLayout,
    params_buffer: Buffer,
}

impl Downsampler {
    /// One per target format: the splat path resolves into `Rgba16Float`
    /// (still linear, still pre-tonemap) and the points path into the sRGB
    /// output format, where the hardware's decode-on-load and encode-on-store
    /// are what put the averaging in linear light for free.
    pub fn new(device: &Device, target_format: TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("downsample_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/points/downsample.wgsl").into(),
            ),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("downsample_bind_group_layout"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("downsample_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("downsample_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_downsample"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_downsample"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
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
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("downsample_params_buffer"),
            size: std::mem::size_of::<DownsampleParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self { pipeline, layout, params_buffer }
    }

    pub fn upload_params(
        &self,
        queue: &wgpu::Queue,
        supersample: u32,
        filter: Filter,
        filter_radius: f32,
        source: Source,
    ) {
        let params = DownsampleParams {
            scale: supersample.max(1),
            kernel: filter.index(),
            support: filter_radius.clamp(MIN_FILTER_RADIUS, MAX_FILTER_RADIUS),
            straight_alpha: (source == Source::StraightAlpha) as u32 as f32,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    /// Encode the filter pass. `src` is the supersampled texture, `dst` the
    /// output-sized one; the pass overwrites `dst` completely.
    pub fn pass(
        &self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("downsample_bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("downsample_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Fullscreen overwrite; the clear value never survives.
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader switches on these numbers. They are one fact in two
    /// languages and WGSL is opaque to the compiler, so a test guards them —
    /// the same arrangement `EDGE_DOT_VERTS` uses for the gizmo shader.
    #[test]
    fn the_shader_agrees_about_the_kernel_numbering() {
        let wgsl = include_str!("../../../shaders/points/downsample.wgsl");
        for (f, name) in [
            (Filter::Box, "KERNEL_BOX"),
            (Filter::Triangle, "KERNEL_TRIANGLE"),
            (Filter::Gaussian, "KERNEL_GAUSSIAN"),
            (Filter::Mitchell, "KERNEL_MITCHELL"),
            (Filter::Lanczos, "KERNEL_LANCZOS"),
        ] {
            let decl = format!("const {}: u32 = {}u;", name, f.index());
            assert!(
                wgsl.contains(&decl),
                "downsample.wgsl must declare `{}` — Filter::{:?} is {}",
                decl,
                f,
                f.index()
            );
        }
    }

    /// Every kernel is selected by name on the command line and named back in
    /// the render record, so the two spellings have to agree.
    #[test]
    fn filter_labels_match_their_cli_names() {
        use clap::ValueEnum;
        for f in Filter::value_variants() {
            let cli = f.to_possible_value().expect("every filter is selectable");
            assert_eq!(cli.get_name(), f.label());
        }
    }

    /// The floor exists so the kernel always reaches every source texel of the
    /// block; below it, supersampling would discard most of what it computed.
    #[test]
    fn the_filter_radius_floor_covers_the_whole_block() {
        assert!(MIN_FILTER_RADIUS >= 0.5);
        assert!(DEFAULT_FILTER_RADIUS >= MIN_FILTER_RADIUS);
        assert!(DEFAULT_FILTER_RADIUS <= MAX_FILTER_RADIUS);
    }
}
