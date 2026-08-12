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
pub mod lines;
pub mod overlay;
pub mod points;

// Re-export shared types
pub use buffers::CameraUniforms;
pub use context::GpuContext;

// Re-export the active renderer (points)
pub use gizmo::GizmoRenderer;
pub use lines::{LineRenderer, LineVertex};
pub use overlay::OverlayTargets;
pub use points::{Filter, PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};
