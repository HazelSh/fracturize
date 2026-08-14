//! Density estimation: the variable-width blur.
//!
//! Wide kernels where the histogram is sparse and noisy, narrow where it is
//! dense and detailed. See `shaders/points/density_estimate.wgsl` for the
//! method and for why it is a mip pyramid rather than the summed-area table
//! the plan proposed — the short version is that an f32 SAT carries ~715x the
//! error of the faint tails DE exists to smooth.
//!
//! It runs on **linear density at accumulation resolution**, between the
//! histogram resolve and the reconstruction filter. That position matters: DE
//! before the filter means the filter still does its own job on an image DE
//! has already de-noised, and both run before the log tonemap, where the
//! arithmetic is still additive and a blur means what it says.

use wgpu::{BindGroup, BindGroupLayout, Buffer, Device, RenderPipeline, TextureFormat};

/// Deepest pyramid level, so the largest box is 2^6 = 64 accumulation texels.
///
/// Past this the blur is wider than any noise structure in a flame image and
/// the level is a solid colour over most of the frame — it would flatten the
/// picture, not smooth it.
const MAX_LEVELS: u32 = 7;

/// Largest DE radius at `amount = 1`, in **output** pixels.
///
/// Output rather than accumulation pixels so the control means the same thing
/// at every supersample factor: the caller multiplies by N.
pub const MAX_RADIUS_PX: f32 = 6.0;

/// Samples per accumulation texel treated as "enough" at `amount = 1`.
///
/// The radius law is `sqrt(target / density)` (see the shader), so this is the
/// density at which DE switches off entirely.
///
/// **Calibrated against measured densities, not guessed.** `exposure_survey`
/// reports blossom's median *lit* accumulation texel at ~13 samples with p99.5
/// at ~8000 — a flame image is mostly sparse haze with a thin bright skeleton
/// through it. The first attempt used 256, which put the median lit texel at a
/// 2.4-texel radius and smeared the structure along with the noise.
///
/// At 16: a median lit texel (13) gets radius 1.1 and is essentially untouched,
/// a filament (100+) gets 0.4 and is left strictly alone, and the sparse haze
/// at density 1 gets 4 texels — which is the material DE is for.
const TARGET_DENSITY: f32 = 16.0;

/// Uniforms (16 bytes), must match `DeParams` in density_estimate.wgsl
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct DeParams {
    max_radius: f32,
    target_density: f32,
    max_level: f32,
    _pad: f32,
}

/// How much density estimation to apply, as one 0-1 amount.
///
/// One control with the internals derived, following `haze.rs` ("one amount,
/// band and falloffs derived"). flam3 exposes three knobs — `estimator`,
/// `estimator_curve`, `estimator_minimum` — and two of them have one sensible
/// value each; a user turning DE on wants "more" or "less", not a curve
/// exponent.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct DensityEstimation {
    /// 0 is off, and off means *not built* — no pyramid, no passes, and a
    /// render byte-identical to one from before this existed.
    pub amount: f32,
}

impl DensityEstimation {
    pub fn is_off(self) -> bool {
        !(self.amount > 0.0)
    }

    pub fn clamped(self) -> Self {
        Self { amount: self.amount.clamp(0.0, 1.0) }
    }

    /// Largest radius in accumulation texels, given the supersample factor.
    fn max_radius(self, supersample: u32) -> f32 {
        self.clamped().amount * MAX_RADIUS_PX * supersample.max(1) as f32
    }
}

