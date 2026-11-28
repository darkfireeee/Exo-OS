//! Modular ExoOS system telemetry: RTC clock/calendar, live Ring 0/1 counters, and ExoFS metadata.

pub mod clock;
pub mod collector;
pub mod exofs;

pub use collector::TelemetryState;
pub use exofs::{inspect_exofs_object, ExoFsObjectInfo};
