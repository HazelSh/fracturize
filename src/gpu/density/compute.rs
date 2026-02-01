use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::gpu::buffers::GpuTransform;

/// Number of threads per workgroup (must match shader)
const WORKGROUP_SIZE: u32 = 256;

/// Compute shader parameters (must match WGSL struct)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ComputeParams {
    pub num_transforms: u32,
    pub num_walkers: u32,
    pub iterations_per_walker: u32,
    pub _pad: u32,
}

/// Per-walker state (48 bytes, one per parallel walker)
/// Must match WGSL WalkerState struct
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct WalkerState {
    pub current_pos: [f32; 3],    // 12 bytes
    pub _pad0: f32,               // 4 bytes -> 16 total
    pub current_color: f32,       // 4 bytes - single color value (0.0-1.0)
    pub _pad1: f32,               // 4 bytes
    pub _pad2: f32,               // 4 bytes
    pub _pad3: f32,               // 4 bytes -> 32 total
    pub rng_state: [u32; 4],      // 16 bytes -> 48 total
}

impl WalkerState {
    pub fn new(seed: u64) -> Self {
        // Initialize xorshift128 state from seed
        let mut state = [
            (seed & 0xFFFFFFFF) as u32,
            ((seed >> 32) & 0xFFFFFFFF) as u32,
            seed.wrapping_mul(0x5DEECE66D) as u32,
            (seed.wrapping_mul(0x5DEECE66D) >> 32) as u32,
        ];
        // Ensure no zeros
        for s in &mut state {
            if *s == 0 {
                *s = 0xDEADBEEF;
            }
        }

        Self {
            current_pos: [0.0, 0.0, 0.0],
            _pad0: 0.0,
            current_color: 0.5,  // Start in middle of colormap
            _pad1: 0.0,
            _pad2: 0.0,
            _pad3: 0.0,
            rng_state: state,
        }
    }
}

/// Chaos game compute pipeline - hash grid version
/// Projects points to view space and inserts into hash grid
pub struct ChaosCompute {
    pub pipeline: wgpu::ComputePipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub transform_buffer: wgpu::Buffer,
    pub walker_states_buffer: wgpu::Buffer,
    pub params_buffer: wgpu::Buffer,
    pub num_walkers: u32,
    pub num_workgroups: u32,
    pub iterations_per_walker: u32,
}

impl ChaosCompute {
    pub fn new(
        device: &wgpu::Device,
        transforms: &[(glam::Mat4, f32, f32, f32)], // (matrix, color_value, weight, color_speed)
        target_points_per_frame: usize,
    ) -> Self {
        // Calculate parallelism parameters
        let num_workgroups = 64u32;
        let num_walkers = num_workgroups * WORKGROUP_SIZE;  // 16384 parallel walkers

        let iterations_per_walker = ((target_points_per_frame as u32 + num_walkers - 1) / num_walkers).max(1);

        log::info!(
            "Hash grid compute: {} walkers × {} iters/walker = {} points/frame",
            num_walkers, iterations_per_walker, num_walkers * iterations_per_walker
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chaos_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/density/chaos.wgsl").into()),
        });

        // Prepare transform data with cumulative weights
        let total_weight: f32 = transforms.iter().map(|(_, _, w, _)| w).sum();
        let mut cumulative = 0.0;
        let gpu_transforms: Vec<GpuTransform> = transforms
            .iter()
            .map(|(matrix, color_value, weight, speed)| {
                cumulative += weight / total_weight;
                GpuTransform::new(*matrix, *color_value, *weight, cumulative, *speed)
            })
            .collect();

        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("transform_buffer"),
            contents: bytemuck::cast_slice(&gpu_transforms),
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Initialize all walker states with unique seeds
        let walker_states: Vec<WalkerState> = (0..num_walkers)
            .map(|i| WalkerState::new(42_u64.wrapping_add((i as u64).wrapping_mul(0x9E3779B97F4A7C15))))
            .collect();
        let walker_states_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("walker_states_buffer"),
            contents: bytemuck::cast_slice(&walker_states),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let params = ComputeParams {
            num_transforms: transforms.len() as u32,
            num_walkers,
            iterations_per_walker,
            _pad: 0,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute_params_buffer"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Bind group layout:
        // 0: grid (read-write)
        // 1: transforms (read)
        // 2: walker_states (read-write)
        // 3: compute_params (uniform)
        // 4: grid_params (uniform)
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chaos_bind_group_layout"),
            entries: &[
                // Hash grid (read-write)
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
                // Transforms (read-only)
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
                // Walker states (read-write)
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
                // Compute params (uniform)
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
                // Grid params (uniform)
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Depth buffer (read-write)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chaos_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("chaos_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            transform_buffer,
            walker_states_buffer,
            params_buffer,
            num_walkers,
            num_workgroups,
            iterations_per_walker,
        }
    }

    /// Create a bind group for this frame
    pub fn create_bind_group(
        &self,
        device: &wgpu::Device,
        grid_buffer: &wgpu::Buffer,
        grid_params_buffer: &wgpu::Buffer,
        depth_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chaos_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: grid_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.transform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.walker_states_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: grid_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: depth_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// Reset the walker states
    pub fn reset(&self, queue: &wgpu::Queue) {
        let walker_states: Vec<WalkerState> = (0..self.num_walkers)
            .map(|i| WalkerState::new(42_u64.wrapping_add((i as u64).wrapping_mul(0x9E3779B97F4A7C15))))
            .collect();
        queue.write_buffer(&self.walker_states_buffer, 0, bytemuck::cast_slice(&walker_states));
    }

    /// Update the number of iterations per frame
    pub fn set_iterations(&mut self, queue: &wgpu::Queue, target_points_per_frame: usize) {
        self.iterations_per_walker = ((target_points_per_frame as u32 + self.num_walkers - 1) / self.num_walkers).max(1);
        
        let actual_points = self.num_walkers * self.iterations_per_walker;
        log::info!(
            "Updated iterations: {}/walker = {} points/frame",
            self.iterations_per_walker, actual_points
        );

        let params = ComputeParams {
            num_transforms: (self.params_buffer.size() / std::mem::size_of::<ComputeParams>() as u64) as u32, // Hacky: we don't store num_transforms, but it doesn't change
            num_walkers: self.num_walkers,
            iterations_per_walker: self.iterations_per_walker,
            _pad: 0,
        };
        // We need to preserve num_transforms. Since we don't store it in the struct,
        // we should probably store it or read it. 
        // Better: let's just update the offset for iterations_per_walker.
        // ComputeParams layout: num_transforms(0), num_walkers(4), iterations_per_walker(8)
        queue.write_buffer(&self.params_buffer, 8, bytemuck::bytes_of(&self.iterations_per_walker));
    }

    /// Run compute pass
    pub fn dispatch(&self, encoder: &mut wgpu::CommandEncoder, bind_group: &wgpu::BindGroup) {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("chaos_pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&self.pipeline);
        compute_pass.set_bind_group(0, bind_group, &[]);
        compute_pass.dispatch_workgroups(self.num_workgroups, 1, 1);
    }
}
