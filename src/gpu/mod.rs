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
pub mod thumbs;
pub mod timing;

// Re-export shared types
pub use buffers::CameraUniforms;
pub use context::GpuContext;

// Re-export the active renderer (points)
pub use gizmo::GizmoRenderer;
pub use lines::{LineRenderer, LineVertex};
pub use overlay::OverlayTargets;
pub use timing::GpuTimer;
pub use points::{EmphasisParams, EmphasisRenderer, Filter, PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};

/// One pane's rectangle of the surface, in physical pixels (`multiview`).
///
/// Set on every pass that draws a pane, as both the viewport and the scissor:
/// the viewport maps the pane's camera onto its rectangle, and the scissor
/// keeps a fullscreen triangle (a tonemap, a volume housekeeping pass, the
/// selection glow's composite) inside it. Every one of those passes addresses
/// its textures by `clip_position.xy`, which is in framebuffer pixels whatever
/// the viewport says, so a pane reads and writes exactly its own pixels of
/// the window-sized targets and none of the shaders know panes exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Viewport {
    pub fn apply(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_viewport(self.x as f32, self.y as f32, self.w as f32, self.h as f32, 0.0, 1.0);
        pass.set_scissor_rect(self.x, self.y, self.w, self.h);
    }
}
