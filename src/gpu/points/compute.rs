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
    pub bind_group: wgpu::BindGroup,
    pub transform_buffer: wgpu::Buffer,
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
}

impl PointCompute {
    pub fn new(
        device: &wgpu::Device,
        transforms: &[TransformSpec],
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
        let total_weight: f32 = transforms.iter().map(|t| t.weight).sum();
        let mut cumulative = 0.0;
        let gpu_transforms: Vec<GpuTransform> = transforms
            .iter()
            .map(|t| {
                cumulative += t.weight / total_weight;
                GpuTransform::new(t, cumulative, t.weight)
            })
            .collect();

        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("transform_buffer"),
            contents: bytemuck::cast_slice(&gpu_transforms),
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
            usage: wgpu::BufferUsages::STORAGE,
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
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_compute_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
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
            ],
        });

        Self {
            pipeline,
            bind_group,
            transform_buffer,
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
        }
    }

    /// Whether we're still in warmup phase (buffer not yet full)
    pub fn is_warming_up(&self) -> bool {
        self.current_frame < self.warmup_frames
    }

    /// Current iterations per walker (higher during warmup)
    pub fn current_iterations(&self) -> u32 {
        if self.is_warming_up() {
            self.warmup_iterations_per_walker
        } else {
            self.iterations_per_walker
        }
    }

    /// Points generated per frame (varies during warmup)
    pub fn points_per_frame(&self) -> u32 {
        self.num_walkers * self.current_iterations()
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

    /// Update for next frame - advances write offset and returns valid point count
    pub fn advance_frame(&mut self, queue: &wgpu::Queue) -> u32 {
        let current_iters = self.current_iterations();
        
        // Upload params with current write offset and iteration count
        let params = PointComputeParams {
            num_transforms: self.num_transforms,
            num_walkers: self.num_walkers,
            iterations_per_walker: current_iters,
            write_offset: self.write_offset,
            buffer_capacity: self.buffer_capacity,
            _pad: [0; 3],
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        // Advance for next frame
        self.write_offset = (self.write_offset + self.points_per_frame()) % self.buffer_capacity;
        self.current_frame += 1;

        self.valid_point_count()
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

    /// Reupload transform weights with some transforms disabled
    pub fn update_weights(
        &self,
        queue: &wgpu::Queue,
        transforms: &[TransformSpec],
        enabled: &[bool],
    ) {
        let total_weight: f32 = transforms
            .iter()
            .zip(enabled.iter())
            .map(|(t, &on)| if on { t.weight } else { 0.0 })
            .sum();

        let mut cumulative = 0.0;
        let gpu_transforms: Vec<GpuTransform> = transforms
            .iter()
            .zip(enabled.iter())
            .map(|(t, &on)| {
                let effective_weight = if on { t.weight } else { 0.0 };
                if total_weight > 0.0 {
                    cumulative += effective_weight / total_weight;
                }
                GpuTransform::new(t, cumulative, effective_weight)
            })
            .collect();

        queue.write_buffer(&self.transform_buffer, 0, bytemuck::cast_slice(&gpu_transforms));
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
