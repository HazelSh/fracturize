//! Simple chaos game compute pipeline
//!
//! Runs the IFS chaos game and writes points directly to a buffer.
//! Uses a circular buffer approach for temporal stability.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::gpu::buffers::{GpuTransform, Point, PointComputeParams};
use crate::scene::TransformSpec;

/// Number of threads per workgroup (must match shader)
const WORKGROUP_SIZE: u32 = 256;

/// Per-walker state (48 bytes, one per parallel walker)
/// Must match WGSL WalkerState struct
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct WalkerState {
    pub current_pos: [f32; 3],
    pub _pad0: f32,
    pub current_color: f32,
    pub _pad1: f32,
    pub _pad2: f32,
    pub _pad3: f32,
    pub rng_state: [u32; 4],
}

impl WalkerState {
    pub fn new(seed: u64) -> Self {
        let mut state = [
            (seed & 0xFFFFFFFF) as u32,
            ((seed >> 32) & 0xFFFFFFFF) as u32,
            seed.wrapping_mul(0x5DEECE66D) as u32,
            (seed.wrapping_mul(0x5DEECE66D) >> 32) as u32,
        ];
        for s in &mut state {
            if *s == 0 {
                *s = 0xDEADBEEF;
            }
        }

        Self {
            current_pos: [0.0, 0.0, 0.0],
            _pad0: 0.0,
            current_color: 0.5,
            _pad1: 0.0,
            _pad2: 0.0,
            _pad3: 0.0,
            rng_state: state,
        }
    }
}

/// Simple chaos game compute pipeline
/// Writes points directly to a circular buffer
pub struct PointCompute {
    pub pipeline: wgpu::ComputePipeline,
    /// Carries the whole buffer through a camera wrap; see `rewrap` in
    /// `shaders/points/chaos.wgsl`. Same module, same bind group, same
    /// layout — only the entry point differs.
    pub rewrap_pipeline: wgpu::ComputePipeline,
    pub bind_group: wgpu::BindGroup,
    pub transform_buffer: wgpu::Buffer,
    /// Symmetry group elements, all groups concatenated. Fixed capacity
    /// ([`MAX_GROUP_ELEMENTS`]), so a group change is a write, not a rebuild.
    pub group_buffer: wgpu::Buffer,
    pub walker_states_buffer: wgpu::Buffer,
    pub params_buffer: wgpu::Buffer,
    pub point_buffer: wgpu::Buffer,
    pub colormap_buffer: wgpu::Buffer,
    pub num_transforms: u32,
    pub num_walkers: u32,
    pub num_workgroups: u32,
    pub iterations_per_walker: u32,
    /// Higher iteration count used during warmup to fill buffer faster
    pub warmup_iterations_per_walker: u32,
    pub buffer_capacity: u32,
    pub write_offset: u32,
    pub warmup_frames: u32,
    pub current_frame: u32,
    /// Infinite-zoom renormalization applied to every emitted point, if the
    /// scene asked for one (see `renorm.rs`). Rebuilt on transform edits.
    pub zoom: Option<crate::renorm::Renorm>,
}

/// A transform's colour, or white if the parallel `colors` array is short.
/// It can be, briefly: adding a transform pushes onto `transforms` and
/// `colors` separately, and a redraw can land between the two.
fn transform_color(colors: &[glam::Vec3], i: usize) -> glam::Vec3 {
    colors.get(i).copied().unwrap_or(glam::Vec3::ONE)
}

/// Slots in the symmetry group-element buffer.
///
/// Fixed capacity, allocated once, deliberately. The alternative — sizing the
/// buffer to the scene — makes every symmetry edit a bind-group rebuild, and
/// changing `C5` to `I` is an edit you make *by dragging a picker*, so it has to
/// be as cheap as moving a slider. At 64 bytes an element the whole table is
/// 32 KB, which is nothing next to a point buffer measured in hundreds of
/// megabytes, and it holds the largest group this can name (`I` with a mirror,
/// 120) four times over.
pub const MAX_GROUP_ELEMENTS: usize = 512;

