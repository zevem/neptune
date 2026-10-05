//! Geometry, preparation and painting consume project-owned terminal snapshots.

mod cache;
pub mod geometry;
mod hints;
mod links;
mod paint;

pub use cache::Cache;
pub use hints::HintInput;
pub use links::{ImagePath, LinkTarget};
pub use paint::{PaintResult, SelectionInteraction};
