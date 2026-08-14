//! Persistent accumulation histogram: the thing that removes the sample
//! ceiling.
//!
//! The chaos game writes into a *circular* point buffer, so however long a
//! render runs, the distinct samples in the finished image number exactly the
//! buffer's capacity — `--accumulate` only changes which samples survive, never
//! how many. Measured on the reference desktop, a 1.45 s run generates 2.1e9
//! samples and discards 95% of them.
//!
//! Here the ring becomes a streaming working set. Each batch of fresh points is
//! splatted into a transient `Rgba16Float` texture, added into a storage buffer
//! that outlives the ring, and then may be overwritten freely — the buffer
//! already has it. Sample count is now a function of *time*, and the render is
//! an anytime algorithm in the strong sense: stopping early gives the same
//! picture, noisier.
//!
//! See `shaders/points/accumulate.wgsl` for why the accumulator is 64-bit fixed
//! point rather than f32 or a 32-bit fixed point that would fit in half the
//! memory. The short version: f32 *stalls* — once a hot texel's running sum
//! exceeds an increment by 2^24 the increment stops registering, and the
//! sum-to-increment ratio is just the batch count — while 32 bits cannot
//! simultaneously resolve a large gaussian's faint tail and hold a bright core.

use wgpu::{BindGroup, BindGroupLayout, Buffer, ComputePipeline, Device, RenderPipeline};

use crate::gpu::points::density::{DensityEstimation, DensityEstimator};
use crate::gpu::points::downsample::{Downsampler, Filter, Source};

/// Bytes of accumulator per texel: four channels x 64 bits.
pub const BYTES_PER_TEXEL: u64 = 32;

/// Fixed-point scale. Density is stored as `round(value * SCALE)`.
///
/// 1/65536 resolves the faint outskirts of a large gaussian splat (~1.6e-4 of a
/// sample) while leaving 2^48 samples of headroom in the high word, which no
/// render reaches.
const SCALE: f32 = 65536.0;

/// The resolved density texture's format.
///
/// `Rgba32Float` and not the `Rgba16Float` the rest of the splat path uses:
/// fp16 tops out at 65504 and an accumulated density passes that early in a run
/// that is worth accumulating at all. It is still linear density, not a
/// picture — the log tonemap has not run.
pub const RESOLVED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// Uniforms (16 bytes), must match `AccumParams` in accumulate.wgsl
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct AccumParams {
    width: u32,
    height: u32,
    scale: f32,
    _pad: u32,
}

pub struct Accumulator {
    accum: Buffer,
    params_buffer: Buffer,
    add_pipeline: ComputePipeline,
    add_layout: BindGroupLayout,
    resolve_pipeline: RenderPipeline,
    resolve_bind_group: BindGroup,
    resolved_texture: wgpu::Texture,
    resolved_view: wgpu::TextureView,
    /// The reconstruction filter and the output-sized texture it writes, when
    /// supersampling is on. `None` at 1x, where the resolve is already the
    /// output size.
    ///
    /// It lives here rather than in `SplatRenderer` because in this path the
    /// filter's input is the *histogram*, resolved once at the end — not the
    /// per-batch splat texture. Filtering each batch and summing the results
    /// would be a different and worse thing: the kernel is not idempotent
    /// under summation the way the histogram is, and every batch would pay
    /// the filter's tap count.
    filtered: Option<Filtered>,
    /// Density estimation, when asked for. Runs between the resolve and the
    /// filter: on linear density, at accumulation resolution, before the log.
    /// `None` when the amount is zero, so an ordinary render allocates no
    /// pyramid and encodes no extra passes.
    de: Option<DensityEstimator>,
    /// Accumulation size — the *supersampled* size when supersampling is on.
    width: u32,
    height: u32,
}

