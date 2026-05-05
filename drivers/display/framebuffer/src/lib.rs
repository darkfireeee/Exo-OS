#![no_std]

#[cfg(test)]
extern crate std;

pub mod blit;
pub mod cursor;
pub mod fb;

pub use fb::{Color, Framebuffer, FramebufferInfo, PixelFormat};
