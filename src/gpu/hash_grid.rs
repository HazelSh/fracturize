use wgpu::util::DeviceExt;

use super::buffers::{HashCell, HashGridParams, RenderVoxel, VoxelCounter, HASH_GRID_SIZE, MAX_RENDER_VOXELS};

/// Double-buffered hash grid for view-space voxel accumulation
///
/// Each frame:
/// 1. Reprojection reads grid_a (previous frame), writes to grid_b (current frame)
/// 2. Chaos compute writes new points to grid_b
/// 3. Compaction reads grid_b, writes to render_voxels
/// 4. Rendering draws from render_voxels
/// 5. Swap: grid_a and grid_b are swapped for next frame
pub struct HashGrid {
    /// Grid A buffer (one frame's accumulation)
    pub grid_a: wgpu::Buffer,
    /// Grid B buffer (other frame's accumulation)
    pub grid_b: wgpu::Buffer,
    /// Render voxels output buffer (compaction result)
    pub render_voxels: wgpu::Buffer,
    /// Voxel counter (atomic counter for compaction)
    pub voxel_counter: wgpu::Buffer,
    /// Staging buffer for reading voxel count back to CPU
    pub counter_staging: wgpu::Buffer,
    /// Parameters uniform buffer
    pub params_buffer: wgpu::Buffer,
    /// Current voxel count (from previous frame's readback)
    pub current_voxel_count: u32,
    /// Track which grid is "current" (for swapping)
    pub swapped: bool,
}

impl HashGrid {
    pub fn new(device: &wgpu::Device) -> Self {
        let grid_size = HASH_GRID_SIZE * std::mem::size_of::<HashCell>();
        let render_size = MAX_RENDER_VOXELS * std::mem::size_of::<RenderVoxel>();

        log::info!(
            "Creating hash grids: 2 × {:.1}MB grids, {:.1}MB render buffer",
            grid_size as f64 / 1_000_000.0,
            render_size as f64 / 1_000_000.0
        );

        // Create grid buffers initialized to empty
        let empty_cells: Vec<HashCell> = vec![HashCell::empty(); HASH_GRID_SIZE];
        let grid_a = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hash_grid_a"),
            contents: bytemuck::cast_slice(&empty_cells),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let grid_b = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hash_grid_b"),
            contents: bytemuck::cast_slice(&empty_cells),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Render voxels buffer
        let render_voxels = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("render_voxels"),
            size: render_size as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        // Voxel counter (atomic u32)
        let counter = VoxelCounter { count: 0, _pad: [0; 3] };
        let voxel_counter = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel_counter"),
            contents: bytemuck::bytes_of(&counter),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        });

        // Staging buffer for counter readback
        let counter_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("counter_staging"),
            size: std::mem::size_of::<VoxelCounter>() as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Parameters uniform buffer
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hash_grid_params"),
            size: std::mem::size_of::<HashGridParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            grid_a,
            grid_b,
            render_voxels,
            voxel_counter,
            counter_staging,
            params_buffer,
            current_voxel_count: 0,
            swapped: false,
        }
    }

    /// Get the previous frame's grid (for reading during reprojection)
    pub fn prev_grid(&self) -> &wgpu::Buffer {
        if self.swapped { &self.grid_b } else { &self.grid_a }
    }

    /// Get the current frame's grid (for writing during reprojection and accumulation)
    pub fn curr_grid(&self) -> &wgpu::Buffer {
        if self.swapped { &self.grid_a } else { &self.grid_b }
    }

    /// Swap grids for next frame
    pub fn swap(&mut self) {
        self.swapped = !self.swapped;
    }

    /// Clear the current grid to empty state
    pub fn clear_current(&self, encoder: &mut wgpu::CommandEncoder, device: &wgpu::Device) {
        // Create a buffer of empty cells and copy it
        let empty_cells: Vec<HashCell> = vec![HashCell::empty(); HASH_GRID_SIZE];
        let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("clear_staging"),
            contents: bytemuck::cast_slice(&empty_cells),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        let grid_size = (HASH_GRID_SIZE * std::mem::size_of::<HashCell>()) as u64;
        encoder.copy_buffer_to_buffer(&staging, 0, self.curr_grid(), 0, grid_size);
    }

    /// Reset voxel counter to 0
    pub fn reset_counter(&self, queue: &wgpu::Queue) {
        let counter = VoxelCounter { count: 0, _pad: [0; 3] };
        queue.write_buffer(&self.voxel_counter, 0, bytemuck::bytes_of(&counter));
    }

    /// Upload parameters for this frame
    pub fn upload_params(&self, queue: &wgpu::Queue, params: &HashGridParams) {
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(params));
    }

    /// Copy counter to staging buffer for readback
    pub fn copy_counter_to_staging(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_buffer_to_buffer(
            &self.voxel_counter,
            0,
            &self.counter_staging,
            0,
            std::mem::size_of::<VoxelCounter>() as u64,
        );
    }

    /// Read back the voxel count (call after GPU work completes)
    /// Returns the count, or None if mapping fails
    pub fn read_counter(&mut self, device: &wgpu::Device) -> Option<u32> {
        let slice = self.counter_staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});

        // Poll until mapped
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });

        let data = slice.get_mapped_range();
        let counter: VoxelCounter = *bytemuck::from_bytes(&data);
        drop(data);
        self.counter_staging.unmap();

        self.current_voxel_count = counter.count.min(MAX_RENDER_VOXELS as u32);
        Some(self.current_voxel_count)
    }
}
