#![allow(dead_code)]

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
#[allow(unused_imports)]
use half::f16;

/// Hash grid capacity (4M entries)
pub const HASH_GRID_SIZE: usize = 4 * 1024 * 1024;

/// Maximum render voxels after compaction (2M)
pub const MAX_RENDER_VOXELS: usize = 2 * 1024 * 1024;

/// Empty cell marker (0 so clear_buffer works)
pub const EMPTY_KEY: u32 = 0;

/// Hash cell: 16 bytes
/// Stored in GPU hash table, supports atomic operations
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HashCell {
    /// Encoded key: screen_x:11 | screen_y:11 | depth_slice:10
    /// EMPTY_KEY (0xFFFFFFFF) means empty cell
    pub key: u32,
    /// Accumulated density (fixed-point u16.16 for atomic add precision)
    pub density: u32,
    /// colormap_idx:8 | reserved:8 | weight:16 (for weighted color averaging)
    pub color_weight: u32,
    /// Nearest depth in cell (for occlusion) - uses atomicMin
    pub min_depth: u32,
}

impl HashCell {
    pub fn empty() -> Self {
        Self {
            key: EMPTY_KEY,
            density: 0,
            color_weight: 0,
            min_depth: 0, // cleared state
        }
    }
}

/// Output voxel for rendering: 16 bytes
/// Produced by compaction pass, consumed by voxel renderer
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RenderVoxel {
    /// NDC position (x, y, z) and depth (w) as f16
    pub ndc_xyzw: [f16; 4],
    /// RGB color from colormap lookup
    pub color_rgb: [f16; 3],
    /// Density for alpha/size modulation
    pub density: f16,
}

impl RenderVoxel {
    pub fn zeroed() -> Self {
        Self {
            ndc_xyzw: [f16::ZERO; 4],
            color_rgb: [f16::ZERO; 3],
            density: f16::ZERO,
        }
    }
}

/// Hash grid parameters uniform (must be 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HashGridParams {
    /// Previous frame inverse view-projection matrix
    pub prev_inv_view_proj: [[f32; 4]; 4],
    /// Current frame view-projection matrix
    pub curr_view_proj: [[f32; 4]; 4],
    /// Grid resolution in pixels (matches screen size)
    pub res_x: u32,
    pub res_y: u32,
    /// Depth slices (e.g., 1024)
    pub depth_slices: u32,
    /// Decay factor for reprojection (0.98)
    pub decay: f32,
    /// Hash table size
    pub hash_size: u32,
    /// Near/far planes for depth encoding
    pub near_plane: f32,
    pub far_plane: f32,
    /// Frame number for debugging
    pub frame: u32,
    /// Depth culling enabled (1 = yes, 0 = no)
    pub depth_cull_enabled: u32,
    /// Padding for 16-byte alignment (WGSL requires struct size multiple of largest alignment)
    pub _pad: [u32; 3],
}

impl HashGridParams {
    pub fn new(
        prev_inv_view_proj: Mat4,
        curr_view_proj: Mat4,
        res_x: u32,
        res_y: u32,
        near_plane: f32,
        far_plane: f32,
        decay: f32,
        frame: u32,
        depth_cull_enabled: bool,
    ) -> Self {
        Self {
            prev_inv_view_proj: prev_inv_view_proj.to_cols_array_2d(),
            curr_view_proj: curr_view_proj.to_cols_array_2d(),
            res_x,
            res_y,
            depth_slices: 2048,  // 11 bits (stolen 1 from y)
            decay,
            hash_size: HASH_GRID_SIZE as u32,
            near_plane,
            far_plane,
            frame,
            depth_cull_enabled: if depth_cull_enabled { 1 } else { 0 },
            _pad: [0; 3],
        }
    }
}

/// Voxel count buffer for indirect draw
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoxelCounter {
    /// Number of voxels after compaction
    pub count: u32,
    /// Padding for alignment
    pub _pad: [u32; 3],
}

