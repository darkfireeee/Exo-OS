//! Data visualization primitives for ExoOS Monitor & ExoBench (Dark & Light palette aware).

mod diagrams;
mod gauges;
mod series;

pub use diagrams::{draw_hexagon_radar, draw_module_map};
pub use gauges::draw_radial_gauge;
pub use series::{draw_area_chart, draw_horizontal_bars, BarEntry};