struct Filtered {
    downsampler: Downsampler,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Accumulator {
    /// Allocate an accumulator over a `width x height` texel grid, which is the
    /// supersampled size when supersampling is on.
    ///
    /// Fails rather than panicking when the buffer would exceed what the device
    /// will bind: at 32 bytes a texel this is the resource that decides whether
    /// a large accumulating render runs at all, and "your GPU will not bind
    /// 1.1 GB" deserves to arrive as a sentence naming the two dials that fix
    /// it, not as a validation error from inside wgpu.
    pub fn new(
        device: &Device,
        queue: &wgpu::Queue,
        out_width: u32,
        out_height: u32,
        supersample: u32,
        filter: Filter,
        filter_radius: f32,
        de: DensityEstimation,
    ) -> Result<Self, String> {
        let n = supersample.max(1);
        let (width, height) = (out_width * n, out_height * n);
        let texels = width as u64 * height as u64;
        let size = texels * BYTES_PER_TEXEL;
        let limit = device.limits().max_storage_buffer_binding_size;
        if size > limit {
            return Err(format!(
                "accumulation histogram needs {:.1} GB ({}x{} texels x {} bytes) but this GPU \
                 binds at most {:.1} GB — render smaller, or lower --supersample",
                size as f64 / 1e9,
                width,
                height,
                BYTES_PER_TEXEL,
                limit as f64 / 1e9,
            ));
        }

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("accumulate_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/points/accumulate.wgsl").into(),
            ),
        });

        let accum = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accum_histogram"),
            size,
            // COPY_SRC as well, so a run can be checkpointed: this buffer *is*
            // the accumulated state, and reading it out is what makes a long
            // render resumable rather than restartable.
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accum_params"),
            size: std::mem::size_of::<AccumParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Written once: the grid and the scale are fixed for the run.
        queue.write_buffer(
            &params_buffer,
            0,
            bytemuck::bytes_of(&AccumParams { width, height, scale: SCALE, _pad: 0 }),
        );

        // --- add pass: batch texture + accumulator + params ---
        let add_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("accum_add_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let add_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("accum_add_pipeline_layout"),
                bind_group_layouts: &[Some(&add_layout)],
                immediate_size: 0,
            });
        let add_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("accum_add_pipeline"),
            layout: Some(&add_pipeline_layout),
            module: &shader,
            entry_point: Some("accumulate"),
            compilation_options: Default::default(),
            cache: None,
        });

        // --- resolve pass: accumulator (read-only) + params -> float texture ---
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("accum_resolve_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
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
        let resolve_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("accum_resolve_pipeline_layout"),
                bind_group_layouts: &[Some(&resolve_layout)],
                immediate_size: 0,
            });
        let resolve_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("accum_resolve_pipeline"),
            layout: Some(&resolve_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_resolve"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_resolve"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: RESOLVED_FORMAT,
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

        let resolve_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("accum_resolve_bind_group"),
            layout: &resolve_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: accum.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: params_buffer.as_entire_binding() },
            ],
        });

        let make_float_target = |label, w, h| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: RESOLVED_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        // So the grade buffer can be read back off it.
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
        };
        let resolved = make_float_target("accum_resolved_texture", width, height);
        let filtered = (n > 1).then(|| {
            let downsampler = Downsampler::new(device, RESOLVED_FORMAT);
            // The histogram is the splat's additive `(colour * weight, weight)`
            // summed over every batch, so it filters channel by channel and the
            // tonemap's `rgb / a` still recovers the density-weighted mean.
            downsampler.upload_params(queue, n, filter, filter_radius, Source::Additive);
            let texture = make_float_target("accum_filtered_texture", out_width, out_height);
            Filtered {
                downsampler,
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                texture,
            }
        });

        let de = (!de.is_off()).then(|| {
            DensityEstimator::new(device, queue, width, height, RESOLVED_FORMAT, de, n)
        });

        Ok(Self {
            accum,
            params_buffer,
            add_pipeline,
            add_layout,
            resolve_pipeline,
            resolve_bind_group,
            resolved_view: resolved.create_view(&wgpu::TextureViewDescriptor::default()),
            resolved_texture: resolved,
            filtered,
            de,
            width,
            height,
        })
    }

    /// Bind a batch texture for the add pass.
    ///
    /// Called once and reused for every batch, rather than rebuilt inside
    /// `add`: the batch texture does not change across a run, and a run is
    /// thousands of batches.
    pub fn bind_batch(&self, device: &Device, batch: &wgpu::TextureView) -> BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("accum_add_bind_group"),
            layout: &self.add_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(batch),
                },
                wgpu::BindGroupEntry { binding: 1, resource: self.accum.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// Fold one batch into the histogram.
    pub fn add(&self, encoder: &mut wgpu::CommandEncoder, batch_bind_group: &BindGroup) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("accum_add_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.add_pipeline);
        pass.set_bind_group(0, batch_bind_group, &[]);
        pass.dispatch_workgroups(self.width.div_ceil(8), self.height.div_ceil(8), 1);
    }

    /// Read the histogram back out as **output-sized linear density**, once, at
    /// the end: resolve the fixed-point pairs to floats, then run the
    /// reconstruction filter if supersampling is on.
    ///
    /// Still linear, still pre-tonemap — the caller hands the returned view
    /// straight to `SplatRenderer::tonemap_pass`.
    pub fn resolve(
        &self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> &wgpu::TextureView {
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("accum_resolve_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.resolved_view,
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
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, &self.resolve_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        // Density estimation first, then the reconstruction filter. Both are
        // blurs and the order is not arbitrary: DE varies its width from the
        // *unblurred* density, so it has to see the histogram as accumulated,
        // and the filter's job — resolving N x supersampling down to output
        // pixels — is the same either way.
        let density = match &self.de {
            Some(de) => de.pass_over(device, encoder, &self.resolved_view),
            None => &self.resolved_view,
        };
        match &self.filtered {
            Some(f) => {
                f.downsampler.pass(device, encoder, density, &f.view);
                &f.view
            }
            None => density,
        }
    }

    /// The texture `resolve` last wrote — output-sized linear density, and
    /// exactly the tonemap's input. What the grade buffer is read back from.
    pub fn output_texture(&self) -> &wgpu::Texture {
        match (&self.filtered, &self.de) {
            (Some(f), _) => &f.texture,
            // No filter, so whatever the last pass wrote is the output — which
            // is DE's own target when DE ran.
            (None, Some(de)) => de.output_texture(),
            (None, None) => &self.resolved_texture,
        }
    }

    /// The accumulation grid, which is the *supersampled* size.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Read the histogram out to CPU words, for a checkpoint.
    ///
    /// Blocking and not cheap — 265 MB at 1080p / 2x — so it happens once, at
    /// the end of a run. That is also the only time it is meaningful: the
    /// buffer is only a coherent picture between folds.
    pub fn read_back(&self, device: &Device, queue: &wgpu::Queue) -> Vec<u32> {
        let size = self.width as u64 * self.height as u64 * BYTES_PER_TEXEL;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accum_checkpoint_readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("accum_checkpoint_encoder"),
        });
        encoder.copy_buffer_to_buffer(&self.accum, 0, &staging, 0, size);
        queue.submit(std::iter::once(encoder.finish()));

        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let out = {
            let data = slice.get_mapped_range();
            bytemuck::cast_slice::<u8, u32>(&data).to_vec()
        };
        staging.unmap();
        out
    }

    /// Load a saved histogram back in, replacing whatever is there.
    ///
    /// The caller has already checked the geometry matches (see
    /// `Checkpoint::check_compatible`); this only guards the raw length, which
    /// would otherwise be a partial write leaving half the image stale.
    pub fn restore(&self, queue: &wgpu::Queue, words: &[u32]) -> Result<(), String> {
        let expected = self.width as usize * self.height as usize
            * (BYTES_PER_TEXEL as usize / 4);
        if words.len() != expected {
            return Err(format!(
                "checkpoint holds {} words but this {}x{} accumulation needs {}",
                words.len(),
                self.width,
                self.height,
                expected
            ));
        }
        queue.write_buffer(&self.accum, 0, bytemuck::cast_slice(words));
        Ok(())
    }

    /// Zero the histogram. Called once at the start of a run and **never
    /// between batches** — clearing mid-run is exactly the bug this module
    /// exists to prevent, and would silently reduce an overnight render to the
    /// last batch.
    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.accum, 0, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader and this module both hardcode the texel stride; one is opaque
    /// to the compiler.
    #[test]
    fn the_shader_agrees_about_the_texel_stride() {
        let wgsl = include_str!("../../../shaders/points/accumulate.wgsl");
        let words = BYTES_PER_TEXEL / 4;
        assert!(
            wgsl.contains(&format!("const WORDS_PER_TEXEL: u32 = {}u;", words)),
            "accumulate.wgsl must declare WORDS_PER_TEXEL = {}", words
        );
    }

    /// The dispatch geometry in `add` and the `@workgroup_size` in the shader
    /// are one fact in two languages.
    #[test]
    fn the_shader_agrees_about_the_workgroup_size() {
        let wgsl = include_str!("../../../shaders/points/accumulate.wgsl");
        assert!(wgsl.contains("@compute @workgroup_size(8, 8)"));
    }

    /// The scale has to resolve a large gaussian's faint tail without the high
    /// word ever being reachable. Both ends, stated as arithmetic.
    #[test]
    fn the_fixed_point_scale_spans_tail_to_core() {
        // A tail contribution of 1.6e-4 of a sample must survive rounding.
        assert!((1.6e-4_f32 * SCALE).round() >= 10.0);
        // 2^48 samples of headroom: unreachable at any rate for any duration.
        let max_samples = 2f64.powi(64) / SCALE as f64;
        assert!(max_samples > 1e14);
    }
}
