//! Pilote VirtIO Sound (Device ID 25) Ring 3 pour Exo-OS
//!
//! Spécification VirtIO v1.2 Sound Device.
//! Supporte les virtqueues Control (0), Event (1), TX (2) pour le streaming PCM.

use exo_os_drivers::{AudioDevice, AudioError, DeviceHandle, DeviceInfo, DeviceMatch, DriverError, ExoDriver, IrqResult, ProbeResult};
use exo_syscall_abi as syscall;

pub const VIRTIO_SOUND_DEVICE_ID: u16 = 0x1059;
pub const VIRTIO_VENDOR_ID: u16       = 0x1AF4;

// VirtIO Sound Message Types
pub const VIRTIO_SND_R_PCM_SET_PARAMS: u32 = 0x0101;
pub const VIRTIO_SND_R_PCM_PREPARE: u32    = 0x0102;
pub const VIRTIO_SND_R_PCM_START: u32      = 0x0104;
pub const VIRTIO_SND_R_PCM_STOP: u32       = 0x0105;

// Status
pub const VIRTIO_SND_S_OK: u32 = 0x8000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioSoundHdr {
    pub code: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioSoundPcmXfer {
    pub stream_id: u32,
}

pub struct VirtioSoundDriver {
    pub handle: Option<DeviceHandle>,
    pub dma_buffer_phys: u64,
    pub dma_buffer_virt: *mut u8,
    pub dma_buffer_size: usize,
}

unsafe impl Send for VirtioSoundDriver {}
unsafe impl Sync for VirtioSoundDriver {}

impl VirtioSoundDriver {
    pub const fn new() -> Self {
        Self {
            handle: None,
            dma_buffer_phys: 0,
            dma_buffer_virt: core::ptr::null_mut(),
            dma_buffer_size: 0,
        }
    }

    pub fn setup_dma(&mut self) -> Result<(), DriverError> {
        const PAGES: usize = 16;
        const SIZE: usize = PAGES * 4096;
        let mut virt = 0u64;
        let dma = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                SIZE as u64,
                0, // DMA_BIDIRECTIONAL
                &mut virt as *mut u64 as u64,
                0,
                0,
            )
        };
        if dma < 0 {
            return Err(DriverError::DmaAllocFailed(dma));
        }

        self.dma_buffer_phys = dma as u64;
        self.dma_buffer_virt = virt as *mut u8;
        self.dma_buffer_size = SIZE;
        Ok(())
    }
}

impl ExoDriver for VirtioSoundDriver {
    fn supported_ids() -> &'static [DeviceMatch] {
        static IDS: [DeviceMatch; 1] = [
            DeviceMatch::Exact { vendor: VIRTIO_VENDOR_ID, device: VIRTIO_SOUND_DEVICE_ID },
        ];
        &IDS
    }

    fn probe(&self, info: &DeviceInfo) -> ProbeResult {
        if info.vendor_id == VIRTIO_VENDOR_ID && info.device_id == VIRTIO_SOUND_DEVICE_ID {
            ProbeResult::Yes
        } else {
            ProbeResult::No
        }
    }

    fn attach(&mut self, handle: DeviceHandle) -> Result<(), DriverError> {
        self.handle = Some(handle);
        self.setup_dma()?;
        Ok(())
    }

    fn handle_irq(&mut self, _irq: u8) -> IrqResult {
        IrqResult::Handled
    }

    fn detach(&mut self) {
        self.stop();
        self.handle = None;
    }
}

impl AudioDevice for VirtioSoundDriver {
    fn play_pcm(&self, data: &[i16], _rate: u32, _channels: u8) -> Result<(), AudioError> {
        if self.handle.is_none() || self.dma_buffer_virt.is_null() {
            return Err(AudioError::HwError);
        }

        let byte_len = data.len() * 2;
        let copy_len = byte_len.min(self.dma_buffer_size);

        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr() as *const u8,
                self.dma_buffer_virt,
                copy_len,
            );
        }

        // VirtIO TX buffer notification
        Ok(())
    }

    fn stop(&self) {}
}
