//! Density-based hash grid renderer (complex, experimental)
//!
//! This module contains the view-space hash grid accumulation renderer with
//! temporal reprojection. It's preserved but not currently in use.

pub mod compact;
pub mod compute;
pub mod hash_grid;
pub mod reproject;
pub mod voxel_renderer;

pub use compact::CompactPipeline;
pub use compute::ChaosCompute;
pub use hash_grid::HashGrid;
pub use reproject::ReprojectPipeline;
pub use voxel_renderer::{VoxelRenderer, DEPTH_FORMAT};
