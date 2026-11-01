//! Glass: the points as samples of an absorbing, tinted fog.
//!
//! See `GLASS-RENDERER-PLAN.md`. Each point is a flat disc of its own colour,
//! partly transparent, and near material partly hides far material. The rule
//! for how opaque a dot is, the compositing, and the reasons for both are in
//! the plan; `src/glass_proto.rs` is the CPU reference this transcribes.
//!
//! Three passes a pane:
//!
//! 1. **front** (compute, a thread a point): every fragment adds its tau to
//!    the pixel's total and raises its far depth; one nearer than the pixel's
//!    K-th is bubbled into its sorted K-list.
//! 2. **weigh** (compute, the same points again): every fragment works out
//!    the tau in front of it, exactly among the K nearest and by a straight
//!    line through the rest, and adds its colour and weight to two sums.
//! 3. **resolve** (a fullscreen triangle in the pane): opacity from the exact
//!    total, colour from the weighted mean.
//!
//! Rasterising in compute rather than through the hardware is what lets one
//! thread visit a dot's pixels with the prototype's own disc and feather, and
//! lets every per-pixel quantity be a plain atomic word.
//!
//! It knows nothing of the other renderers and shares only the point buffer,
//! the colormap and the mute/solo mask with them. Its bind groups are built
//! per call from whatever buffers are current, so a new ring needs no rebind.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::camera::Fov;
use crate::gpu::buffers::Alpha;
use crate::gpu::Viewport;

/// The live default for K, the fragments kept in depth order per pixel.
pub const DEFAULT_K: u32 = 8;
/// K's range in the Render window, as powers of two.
pub const MIN_K: u32 = 4;
pub const MAX_K: u32 = 64;
/// The default opacity of the material, sigma.
pub const DEFAULT_SIGMA: f32 = 1.2;
/// The opacity slider's range, logarithmic: most scenes want 0.3 to 10.
pub const MIN_SIGMA: f32 = 0.05;
pub const MAX_SIGMA: f32 = 32.0;
/// The largest disc drawn, as a radius in pixels. A dot that close is faint
/// per pixel under the dot rule anyway; past this it is cropped rather than
/// left to stall the thread that draws it.
const MAX_RADIUS_PX: f32 = 8.0;
/// A dot's radius as a share of the scene's `point_size`: the points
/// renderer's on-screen size (`point_size * point_scale * H / z` pixels
/// across), so one point size draws the same dot in both. The first build
/// used 0.5, 21% wider, and big dots blurred the surface into mush.
const DOT_RADIUS: f32 = 0.414_213_56; // tan(22.5 degrees)
/// Words a pixel in the sums buffer (`STRIDE` in glass_common.wgsl).
const STRIDE: u64 = 8;

const COMMON: &str = include_str!("../../../shaders/points/glass_common.wgsl");
const PASSES: &str = include_str!("../../../shaders/points/glass.wgsl");
const RESOLVE: &str = include_str!("../../../shaders/points/glass_resolve.wgsl");

/// `Glass` in glass_common.wgsl, field for field.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlassUniforms {
    view_proj: [[f32; 4]; 4],
    rect: [f32; 4],
    fb_width: u32,
    k: u32,
    point_count: u32,
    color_rgb_mode: u32,
    ppw_unit: f32,
    radius_world: f32,
    tau_point: f32,
    near: f32,
    max_radius_px: f32,
    color_contrast: f32,
    alpha_mode: f32,
    _pad0: f32,
    background: [f32; 4],
    guard: [f32; 4],
    guard_inv_ln_width: f32,
    _pad: [f32; 3],
}

/// What one pane's glass needs to know.
pub struct GlassFrame<'a> {
    pub view_proj: Mat4,
    /// The pane, in framebuffer pixels.
    pub pane: Viewport,
    pub fov: Fov,
    pub near: f32,
    /// The scene's point size, a world diameter (as `rendered_point_size`).
    pub point_size: f32,
    /// How many points are drawn: each carries 1/N of the material.
    pub point_count: u32,
    /// The material's opacity.
    pub sigma: f32,
    /// The scene's reference length L: tau_point = sigma * L^2 / N.
    pub length: f32,
    pub k: u32,
    pub color_contrast: f32,
    pub rgb_mode: bool,
    pub background: [f32; 3],
    pub alpha: Alpha,
    pub zoom: Option<&'a crate::renorm::Renorm>,
    /// The eye the zoom guard is measured from.
    pub guard_eye: Vec3,
}