/// GPU transform representation (160 bytes)
/// Uses color_value (0-1) instead of full RGBA
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuTransform {
    pub matrix: [[f32; 4]; 4], // 64 bytes
    pub color_value: f32,      // 4 bytes - colormap index (0.0-1.0)
    pub weight: f32,           // 4 bytes
    pub cumulative_weight: f32,// 4 bytes
    pub color_speed: f32,      // 4 bytes - per-transform blending speed
    /// Variation blend weights (slot order matches chaos.wgsl / scene::VARIATION_NAMES)
    pub var_weights: [f32; crate::scene::NUM_VARIATIONS], // 80 bytes
}

impl GpuTransform {
    pub fn new(spec: &crate::scene::TransformSpec, cumulative_weight: f32, effective_weight: f32) -> Self {
        Self {
            matrix: spec.matrix.to_cols_array_2d(),
            color_value: spec.color_value,
            weight: effective_weight,
            cumulative_weight,
            color_speed: spec.color_speed,
            var_weights: spec.variations,
        }
    }
}

/// Camera uniforms (112 bytes, must be 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CameraUniforms {
    pub mvp: [[f32; 4]; 4], // 64 bytes
    pub screen_height: f32, // 4 bytes
    pub point_size: f32,    // 4 bytes
    pub aspect_ratio: f32,  // 4 bytes (width / height)
    pub min_point_pixels: f32, // 4 bytes - minimum screen size in pixels
    pub fog_near: f32,      // 4 bytes - distance where fog starts
    pub fog_far: f32,       // 4 bytes - distance where fog is maximum
    pub fog_brightness: f32, // 4 bytes - brightness reduction at max fog (0-1, 1=no change)
    pub fog_saturation: f32, // 4 bytes - saturation reduction at max fog (0-1, 1=no change)
    pub color_contrast: f32, // 4 bytes - cyclic contrast stretch of colormap index (1=off)
    pub _pad: [f32; 3],     // 12 bytes - struct size must be a multiple of 16
}

impl CameraUniforms {
    pub fn new(
        mvp: Mat4,
        screen_height: f32,
        point_size: f32,
        aspect_ratio: f32,
        min_point_pixels: f32,
        fog_near: f32,
        fog_far: f32,
        fog_brightness: f32,
        fog_saturation: f32,
        color_contrast: f32,
    ) -> Self {
        Self {
            mvp: mvp.to_cols_array_2d(),
            screen_height,
            point_size,
            aspect_ratio,
            min_point_pixels,
            fog_near,
            fog_far,
            fog_brightness,
            fog_saturation,
            color_contrast,
            _pad: [0.0; 3],
        }
    }
}

/// Create a uniform buffer for camera data
pub fn create_camera_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("camera_buffer"),
        size: std::mem::size_of::<CameraUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

// =============================================================================
// Simple Point Renderer Types
// =============================================================================

/// A single point in the chaos game buffer (16 bytes)
/// Used by the simple point renderer
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Point {
    /// World-space position
    pub position: [f32; 3],
    /// Colormap index (0-255 in lower 8 bits)
    pub color_idx: u32,
}

/// Compute parameters for the simple chaos game (32 bytes)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PointComputeParams {
    pub num_transforms: u32,
    pub num_walkers: u32,
    pub iterations_per_walker: u32,
    pub write_offset: u32,
    pub buffer_capacity: u32,
    pub _pad: [u32; 3],
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_gpu_transform_size() {
        assert_eq!(std::mem::size_of::<GpuTransform>(), 160, "GpuTransform must be 160 bytes to match WGSL struct");
    }

    #[test]
    fn test_camera_uniforms_size() {
        assert_eq!(std::mem::size_of::<CameraUniforms>(), 112, "CameraUniforms must be 112 bytes to match WGSL struct");
    }

    #[test]
    fn test_hash_grid_params_size() {
        println!("HashGridParams size: {}", std::mem::size_of::<HashGridParams>());
        assert_eq!(std::mem::size_of::<HashGridParams>(), 176, "HashGridParams must be 176 bytes to match WGSL struct");
    }
}
