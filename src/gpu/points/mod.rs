//! Simple point-based chaos game renderer
//!
//! This module implements a straightforward approach:
//! 1. Run chaos game, write points (pos + color) to a large buffer
//! 2. Render points as billboard quads
//! 3. Use circular buffer updates for temporal stability

pub mod compute;
pub mod renderer;
pub mod splat;

pub use compute::PointCompute;
pub use renderer::{PointRenderer, DEPTH_FORMAT};
pub use splat::SplatRenderer;
