//! Generic framebuffer driver.

use crate::drivers::{DeviceInfo, Driver, DriverError, DriverResult};
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;

/// Framebuffer pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PixelFormat {
    RGB888,
    BGR888,
    RGBA8888,
    BGRA8888,
}

/// Framebuffer information.
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub address: usize,
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub bpp: usize,
    pub format: PixelFormat,
}

/// Framebuffer driver structure.
pub struct FramebufferDriver {
    info: Option<FramebufferInfo>,
}

impl FramebufferDriver {
    pub const fn new() -> Self {
        Self { info: None }
    }

    /// Initializes the framebuffer with the given information.
    pub fn init_with_info(&mut self, info: FramebufferInfo) {
        self.info = Some(info);
    }

    /// Gets the framebuffer info if available.
    pub fn info(&self) -> Option<&FramebufferInfo> {
        self.info.as_ref()
    }

    /// Writes a pixel at the given coordinates.
    pub fn write_pixel(&mut self, x: usize, y: usize, color: u32) -> Result<(), &'static str> {
        let info = self.info.ok_or("Framebuffer not initialized")?;

        if x >= info.width || y >= info.height {
            return Err("Coordinates out of bounds");
        }

        let pixel_offset = y * info.pitch + x * (info.bpp / 8);
        let framebuffer = unsafe {
            core::slice::from_raw_parts_mut(info.address as *mut u8, info.pitch * info.height)
        };

        match info.format {
            PixelFormat::RGB888 | PixelFormat::BGR888 => {
                let offset = pixel_offset;
                framebuffer[offset] = (color & 0xFF) as u8;
                framebuffer[offset + 1] = ((color >> 8) & 0xFF) as u8;
                framebuffer[offset + 2] = ((color >> 16) & 0xFF) as u8;
            }
            PixelFormat::RGBA8888 | PixelFormat::BGRA8888 => {
                let offset = pixel_offset;
                framebuffer[offset] = (color & 0xFF) as u8;
                framebuffer[offset + 1] = ((color >> 8) & 0xFF) as u8;
                framebuffer[offset + 2] = ((color >> 16) & 0xFF) as u8;
                framebuffer[offset + 3] = ((color >> 24) & 0xFF) as u8;
            }
        }

        Ok(())
    }

    /// Clears the framebuffer with the given color.
    pub fn clear(&mut self, color: u32) -> Result<(), &'static str> {
        let info = self.info.ok_or("Framebuffer not initialized")?;

        for y in 0..info.height {
            for x in 0..info.width {
                self.write_pixel(x, y, color)?;
            }
        }

        Ok(())
    }
}

impl Driver for FramebufferDriver {
    fn name(&self) -> &str {
        "Generic Framebuffer Driver"
    }

    fn init(&mut self) -> DriverResult<()> {
        // Framebuffer initialization would typically get info from bootloader
        // For now, assume it's set via init_with_info
        Ok(())
    }

    fn probe(&self) -> DriverResult<DeviceInfo> {
        Ok(DeviceInfo {
            name: "Framebuffer",
            vendor_id: 0,
            device_id: 0,
        })
    }
}

lazy_static! {
    pub static ref FRAMEBUFFER: Mutex<FramebufferDriver> = Mutex::new(FramebufferDriver::new());
}
