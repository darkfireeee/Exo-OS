//! Drivers matériels

pub mod block;
pub mod char;
pub mod input;
pub mod video;

/// Error type for driver operations.
#[derive(Debug)]
pub enum DriverError {
    InitFailed,
    DeviceNotFound,
    IoError,
    NotSupported,
}

/// Result type for driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Basic information about a device.
pub struct DeviceInfo {
    pub name: &'static str,
    pub vendor_id: u16,
    pub device_id: u16,
}

/// Trait that all drivers must implement.
pub trait Driver {
    /// Returns the name of the driver.
    fn name(&self) -> &str;

    /// Initializes the driver.
    fn init(&mut self) -> DriverResult<()>;

    /// Probes for the device.
    fn probe(&self) -> DriverResult<DeviceInfo>;
}
