//! GPU rendering infrastructure
//!
//! This module contains two renderer implementations:
//! - `density`: Complex hash-grid based renderer with temporal reprojection (experimental)
//! - `points`: Simple point-based renderer with circular buffer updates (current)

pub mod buffers;
pub mod context;
#[allow(dead_code, unused_imports, unused_variables)]
pub mod density;
pub mod gizmo;
pub mod points;
pub mod text;

// Re-export shared types
pub use buffers::CameraUniforms;
pub use context::GpuContext;

// Re-export the active renderer (points)
pub use gizmo::GizmoRenderer;
pub use points::{PointCompute, PointRenderer, DEPTH_FORMAT};
pub use text::TextRenderer;
