//! Null driver (discards all input).

use crate::drivers::{DeviceInfo, Driver, DriverResult};
use core::fmt;

/// Null driver structure.
pub struct NullDriver;

impl NullDriver {
    pub const fn new() -> Self {
        Self
    }
}

impl Driver for NullDriver {
    fn name(&self) -> &str {
        "Null Device Driver"
    }

    fn init(&mut self) -> DriverResult<()> {
        Ok(())
    }

    fn probe(&self) -> DriverResult<DeviceInfo> {
        Ok(DeviceInfo {
            name: "null",
            vendor_id: 0,
            device_id: 0,
        })
    }
}

impl fmt::Write for NullDriver {
    fn write_str(&mut self, _s: &str) -> fmt::Result {
        Ok(())
    }
}
