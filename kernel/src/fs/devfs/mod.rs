//! DevFS - Device Filesystem
//! 
//! Expose devices as files (/dev/null, /dev/zero, etc.)

use alloc::collections::BTreeMap;
use alloc::string::String;
use crate::memory::MemoryResult;

/// Device types
#[derive(Debug, Clone, Copy)]
pub enum DeviceType {
    Null,
    Zero,
    Random,
    Console,
    Tty,
}

/// DevFS instance
pub struct DevFs {
    devices: BTreeMap<String, DeviceType>,
}

impl DevFs {
    pub fn new() -> Self {
        let mut devfs = Self {
            devices: BTreeMap::new(),
        };
        
        // Register standard devices
        devfs.devices.insert(String::from("null"), DeviceType::Null);
        devfs.devices.insert(String::from("zero"), DeviceType::Zero);
        devfs.devices.insert(String::from("random"), DeviceType::Random);
        devfs.devices.insert(String::from("console"), DeviceType::Console);
        devfs.devices.insert(String::from("tty"), DeviceType::Tty);
        
        devfs
    }
    
    pub fn read(&self, device: &str, buf: &mut [u8]) -> MemoryResult<usize> {
        match self.devices.get(device) {
            Some(DeviceType::Null) => Ok(0),
            Some(DeviceType::Zero) => {
                buf.fill(0);
                Ok(buf.len())
            }
            Some(DeviceType::Random) => {
                // TODO: Fill with random data
                Ok(buf.len())
            }
            _ => Err(crate::memory::MemoryError::NotFound),
        }
    }
    
    pub fn write(&self, device: &str, buf: &[u8]) -> MemoryResult<usize> {
        match self.devices.get(device) {
            Some(DeviceType::Null) => Ok(buf.len()),
            Some(DeviceType::Console) | Some(DeviceType::Tty) => {
                // TODO: Write to console
                Ok(buf.len())
            }
            _ => Err(crate::memory::MemoryError::InvalidParameter),
        }
    }
}
