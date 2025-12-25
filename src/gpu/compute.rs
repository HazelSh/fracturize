use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

use super::buffers::{create_point_buffer, GpuPoint, GpuTransform};

/// Compute shader parameters
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ComputeParams {
    pub num_transforms: u32,
    pub max_points: u32,
    pub iterations_per_dispatch: u32,
    pub _pad: u32,
}

/// Iteration state (persistent across frames)
/// Layout must match WGSL struct exactly (80 bytes total):
/// - vec3<f32> + f32 padding = 16 bytes
/// - vec4<f32> = 16 bytes
/// - u32 + u32 + 8 bytes padding = 16 bytes (to align vec4<u32>)
/// - vec4<u32> = 16 bytes
/// - 16 bytes padding to align struct size
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct IterationState {
    pub current_pos: [f32; 3],    // 12 bytes
    pub _pad0: f32,               // 4 bytes -> 16 total
    pub current_color: [f32; 4],  // 16 bytes -> 32 total
    pub point_write_idx: u32,     // 4 bytes -> 36 total
    pub total_iterations: u32,    // 4 bytes -> 40 total
    pub _align_pad: [u32; 2],     // 8 bytes padding -> 48 total (for rng_state alignment)
    pub rng_state: [u32; 4],      // 16 bytes -> 64 total
    pub _struct_pad: [u32; 4],    // 16 bytes -> 80 total (struct alignment)
}

impl IterationState {
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
            current_color: [0.5, 0.5, 0.5, 1.0],
            point_write_idx: 0,
            total_iterations: 0,
            _align_pad: [0; 2],
            rng_state: state,
            _struct_pad: [0; 4],
        }
    }
}

/// Chaos game compute pipeline
pub struct ChaosCompute {
    pub pipeline: wgpu::ComputePipeline,
    pub bind_group: wgpu::BindGroup,
    pub point_buffer: wgpu::Buffer,
    pub transform_buffer: wgpu::Buffer,
    pub state_buffer: wgpu::Buffer,
    pub params_buffer: wgpu::Buffer,
    pub max_points: u32,
    pub iterations_per_dispatch: u32,
}

impl ChaosCompute {
    pub fn new(
        device: &wgpu::Device,
        transforms: &[(Mat4, Vec4, f32)], // (matrix, color, weight)
        max_points: usize,
        iterations_per_dispatch: usize,
    ) -> Self {
        // Load shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chaos_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/chaos.wgsl").into()),
        });

        // Prepare transform data with cumulative weights
        let total_weight: f32 = transforms.iter().map(|(_, _, w)| w).sum();
        let mut cumulative = 0.0;
        let gpu_transforms: Vec<GpuTransform> = transforms
            .iter()
            .map(|(matrix, color, weight)| {
                cumulative += weight / total_weight;
                GpuTransform {
                    matrix: matrix.to_cols_array_2d(),
                    color: color.to_array(),
                    weight: *weight,
                    cumulative_weight: cumulative,
                    _pad: [0.0; 2],
                }
            })
            .collect();

        // Create buffers
        let point_buffer = create_point_buffer(device, max_points);

        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("transform_buffer"),
            contents: bytemuck::cast_slice(&gpu_transforms),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let state = IterationState::new(42);
        let state_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("state_buffer"),
            contents: bytemuck::bytes_of(&state),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let params = ComputeParams {
            num_transforms: transforms.len() as u32,
            max_points: max_points as u32,
            iterations_per_dispatch: iterations_per_dispatch as u32,
            _pad: 0,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("params_buffer"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chaos_bind_group_layout"),
            entries: &[
                // Points buffer (read-write)
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
                // Transforms buffer (read-only)
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
                // State buffer (read-write)
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
                // Params buffer (uniform)
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

        // Create bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chaos_bind_group"),
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
                    resource: state_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        // Create pipeline
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chaos_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
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
            bind_group,
            point_buffer,
            transform_buffer,
            state_buffer,
            params_buffer,
            max_points: max_points as u32,
            iterations_per_dispatch: iterations_per_dispatch as u32,
        }
    }

    /// Reset the iteration state
    pub fn reset(&self, queue: &wgpu::Queue) {
        let state = IterationState::new(42);
        queue.write_buffer(&self.state_buffer, 0, bytemuck::bytes_of(&state));
    }

    /// Run compute pass
    pub fn dispatch(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("chaos_pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&self.pipeline);
        compute_pass.set_bind_group(0, &self.bind_group, &[]);
        compute_pass.dispatch_workgroups(1, 1, 1);
    }

    /// Get the number of points currently in the buffer
    /// Returns max_points once the buffer is filled
    pub fn point_count(&self, total_iterations: u32) -> u32 {
        total_iterations.min(self.max_points)
    }
}
