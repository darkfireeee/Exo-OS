//! Audio device management

use alloc::string::String;
use alloc::vec::Vec;

use crate::{AudioConfig, AudioError};

/// Audio device type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// Playback device (output)
    Playback,
    /// Capture device (input)
    Capture,
}

/// Audio device info
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Device ID
    pub id: u64,
    /// Device name
    pub name: String,
    /// Device type
    pub device_type: DeviceType,
    /// Supported configurations
    pub configs: Vec<AudioConfig>,
    /// Is this the default device?
    pub is_default: bool,
}

/// Audio device manager
pub struct DeviceManager {
    /// Available devices
    devices: Vec<DeviceInfo>,
    /// Default playback device ID
    default_playback: Option<u64>,
    /// Default capture device ID
    default_capture: Option<u64>,
}

impl DeviceManager {
    /// Create new device manager
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            default_playback: None,
            default_capture: None,
        }
    }

    /// Enumerate available devices
    pub fn enumerate(&mut self) -> Result<&[DeviceInfo], AudioError> {
        // TODO: Enumerate actual hardware devices
        log::debug!("Enumerating audio devices");
        Ok(&self.devices)
    }

    /// Get device by ID
    pub fn get_device(&self, id: u64) -> Option<&DeviceInfo> {
        self.devices.iter().find(|d| d.id == id)
    }

    /// Get default playback device
    pub fn get_default_playback(&self) -> Option<&DeviceInfo> {
        self.default_playback.and_then(|id| self.get_device(id))
    }

    /// Get default capture device
    pub fn get_default_capture(&self) -> Option<&DeviceInfo> {
        self.default_capture.and_then(|id| self.get_device(id))
    }

    /// Set default playback device
    pub fn set_default_playback(&mut self, id: u64) -> Result<(), AudioError> {
        if self.get_device(id).is_some() {
            self.default_playback = Some(id);
            Ok(())
        } else {
            Err(AudioError::DeviceNotFound("Device not found".into()))
        }
    }

    /// Add a device
    pub fn add_device(&mut self, info: DeviceInfo) {
        log::debug!("Adding device: {:?}", info.name);
        if info.is_default {
            match info.device_type {
                DeviceType::Playback => self.default_playback = Some(info.id),
                DeviceType::Capture => self.default_capture = Some(info.id),
            }
        }
        self.devices.push(info);
    }

    /// Remove a device
    pub fn remove_device(&mut self, id: u64) {
        self.devices.retain(|d| d.id != id);
        if self.default_playback == Some(id) {
            self.default_playback = None;
        }
        if self.default_capture == Some(id) {
            self.default_capture = None;
        }
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}
