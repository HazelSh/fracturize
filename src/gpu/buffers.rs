use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

/// GPU point representation (32 bytes, 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuPoint {
    pub pos: [f32; 3],
    pub _pad0: f32,
    pub color: [f32; 4],
}

impl GpuPoint {
    pub fn new(pos: Vec3, color: Vec4) -> Self {
        Self {
            pos: pos.to_array(),
            _pad0: 0.0,
            color: color.to_array(),
        }
    }
}

/// GPU transform representation (96 bytes)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuTransform {
    pub matrix: [[f32; 4]; 4], // 64 bytes
    pub color: [f32; 4],       // 16 bytes
    pub weight: f32,           // 4 bytes
    pub cumulative_weight: f32,// 4 bytes
    pub _pad: [f32; 2],        // 8 bytes
}

impl GpuTransform {
    pub fn new(matrix: Mat4, color: Vec4, weight: f32, cumulative_weight: f32) -> Self {
        Self {
            matrix: matrix.to_cols_array_2d(),
            color: color.to_array(),
            weight,
            cumulative_weight,
            _pad: [0.0; 2],
        }
    }
}

/// Camera uniforms (80 bytes, must be 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CameraUniforms {
    pub mvp: [[f32; 4]; 4], // 64 bytes
    pub screen_height: f32, // 4 bytes
    pub point_size: f32,    // 4 bytes
    pub aspect_ratio: f32,  // 4 bytes (width / height)
    pub min_point_pixels: f32, // 4 bytes - minimum screen size in pixels
}

impl CameraUniforms {
    pub fn new(mvp: Mat4, screen_height: f32, point_size: f32, aspect_ratio: f32, min_point_pixels: f32) -> Self {
        Self {
            mvp: mvp.to_cols_array_2d(),
            screen_height,
            point_size,
            aspect_ratio,
            min_point_pixels,
        }
    }
}

/// Create a storage buffer for points
pub fn create_point_buffer(device: &wgpu::Device, max_points: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("point_buffer"),
        size: (max_points * std::mem::size_of::<GpuPoint>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
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

/// Create a storage buffer for transforms
pub fn create_transform_buffer(device: &wgpu::Device, transforms: &[GpuTransform]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("transform_buffer"),
        contents: bytemuck::cast_slice(transforms),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}