/// Every distinct symmetry group in a scene, laid end to end, with each
/// transform's slice into it.
///
/// Distinct is the operative word: three motifs enrolled in one icosahedral
/// group share one 60-element run, so the table is sized by how many *different*
/// groups a scene uses, not by how many maps carry one.
pub struct GroupTable {
    /// The elements, concatenated. Never longer than [`MAX_GROUP_ELEMENTS`].
    pub elements: Vec<[[f32; 4]; 4]>,
    /// `(start, count)` per transform, parallel to the transform list.
    /// `(0, 0)` for a transform with no symmetry.
    pub slices: Vec<(u32, u32)>,
}

impl GroupTable {
    /// Collect the groups a transform list carries.
    pub fn build(transforms: &[TransformSpec]) -> Self {
        let mut elements: Vec<[[f32; 4]; 4]> = Vec::new();
        let mut slices = Vec::with_capacity(transforms.len());
        // Small and linear: a scene has a handful of distinct groups at most,
        // and comparing two `Symmetry`s is four field comparisons (the elements
        // are a pure function of them, so they never have to be walked).
        let mut seen: Vec<(&crate::symmetry::Symmetry, (u32, u32))> = Vec::new();

        for t in transforms {
            let Some(sym) = t.symmetry.as_ref() else {
                slices.push((0, 0));
                continue;
            };
            if let Some((_, slice)) = seen.iter().find(|(s, _)| *s == sym) {
                slices.push(*slice);
                continue;
            }
            if elements.len() + sym.order() > MAX_GROUP_ELEMENTS {
                // Unreachable through the loader, which rejects a scene whose
                // groups don't fit; a live edit that somehow got here renders
                // that motif unsymmetrized rather than overrunning the buffer.
                log::warn!(
                    "symmetry group {} does not fit in the {}-element table; \
                     rendering this motif without it",
                    sym.label(),
                    MAX_GROUP_ELEMENTS
                );
                slices.push((0, 0));
                continue;
            }
            let slice = (elements.len() as u32, sym.order() as u32);
            elements.extend(sym.elements().iter().map(|m| m.to_cols_array_2d()));
            seen.push((sym, slice));
            slices.push(slice);
        }

        Self { elements, slices }
    }

    /// The buffer contents: the table padded out to the fixed capacity.
    /// Element 0 of an unused run is never read (`group_count` is 0), so the
    /// padding's value doesn't matter — but it must be *written*, or the first
    /// upload leaves the tail uninitialized.
    fn padded(&self) -> Vec<[[f32; 4]; 4]> {
        let mut all = self.elements.clone();
        all.resize(MAX_GROUP_ELEMENTS, glam::Mat4::IDENTITY.to_cols_array_2d());
        all
    }
}

impl PointCompute {
    pub fn new(
        device: &wgpu::Device,
        transforms: &[TransformSpec],
        // Per-transform linear RGB, parallel to `transforms`. Only read in
        // `ColorMode::Mix`, but uploaded always: the mode is a uniform flip
        // at draw time, so the buffer has to be ready for either.
        colors: &[glam::Vec3],
        colormap: &[[f32; 4]; 256],
        buffer_capacity: u32,
    ) -> Self {
        // Calculate parallelism - aim for ~0.125% of buffer per frame (full cycle every 800 frames)
        let points_per_frame = (buffer_capacity / 800).max(1000);
        let num_workgroups = 64u32;
        let num_walkers = num_workgroups * WORKGROUP_SIZE;
        let iterations_per_walker = ((points_per_frame + num_walkers - 1) / num_walkers).max(1);
        let actual_points_per_frame = num_walkers * iterations_per_walker;
        
        // Warmup uses 10x more iterations to fill buffer in ~80 frames instead of 800
        let warmup_iterations_per_walker = iterations_per_walker * 10;
        let warmup_points_per_frame = num_walkers * warmup_iterations_per_walker;
        let warmup_frames = (buffer_capacity + warmup_points_per_frame - 1) / warmup_points_per_frame;

        log::info!(
            "Point compute: {} walkers × {} iters = {} points/frame (warmup: {} iters, {} frames to fill)",
            num_walkers, iterations_per_walker, actual_points_per_frame, warmup_iterations_per_walker, warmup_frames
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point_chaos_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/points/chaos.wgsl").into()),
        });

