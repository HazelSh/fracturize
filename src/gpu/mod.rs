//! GPU rendering infrastructure
//!
//! This module contains two renderer implementations:
//! - `density`: Complex hash-grid based renderer with temporal reprojection (experimental)
//! - `points`: Simple point-based renderer with circular buffer updates (current)

pub mod buffers;
pub mod context;
pub mod density;
pub mod points;

// Re-export shared types
pub use buffers::{CameraUniforms, GpuTransform, Point, PointComputeParams};
pub use context::GpuContext;

// Re-export the active renderer (points)
pub use points::{PointCompute, PointRenderer, DEPTH_FORMAT};