pub struct GlassRenderer {
    front: wgpu::ComputePipeline,
    weigh: wgpu::ComputePipeline,
    compute_layout: wgpu::BindGroupLayout,
    resolve_layout: wgpu::BindGroupLayout,
    resolve_pipeline_layout: wgpu::PipelineLayout,
    resolve_module: wgpu::ShaderModule,
    /// One resolve pipeline per target format (the window, a screenshot).
    resolves: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    uniforms: wgpu::Buffer,
    /// Per pixel: tau total, far depth, the list's threshold and its tau, r, g, b, w.
    sums: wgpu::Buffer,
    /// Per pixel: K packed fragments.
    list: wgpu::Buffer,
    /// The framebuffer size and K the buffers are sized for.
    size: (u32, u32, u32),
    /// How many of the two compute passes run, for timing them apart.
    #[cfg(test)]
    pub passes: usize,
}

fn storage(binding: u32, read_only: bool, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl GlassRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let cs = wgpu::ShaderStages::COMPUTE;
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass_compute_layout"),
            entries: &[
                storage(0, true, cs),
                uniform(1, cs),
                storage(2, true, cs),
                storage(3, true, cs),
                storage(4, true, cs),
                storage(5, false, cs),
                storage(6, false, cs),
            ],
        });
        let fs = wgpu::ShaderStages::FRAGMENT;
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass_resolve_layout"),
            entries: &[uniform(0, fs), storage(1, true, fs)],
        });

        let passes = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass_passes"),
            source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{PASSES}").into()),
        });
        let compute_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glass_compute_pipeline_layout"),
            bind_group_layouts: &[Some(&compute_layout)],
            immediate_size: 0,
        });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&compute_pipeline_layout),
                module: &passes,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let front = compute("cs_front");
        let weigh = compute("cs_weigh");

        let resolve_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass_resolve"),
            source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{RESOLVE}").into()),
        });
        let resolve_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glass_resolve_pipeline_layout"),
            bind_group_layouts: &[Some(&resolve_layout)],
            immediate_size: 0,
        });

        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("glass_uniforms"),
            contents: bytemuck::bytes_of(&<GlassUniforms as bytemuck::Zeroable>::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let (sums, list) = Self::buffers(device, 1, 1, DEFAULT_K);
        Self {
            front,
            weigh,
            compute_layout,
            resolve_layout,
            resolve_pipeline_layout,
            resolve_module,
            resolves: Vec::new(),
            uniforms,
            sums,
            list,
            size: (1, 1, DEFAULT_K),
            #[cfg(test)]
            passes: 2,
        }
    }

    fn buffers(device: &wgpu::Device, w: u32, h: u32, k: u32) -> (wgpu::Buffer, wgpu::Buffer) {
        let px = w as u64 * h as u64;
        let make = |label, words: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (px * words * 4).max(16),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        (make("glass_sums", STRIDE), make("glass_list", k as u64))
    }

    /// The K this device can hold at this size: the asked-for one, halved
    /// until its list fits in one storage binding.
    fn fit_k(device: &wgpu::Device, w: u32, h: u32, k: u32) -> u32 {
        let limit = device.limits().max_storage_buffer_binding_size as u64;
        let px = (w as u64 * h as u64).max(1);
        let mut k = k.clamp(1, MAX_K);
        while k > 1 && px * k as u64 * 4 > limit {
            k /= 2;
        }
        k
    }

    /// Size the per-pixel buffers for this framebuffer and K. Returns the K
    /// actually used.
    fn ensure(&mut self, device: &wgpu::Device, w: u32, h: u32, k: u32) -> u32 {
        let k = Self::fit_k(device, w, h, k);
        if self.size != (w, h, k) {
            let (sums, list) = Self::buffers(device, w, h, k);
            self.sums = sums;
            self.list = list;
            self.size = (w, h, k);
        }
        k
    }

    fn resolve_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        if let Some(i) = self.resolves.iter().position(|(f, _)| *f == format) {
            return &self.resolves[i].1;
        }
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glass_resolve"),
            layout: Some(&self.resolve_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.resolve_module,
                entry_point: Some("vs_full"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.resolve_module,
                entry_point: Some("fs_resolve"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            // The pass clears the depth buffer (the overlay reads it for the
            // gizmos) and the resolve writes none: glass hides no gizmo yet.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: super::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.resolves.push((format, pipeline));
        &self.resolves.last().unwrap().1
    }

    /// Draw one pane of glass into `target`. The uniforms land at the next
    /// submit, so a second pane must be a submit of its own, as with the
    /// other renderers.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        load: wgpu::LoadOp<wgpu::Color>,
        frame: &GlassFrame,
        points: &wgpu::Buffer,
        colormap: &wgpu::Buffer,
        mask: &super::mask::Binding,
    ) {
        let (fb_w, fb_h) = size;
        let k = self.ensure(device, fb_w, fb_h, frame.k);
        let p = frame.pane;
        let n = frame.point_count.max(1);
        let length = frame.length.max(1e-6);
        let (guard, guard_inv) = match frame.zoom {
            Some(z) => {
                let (ln_near, inv) = z.guard_params(frame.guard_eye);
                (z.fixed_point.extend(ln_near).to_array(), inv)
            }
            None => ([0.0; 4], 0.0),
        };
        let u = GlassUniforms {
            view_proj: frame.view_proj.to_cols_array_2d(),
            rect: [p.x as f32, p.y as f32, p.w as f32, p.h as f32],
            fb_width: fb_w,
            k,
            point_count: frame.point_count,
            color_rgb_mode: frame.rgb_mode as u32,
            ppw_unit: p.h as f32 / (2.0 * frame.fov.tan_half()),
            // A dot holds its world size whatever the lens: glass projects
            // with the true tangent, so `Fov::point_scale` (the other
            // renderers' correction for projecting without it) doesn't apply.
            // Applied, a 1-degree axis pane's dots came out 47 times too big.
            radius_world: DOT_RADIUS * frame.point_size,
            tau_point: frame.sigma * length * length / n as f32,
            near: frame.near,
            max_radius_px: MAX_RADIUS_PX,
            color_contrast: frame.color_contrast,
            alpha_mode: frame.alpha.shader_value(),
            _pad0: 0.0,
            background: [frame.background[0], frame.background[1], frame.background[2], 1.0],
            guard,
            guard_inv_ln_width: guard_inv,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));

        // Clear the pane's rows. Whole rows, so one range a buffer: the other
        // panes on them are already resolved, in an earlier submit.
        let rows = |words: u64| {
            let row = fb_w as u64 * words * 4;
            (p.y as u64 * row, p.h as u64 * row)
        };
        let (off, len) = rows(STRIDE);
        encoder.clear_buffer(&self.sums, off, Some(len));
        let (off, len) = rows(k as u64);
        encoder.clear_buffer(&self.list, off, Some(len));

        if frame.point_count > 0 {
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glass_compute"),
                layout: &self.compute_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: points.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: colormap.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: mask.maps.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: mask.mask.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: self.sums.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: self.list.as_entire_binding() },
                ],
            });
            // A thread a point, in two dimensions past the 65535-group limit.
            let groups = frame.point_count.div_ceil(256);
            let gx = groups.min(32768);
            let gy = groups.div_ceil(gx);
            #[cfg(test)]
            let passes = self.passes;
            #[cfg(not(test))]
            let passes = 2;
            for (label, pipeline) in [("glass_front", &self.front), ("glass_weigh", &self.weigh)].into_iter().take(passes) {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(gx, gy, 1);
            }
        }

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glass_resolve"),
            layout: &self.resolve_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.sums.as_entire_binding() },
            ],
        });
        let pipeline = self.resolve_pipeline(device, format);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("glass_resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        p.apply(&mut pass);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniforms_are_the_size_the_shader_expects() {
        // 64 (matrix) + 16 (rect) + 16 + 16 + 16 + 16 (background) + 16
        // (guard) + 16: `Glass` in glass_common.wgsl.
        assert_eq!(std::mem::size_of::<GlassUniforms>(), 176);
    }
}