        // Prepare transform data with cumulative weights
        let groups = GroupTable::build(transforms);
        let total_weight: f32 = transforms.iter().map(|t| t.weight).sum();
        let mut cumulative = 0.0;
        let gpu_transforms: Vec<GpuTransform> = transforms
            .iter()
            .enumerate()
            .map(|(i, t)| {
                cumulative += t.weight / total_weight;
                GpuTransform::new(
                    t,
                    cumulative,
                    t.weight,
                    transform_color(colors, i),
                    groups.slices.get(i).copied().unwrap_or((0, 0)),
                )
            })
            .collect();

        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("transform_buffer"),
            contents: bytemuck::cast_slice(&gpu_transforms),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // The symmetry groups. Fixed capacity so that changing a group is a
        // buffer write rather than a pipeline rebuild — see MAX_GROUP_ELEMENTS.
        let group_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("symmetry_group_buffer"),
            contents: bytemuck::cast_slice(&groups.padded()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Initialize walker states with different seeds
        let walker_states: Vec<WalkerState> = (0..num_walkers)
            .map(|i| WalkerState::new((i as u64).wrapping_mul(0x9E3779B97F4A7C15)))
            .collect();

        let walker_states_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("walker_states_buffer"),
            contents: bytemuck::cast_slice(&walker_states),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Parameters uniform
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_compute_params"),
            size: std::mem::size_of::<PointComputeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Point buffer - the main output
        let point_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_buffer"),
            size: (buffer_capacity as usize * std::mem::size_of::<Point>()) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // Colormap buffer
        let colormap_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("colormap_buffer"),
            contents: bytemuck::cast_slice(colormap),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("point_compute_bind_group_layout"),
            entries: &[
                // Points output buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Transforms
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Walker states
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Params
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Symmetry group elements
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_compute_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("point_compute_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let rewrap_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("point_rewrap_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("rewrap"),
            compilation_options: Default::default(),
            cache: None,
        });

        // All buffers are fixed for the lifetime of the pipeline, so the bind
        // group can be created once instead of per frame
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_compute_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: point_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: transform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: walker_states_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: group_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            pipeline,
            rewrap_pipeline,
            bind_group,
            transform_buffer,
            group_buffer,
            walker_states_buffer,
            params_buffer,
            point_buffer,
            colormap_buffer,
            num_transforms: transforms.len() as u32,
            num_walkers,
            num_workgroups,
            iterations_per_walker,
            warmup_iterations_per_walker,
            buffer_capacity,
            write_offset: 0,
            warmup_frames,
            current_frame: 0,
            zoom: None,
        }
    }

    /// Whether we're still in warmup phase (buffer not yet full)
    pub fn is_warming_up(&self) -> bool {
        self.current_frame < self.warmup_frames
    }


    /// Returns number of valid points to render this frame
    pub fn valid_point_count(&self) -> u32 {
        if self.is_warming_up() {
            // During warmup, calculate total written accounting for warmup rate
            let total_written = self.current_frame * self.num_walkers * self.warmup_iterations_per_walker;
            total_written.min(self.buffer_capacity)
        } else {
            // After warmup, buffer is always full
            self.buffer_capacity
        }
    }

    /// Update for next frame - advances write offset and returns valid point
    /// count. `dt` is the wall-clock frame time: after warmup, the churn rate
    /// (points regenerated per second) is normalized to it, so a 120 Hz
    /// display refreshes the buffer at the same real-time rate as 60 Hz
    /// (the historical baseline: `iterations_per_walker` per 1/60 s).
    pub fn advance_frame(&mut self, queue: &wgpu::Queue, dt: f32) -> u32 {
        let current_iters = if self.is_warming_up() {
            // Warmup ignores dt: fill the buffer as fast as possible
            self.warmup_iterations_per_walker
        } else {
            let target = self.iterations_per_walker as f32 * (dt * 60.0);
            (target.round() as u32).clamp(1, self.warmup_iterations_per_walker)
        };

        // Upload params with current write offset and iteration count
        let params = PointComputeParams {
            num_transforms: self.num_transforms,
            num_walkers: self.num_walkers,
            iterations_per_walker: current_iters,
            write_offset: self.write_offset,
            buffer_capacity: self.buffer_capacity,
            ..PointComputeParams::zeroed()
        }
        .with_zoom(self.zoom.as_ref());
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        // Advance for next frame
        self.write_offset =
            (self.write_offset + self.num_walkers * current_iters) % self.buffer_capacity;
        self.current_frame += 1;

        self.valid_point_count()
    }

    /// Carry the whole point buffer through `levels` zoom periods, so the
    /// points move with a camera that just wrapped and every dot keeps its
    /// pixel. See `rewrap` in `shaders/points/chaos.wgsl` for why.
    ///
    /// Submits on its own rather than joining the frame's encoder: a wrap
    /// happens once a period — every several seconds, at the frame rates this
    /// is watched at — and the alternative is threading a pending flag through
    /// the update/render split for something that costs one pass over the
    /// buffer. It must land *before* the frame's chaos dispatch, which writes
    /// fresh points into the band this pass is re-folding, and queue order
    /// gives that for free.
    ///
    /// A no-op without a zoom, or when nothing wrapped.
    pub fn rewrap(&self, device: &wgpu::Device, queue: &wgpu::Queue, levels: i32) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("point_rewrap_encoder"),
        });
        self.rewrap_in(queue, &mut encoder, levels);
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// The same, recorded into a caller's encoder. The window uses this: the
    /// pass belongs to the frame that wrapped, and a submit of its own would
    /// put a queue flush in the middle of it.
    pub fn rewrap_in(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        levels: i32,
    ) {
        if levels == 0 || self.zoom.is_none() {
            return;
        }
        let params = PointComputeParams {
            num_transforms: self.num_transforms,
            num_walkers: self.num_walkers,
            buffer_capacity: self.buffer_capacity,
            ..PointComputeParams::zeroed()
        }
        .with_zoom(self.zoom.as_ref());
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&PointComputeParams { zoom_rewrap: levels as f32, ..params }),
        );

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("point_rewrap_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.rewrap_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(self.buffer_capacity.div_ceil(WORKGROUP_SIZE), 1, 1);
    }

    /// Dispatch the compute shader
    pub fn dispatch(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("point_chaos_pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&self.pipeline);
        compute_pass.set_bind_group(0, &self.bind_group, &[]);
        compute_pass.dispatch_workgroups(self.num_workgroups, 1, 1);
    }

    /// Reupload transform weights with some transforms disabled.
    ///
    /// Also re-uploads the symmetry groups, because a live edit can change one
    /// (the panel's group picker) and there is no cheaper place to notice: the
    /// table is 32 KB and this already runs only on edits, never per frame.
    pub fn update_weights(
        &self,
        queue: &wgpu::Queue,
        transforms: &[TransformSpec],
        colors: &[glam::Vec3],
        enabled: &[bool],
    ) {
        let groups = GroupTable::build(transforms);
        let total_weight: f32 = transforms
            .iter()
            .zip(enabled.iter())
            .map(|(t, &on)| if on { t.weight } else { 0.0 })
            .sum();

        let mut cumulative = 0.0;
        let gpu_transforms: Vec<GpuTransform> = transforms
            .iter()
            .zip(enabled.iter())
            .enumerate()
            .map(|(i, (t, &on))| {
                let effective_weight = if on { t.weight } else { 0.0 };
                if total_weight > 0.0 {
                    cumulative += effective_weight / total_weight;
                }
                GpuTransform::new(
                    t,
                    cumulative,
                    effective_weight,
                    transform_color(colors, i),
                    groups.slices.get(i).copied().unwrap_or((0, 0)),
                )
            })
            .collect();

        queue.write_buffer(&self.transform_buffer, 0, bytemuck::cast_slice(&gpu_transforms));
        queue.write_buffer(&self.group_buffer, 0, bytemuck::cast_slice(&groups.padded()));
    }

    /// Re-upload the colormap after live color edits. Existing points keep
    /// their colormap indices, so they recolor immediately.
    pub fn update_colormap(&self, queue: &wgpu::Queue, colormap: &[[f32; 4]; 256]) {
        queue.write_buffer(&self.colormap_buffer, 0, bytemuck::cast_slice(colormap));
    }

    /// Reset to initial state (for scene reload or reset key)
    pub fn reset(&mut self, queue: &wgpu::Queue) {
        self.write_offset = 0;
        self.current_frame = 0;

        // Re-initialize walker states
        let walker_states: Vec<WalkerState> = (0..self.num_walkers)
            .map(|i| WalkerState::new((i as u64).wrapping_mul(0x9E3779B97F4A7C15)))
            .collect();
        queue.write_buffer(&self.walker_states_buffer, 0, bytemuck::cast_slice(&walker_states));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_walker_state_size() {
        assert_eq!(std::mem::size_of::<WalkerState>(), 48, "WalkerState must be 48 bytes to match WGSL struct");
    }
}