pub struct DensityEstimator {
    copy_pipeline: RenderPipeline,
    reduce_pipeline: RenderPipeline,
    estimate_pipeline: RenderPipeline,
    layout: BindGroupLayout,
    params: Buffer,
    /// The pyramid: level 0 is a copy of the resolved density, each level above
    /// a 2x2 box average of the one below.
    ///
    /// Held rather than read: the views below borrow from it, and dropping the
    /// texture would invalidate every one of them.
    #[allow(dead_code)]
    pyramid: wgpu::Texture,
    /// One single-level view per level. The reduce pass binds level `k-1` as
    /// its source and renders into level `k`; disjoint subresources, so no
    /// aliasing.
    level_views: Vec<wgpu::TextureView>,
    /// All levels at once, for the estimate pass to reach any of them.
    all_levels: wgpu::TextureView,
    /// Where the estimate lands. A separate texture because the estimate reads
    /// every level of the pyramid while writing, so it cannot write into it.
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    levels: u32,
}

impl DensityEstimator {
    /// Build the pyramid and pipelines for a `width x height` density grid,
    /// which is the **accumulation** size.
    pub fn new(
        device: &Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        format: TextureFormat,
        de: DensityEstimation,
        supersample: u32,
    ) -> Self {
        // Enough levels for the largest radius asked for, never more. A tiny
        // radius does not pay for a deep pyramid.
        let want = (de.max_radius(supersample).max(2.0).log2().ceil() as u32 + 1).max(1);
        let fits = 32 - width.min(height).max(1).leading_zeros();
        let levels = want.min(fits).min(MAX_LEVELS).max(1);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("density_estimate_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/points/density_estimate.wgsl").into(),
            ),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("de_bind_group_layout"),
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
            label: Some("de_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make = |label: &str, entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
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
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let pyramid = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("de_pyramid"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let level_views = (0..levels)
            .map(|k| {
                pyramid.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("de_pyramid_level"),
                    base_mip_level: k,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let all_levels = pyramid.create_view(&wgpu::TextureViewDescriptor {
            label: Some("de_pyramid_all"),
            ..Default::default()
        });

        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("de_output"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("de_params"),
            size: std::mem::size_of::<DeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &params,
            0,
            bytemuck::bytes_of(&DeParams {
                max_radius: de.max_radius(supersample),
                target_density: de.clamped().amount * TARGET_DENSITY,
                max_level: (levels - 1) as f32,
                _pad: 0.0,
            }),
        );

        Self {
            copy_pipeline: make("de_copy_pipeline", "fs_copy"),
            reduce_pipeline: make("de_reduce_pipeline", "fs_reduce"),
            estimate_pipeline: make("de_estimate_pipeline", "fs_estimate"),
            layout,
            params,
            pyramid,
            level_views,
            all_levels,
            output,
            output_view,
            levels,
        }
    }

    fn bind(&self, device: &Device, view: &wgpu::TextureView) -> BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("de_bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry { binding: 1, resource: self.params.as_entire_binding() },
            ],
        })
    }

