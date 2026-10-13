//! Simple point-based chaos game renderer
//!
//! This module implements a straightforward approach:
//! 1. Run chaos game, write points (pos + color) to a large buffer
//! 2. Render points as billboard quads
//! 3. Use circular buffer updates for temporal stability

pub mod accumulate;
pub mod compute;
pub mod density;
pub mod downsample;
pub mod emphasis;
#[cfg_attr(not(test), allow(dead_code))]
pub mod moments;
pub mod renderer;
pub mod splat;
pub mod volume;

pub use compute::PointCompute;
pub use downsample::Filter;
pub use emphasis::{EmphasisParams, EmphasisRenderer};
pub use renderer::{PointRenderer, DEPTH_FORMAT};
pub use splat::SplatRenderer;
