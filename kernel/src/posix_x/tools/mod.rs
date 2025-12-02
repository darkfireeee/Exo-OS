//! Developer Tools Module
//!
//! Profiling, analysis, and migration tools

pub mod analyzer;
pub mod benchmark;
pub mod migrator;
pub mod profiler;

pub use analyzer::{analyze_binary, check_compatibility};
pub use benchmark::run_benchmarks;
pub use migrator::create_migration_plan;
pub use profiler::{get_hotspots, start_profiling, stop_profiling};