    fn pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        pipeline: &RenderPipeline,
        bind_group: &BindGroup,
        target: &wgpu::TextureView,
    ) {
        let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        p.set_pipeline(pipeline);
        p.set_bind_group(0, bind_group, &[]);
        p.draw(0..3, 0..1);
    }

    /// Blur `src` variably and return the result, still linear density at the
    /// same resolution.
    ///
    /// Encodes `levels + 1` fullscreen passes: one to copy the source into
    /// level 0, one per further level, and the estimate itself. The pyramid
    /// costs 4/3 of the base texture in bandwidth, so this is cheap next to the
    /// accumulation that produced `src`.
    pub fn pass_over(
        &self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
    ) -> &wgpu::TextureView {
        // Level 0 is the source, copied in. It cannot be the estimate pipeline
        // (that would apply DE twice) nor `fs_reduce` (which averages four
        // *different* texels and would halve the resolution).
        let copy_bind = self.bind(device, src);
        self.pass(encoder, "de_level0", &self.copy_pipeline, &copy_bind, &self.level_views[0]);

        for k in 1..self.levels as usize {
            let bind = self.bind(device, &self.level_views[k - 1]);
            self.pass(encoder, "de_reduce", &self.reduce_pipeline, &bind, &self.level_views[k]);
        }

        let bind = self.bind(device, &self.all_levels);
        self.pass(encoder, "de_estimate", &self.estimate_pipeline, &bind, &self.output_view);
        &self.output_view
    }

    #[allow(dead_code)]
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off must mean *off*, not "a radius so small it rounds away" — the
    /// caller skips building this entirely, and that is what keeps a render
    /// with no DE byte-identical to one from before DE existed.
    #[test]
    fn zero_amount_is_off_and_so_is_nonsense() {
        assert!(DensityEstimation { amount: 0.0 }.is_off());
        assert!(DensityEstimation::default().is_off());
        assert!(DensityEstimation { amount: -1.0 }.is_off());
        assert!(DensityEstimation { amount: f32::NAN }.is_off());
        assert!(!DensityEstimation { amount: 0.01 }.is_off());
    }

    /// The amount is in *output* pixels, so the same control has to mean the
    /// same blur at every supersample factor.
    #[test]
    fn the_radius_scales_with_supersampling() {
        let de = DensityEstimation { amount: 1.0 };
        assert_eq!(de.max_radius(1), MAX_RADIUS_PX);
        assert_eq!(de.max_radius(2), MAX_RADIUS_PX * 2.0);
        assert_eq!(de.max_radius(4), MAX_RADIUS_PX * 4.0);
    }

    #[test]
    fn the_amount_is_bounded() {
        assert_eq!(DensityEstimation { amount: 5.0 }.clamped().amount, 1.0);
        assert_eq!(DensityEstimation { amount: -5.0 }.clamped().amount, 0.0);
    }

    /// The radius law is the feature's whole behaviour, and it lives in WGSL
    /// where the compiler cannot see it. This pins the shape both sides agree
    /// on: at or past the target, no blur at all.
    #[test]
    fn at_the_target_density_the_blur_switches_off() {
        let wgsl = include_str!("../../../shaders/points/density_estimate.wgsl");
        assert!(
            wgsl.contains("sqrt(params.target_density / max(density, 1e-6))"),
            "the shader must use the sqrt(target/density) law this module feeds"
        );
        // sqrt(target/density) < 1 exactly when density > target, and the
        // shader returns the texel untouched below a radius of 1.
        let target = TARGET_DENSITY;
        assert!((target / (target + 1.0)).sqrt() < 1.0);
        assert!((target / (target - 1.0)).sqrt() > 1.0);
    }

    /// The neighbourhood probe is the difference between DE and a bloom filter,
    /// and it lives in WGSL where the compiler cannot see it.
    ///
    /// Without it, an empty texel beside a bright filament takes the widest
    /// radius on offer and gathers the filament outward. Measured: DE's effect
    /// did not diminish at all between 30 and 3000 samples per pixel, because
    /// empty texels stay empty however long you render. With it, the effect
    /// falls 13.6x over that range — which is what "DE retreats as a render
    /// converges" has to look like.
    #[test]
    fn the_radius_asks_its_neighbourhood() {
        let wgsl = include_str!("../../../shaders/points/density_estimate.wgsl");
        assert!(
            wgsl.contains("let effective = max(density, neighbourhood);"),
            "the radius must be set by the denser of the texel and its neighbourhood"
        );
    }

    /// A pyramid deeper than the largest radius is wasted passes and memory.
    #[test]
    fn the_pyramid_is_only_as_deep_as_the_radius_needs() {
        let small = DensityEstimation { amount: 0.05 };
        let big = DensityEstimation { amount: 1.0 };
        let depth = |de: DensityEstimation, n: u32| {
            (de.max_radius(n).max(2.0).log2().ceil() as u32 + 1).max(1)
        };
        assert!(depth(small, 1) < depth(big, 4), "a tiny radius must not build a deep pyramid");
        assert!(depth(big, 4) <= MAX_LEVELS + 4, "sanity");
    }
}
