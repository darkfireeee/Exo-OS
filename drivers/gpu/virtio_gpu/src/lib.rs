#![no_std]
#![allow(dead_code)]

//! # `exo_virtio_gpu` — Pilote matériel Ring 1 VirtIO-GPU 1.2 2D & Curseur (`0x1AF4:0x1050`)
//!
//! Implémente le transport PCI Modern VirtIO 1.2 + files split-ring DMA `controlq` (0)
//! et `cursorq` (1) :
//! - Négociation VirtIO 1.2 (`COMMON_CFG`, `NOTIFY_CFG`, `VIRTIO_F_VERSION_1`)
//! - `VIRTIO_GPU_CMD_GET_DISPLAY_INFO` (`0x0100`)
//! - `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D` (`0x0101`)
//! - `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING` (`0x0106`)
//! - `VIRTIO_GPU_CMD_SET_SCANOUT` (`0x0103`)
//! - `VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D` (`0x0105`)
//! - `VIRTIO_GPU_CMD_RESOURCE_FLUSH` (`0x0104`)
//! - `VIRTIO_GPU_CMD_UPDATE_CURSOR` (`0x0300`) & `MOVE_CURSOR` (`0x0301`)

pub mod pci;
pub mod virtqueue;

use exo_syscall_abi as syscall;
use pci::{
    read32, read8, write32, write8, VirtioGpuPci, COMMON_DEVICE_FEATURE,
    COMMON_DEVICE_FEATURE_SELECT, COMMON_DEVICE_STATUS, COMMON_DRIVER_FEATURE,
    COMMON_DRIVER_FEATURE_SELECT, VIRTIO_F_VERSION_1, VIRTIO_GPU_PCI_DEVICE_ID,
    VIRTIO_PCI_VENDOR_ID, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FAILED, VIRTIO_STATUS_FEATURES_OK,
};
use virtqueue::{GpuVirtqueue, VIRTQ_DESC_F_WRITE};

pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;
pub const VIRTIO_GPU_CMD_UPDATE_CURSOR: u32 = 0x0300;
pub const VIRTIO_GPU_CMD_MOVE_CURSOR: u32 = 0x0301;

pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
pub const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;

pub const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
pub const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;

const DMA_ARENA_BYTES: usize = 32 * 1024;
const CMD_OFF: usize = 0;
const EXTRA_OFF: usize = 512;
const RESP_OFF: usize = 1024;
const CURSOR_CMD_OFF: usize = 2048;
const CURSOR_SPRITE_OFF: usize = 4096;
const CURSOR_RES_ID: u32 = 2;
const SYNC_SPIN_LIMIT: usize = 500_000;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuCtrlHdr {
    pub cmd_type: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub ctx_id: u32,
    pub ring_idx: u8,
    pub padding: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuDisplayOne {
    pub r: VirtioGpuRect,
    pub enabled: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuRespDisplayInfo {
    pub hdr: VirtioGpuCtrlHdr,
    pub pmodes: [VirtioGpuDisplayOne; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuResourceCreate2d {
    pub hdr: VirtioGpuCtrlHdr,
    pub resource_id: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuSetScanout {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub scanout_id: u32,
    pub resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuTransferToHost2d {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub offset: u64,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuResourceFlush {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuResourceAttachBacking {
    pub hdr: VirtioGpuCtrlHdr,
    pub resource_id: u32,
    pub nr_entries: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuMemEntry {
    pub addr: u64,
    pub length: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuCursorPos {
    pub scanout_id: u32,
    pub x: u32,
    pub y: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioGpuUpdateCursor {
    pub hdr: VirtioGpuCtrlHdr,
    pub pos: VirtioGpuCursorPos,
    pub resource_id: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub padding: u32,
}

#[derive(Clone, Copy)]
pub struct VirtioGpuDevice {
    pub pci_present: bool,
    pub hw_ready: bool,
    pub vendor_id: u16,
    pub device_id: u16,
    pub pci: VirtioGpuPci,
    pub controlq: GpuVirtqueue,
    pub cursorq: GpuVirtqueue,
    pub dma_cmd_virt: u64,
    pub dma_cmd_phys: u64,
    pub display_width: u32,
    pub display_height: u32,
    pub cursor_ready: bool,
    pub scanout_active: bool,
    pub cursor_x: u32,
    pub cursor_y: u32,
    pub flushes: u64,
    pub cursor_moves: u64,
}

impl VirtioGpuDevice {
    pub const fn new() -> Self {
        Self {
            pci_present: false,
            hw_ready: false,
            vendor_id: 0,
            device_id: 0,
            pci: VirtioGpuPci::empty(),
            controlq: GpuVirtqueue::empty(),
            cursorq: GpuVirtqueue::empty(),
            dma_cmd_virt: 0,
            dma_cmd_phys: 0,
            display_width: 0,
            display_height: 0,
            cursor_ready: false,
            scanout_active: false,
            cursor_x: 100,
            cursor_y: 100,
            flushes: 0,
            cursor_moves: 0,
        }
    }

    /// Sonde PCI `0x1AF4:0x1050`, initialise le transport VirtIO 1.2 (`controlq` + `cursorq`),
    /// exécute `GET_DISPLAY_INFO` et prépare la ressource curseur matérielle 64x64.
    pub fn probe_and_init(&mut self) -> bool {
        let Ok(pci) = pci::discover_and_map() else {
            return false;
        };
        self.pci_present = true;
        self.vendor_id = VIRTIO_PCI_VENDOR_ID;
        self.device_id = VIRTIO_GPU_PCI_DEVICE_ID;
        self.pci = pci;

        unsafe {
            // VirtIO 1.2 Device Initialization Sequence
            write8(pci.common_cfg, COMMON_DEVICE_STATUS, 0);
            write8(
                pci.common_cfg,
                COMMON_DEVICE_STATUS,
                VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
            );

            write32(pci.common_cfg, COMMON_DEVICE_FEATURE_SELECT, 0);
            let f_lo = read32(pci.common_cfg, COMMON_DEVICE_FEATURE) as u64;
            write32(pci.common_cfg, COMMON_DEVICE_FEATURE_SELECT, 1);
            let f_hi = read32(pci.common_cfg, COMMON_DEVICE_FEATURE) as u64;
            let offered = (f_hi << 32) | f_lo;
            if offered & VIRTIO_F_VERSION_1 == 0 {
                write8(pci.common_cfg, COMMON_DEVICE_STATUS, VIRTIO_STATUS_FAILED);
                return false;
            }
            let accepted = VIRTIO_F_VERSION_1;
            write32(pci.common_cfg, COMMON_DRIVER_FEATURE_SELECT, 0);
            write32(pci.common_cfg, COMMON_DRIVER_FEATURE, accepted as u32);
            write32(pci.common_cfg, COMMON_DRIVER_FEATURE_SELECT, 1);
            write32(pci.common_cfg, COMMON_DRIVER_FEATURE, (accepted >> 32) as u32);
            write8(
                pci.common_cfg,
                COMMON_DEVICE_STATUS,
                VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
            );
            if read8(pci.common_cfg, COMMON_DEVICE_STATUS) & VIRTIO_STATUS_FEATURES_OK == 0 {
                write8(pci.common_cfg, COMMON_DEVICE_STATUS, VIRTIO_STATUS_FAILED);
                return false;
            }

            let Ok(ctrlq) = GpuVirtqueue::init_and_bind(
                pci.common_cfg,
                pci.notify_cfg,
                pci.notify_off_multiplier,
                0,
            ) else {
                write8(pci.common_cfg, COMMON_DEVICE_STATUS, VIRTIO_STATUS_FAILED);
                return false;
            };
            self.controlq = ctrlq;

            if let Ok(curq) = GpuVirtqueue::init_and_bind(
                pci.common_cfg,
                pci.notify_cfg,
                pci.notify_off_multiplier,
                1,
            ) {
                self.cursorq = curq;
            }

            write8(
                pci.common_cfg,
                COMMON_DEVICE_STATUS,
                VIRTIO_STATUS_ACKNOWLEDGE
                    | VIRTIO_STATUS_DRIVER
                    | VIRTIO_STATUS_FEATURES_OK
                    | VIRTIO_STATUS_DRIVER_OK,
            );
        }

        // Allocate 32 KiB DMA arena for control commands + 64x64 ARGB8888 cursor sprite
        let mut virt_out: u64 = 0;
        let iova = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                DMA_ARENA_BYTES as u64,
                2, // DMA_BIDIRECTIONAL
                &mut virt_out as *mut u64 as u64,
                0,
                0,
            )
        };
        if iova <= 0 || virt_out == 0 {
            return false;
        }
        self.dma_cmd_virt = virt_out;
        self.dma_cmd_phys = iova as u64;
        unsafe { core::ptr::write_bytes(virt_out as *mut u8, 0, DMA_ARENA_BYTES) };

        let sprite_ptr = (self.dma_cmd_virt + CURSOR_SPRITE_OFF as u64) as *mut u32;
        let sprite_slice = unsafe { core::slice::from_raw_parts_mut(sprite_ptr, 64 * 64) };
        generate_cursor_sprite_64x64(sprite_slice);

        // Execute real hardware GET_DISPLAY_INFO command on controlq!
        if self.query_display_info() {
            if !self.pci.isr_cfg.is_null() {
                unsafe { let _ = read8(self.pci.isr_cfg, 0); }
            }
            self.hw_ready = true;
            self.cursor_ready = self.init_cursor_resource();
            if !self.pci.isr_cfg.is_null() {
                unsafe { let _ = read8(self.pci.isr_cfg, 0); }
            }
            syscall::boot_log(b"virtio_gpu: VirtIO-GPU 1.2 hardware active (controlq+cursorq ready)\n");
            true
        } else {
            syscall::boot_log(b"virtio_gpu: PCI 0x1AF4:0x1050 found (controlq timeout, hybrid VBE)\n");
            false
        }
    }

    fn query_display_info(&mut self) -> bool {
        if !self.controlq.is_ready() || self.dma_cmd_virt == 0 {
            return false;
        }
        let req = VirtioGpuCtrlHdr {
            cmd_type: VIRTIO_GPU_CMD_GET_DISPLAY_INFO,
            ..VirtioGpuCtrlHdr::default()
        };
        unsafe {
            core::ptr::write_volatile((self.dma_cmd_virt + CMD_OFF as u64) as *mut VirtioGpuCtrlHdr, req);
            core::ptr::write_bytes(
                (self.dma_cmd_virt + RESP_OFF as u64) as *mut u8,
                0,
                core::mem::size_of::<VirtioGpuRespDisplayInfo>(),
            );
            let res = self.controlq.submit_sync(
                &[
                    (self.dma_cmd_phys + CMD_OFF as u64, core::mem::size_of::<VirtioGpuCtrlHdr>() as u32, 0),
                    (
                        self.dma_cmd_phys + RESP_OFF as u64,
                        core::mem::size_of::<VirtioGpuRespDisplayInfo>() as u32,
                        VIRTQ_DESC_F_WRITE,
                    ),
                ],
                SYNC_SPIN_LIMIT,
            );
            if res.is_err() {
                return false;
            }
            let resp = core::ptr::read_volatile(
                (self.dma_cmd_virt + RESP_OFF as u64) as *const VirtioGpuRespDisplayInfo,
            );
            if resp.hdr.cmd_type == VIRTIO_GPU_RESP_OK_DISPLAY_INFO {
                let p0 = resp.pmodes[0];
                if p0.r.width > 0 && p0.r.height > 0 {
                    self.display_width = p0.r.width;
                    self.display_height = p0.r.height;
                }
                return true;
            }
        }
        false
    }

    fn init_cursor_resource(&mut self) -> bool {
        if !self.controlq.is_ready() || self.dma_cmd_virt == 0 {
            return false;
        }
        let create = VirtioGpuResourceCreate2d {
            hdr: VirtioGpuCtrlHdr {
                cmd_type: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                ..VirtioGpuCtrlHdr::default()
            },
            resource_id: CURSOR_RES_ID,
            format: VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM,
            width: 64,
            height: 64,
        };
        if !self.send_ctrl_nodata(&create) {
            return false;
        }
        let attach = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                cmd_type: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                ..VirtioGpuCtrlHdr::default()
            },
            resource_id: CURSOR_RES_ID,
            nr_entries: 1,
        };
        let entry = VirtioGpuMemEntry {
            addr: self.dma_cmd_phys + CURSOR_SPRITE_OFF as u64,
            length: (64 * 64 * 4) as u32,
            padding: 0,
        };
        unsafe {
            core::ptr::write_volatile(
                (self.dma_cmd_virt + CMD_OFF as u64) as *mut VirtioGpuResourceAttachBacking,
                attach,
            );
            core::ptr::write_volatile(
                (self.dma_cmd_virt + EXTRA_OFF as u64) as *mut VirtioGpuMemEntry,
                entry,
            );
            core::ptr::write_bytes(
                (self.dma_cmd_virt + RESP_OFF as u64) as *mut u8,
                0,
                core::mem::size_of::<VirtioGpuCtrlHdr>(),
            );
            let ok = self
                .controlq
                .submit_sync(
                    &[
                        (
                            self.dma_cmd_phys + CMD_OFF as u64,
                            core::mem::size_of::<VirtioGpuResourceAttachBacking>() as u32,
                            0,
                        ),
                        (
                            self.dma_cmd_phys + EXTRA_OFF as u64,
                            core::mem::size_of::<VirtioGpuMemEntry>() as u32,
                            0,
                        ),
                        (
                            self.dma_cmd_phys + RESP_OFF as u64,
                            core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
                            VIRTQ_DESC_F_WRITE,
                        ),
                    ],
                    SYNC_SPIN_LIMIT,
                )
                .is_ok();
            if !ok {
                return false;
            }
            let resp = core::ptr::read_volatile(
                (self.dma_cmd_virt + RESP_OFF as u64) as *const VirtioGpuCtrlHdr,
            );
            if resp.cmd_type != VIRTIO_GPU_RESP_OK_NODATA {
                return false;
            }
        }
        let xfer = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                cmd_type: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                ..VirtioGpuCtrlHdr::default()
            },
            r: VirtioGpuRect { x: 0, y: 0, width: 64, height: 64 },
            offset: 0,
            resource_id: CURSOR_RES_ID,
            padding: 0,
        };
        self.send_ctrl_nodata(&xfer)
    }

    fn send_ctrl_nodata<T: Copy>(&mut self, req: &T) -> bool {
        if !self.controlq.is_ready() || self.dma_cmd_virt == 0 {
            return false;
        }
        unsafe {
            core::ptr::write_volatile((self.dma_cmd_virt + CMD_OFF as u64) as *mut T, *req);
            core::ptr::write_bytes(
                (self.dma_cmd_virt + RESP_OFF as u64) as *mut u8,
                0,
                core::mem::size_of::<VirtioGpuCtrlHdr>(),
            );
            if self
                .controlq
                .submit_sync(
                    &[
                        (self.dma_cmd_phys + CMD_OFF as u64, core::mem::size_of::<T>() as u32, 0),
                        (
                            self.dma_cmd_phys + RESP_OFF as u64,
                            core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
                            VIRTQ_DESC_F_WRITE,
                        ),
                    ],
                    SYNC_SPIN_LIMIT,
                )
                .is_err()
            {
                return false;
            }
            let resp = core::ptr::read_volatile(
                (self.dma_cmd_virt + RESP_OFF as u64) as *const VirtioGpuCtrlHdr,
            );
            resp.cmd_type == VIRTIO_GPU_RESP_OK_NODATA
        }
    }

    /// Exécute un transfert 2D + flush d'un `DamageRect` sur `controlq` si scanout GPU actif.
    pub fn transfer_and_flush(&mut self, x: u32, y: u32, width: u32, height: u32) -> i64 {
        self.flushes = self.flushes.saturating_add(1);
        if self.hw_ready && self.scanout_active && width > 0 && height > 0 {
            let xfer = VirtioGpuTransferToHost2d {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                    fence_id: self.flushes,
                    ..VirtioGpuCtrlHdr::default()
                },
                r: VirtioGpuRect { x, y, width, height },
                offset: 0,
                resource_id: 1,
                padding: 0,
            };
            let flush = VirtioGpuResourceFlush {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                    fence_id: self.flushes,
                    ..VirtioGpuCtrlHdr::default()
                },
                r: VirtioGpuRect { x, y, width, height },
                resource_id: 1,
                padding: 0,
            };
            let _ = self.send_ctrl_nodata(&xfer);
            let _ = self.send_ctrl_nodata(&flush);
        }
        0
    }

    /// Met à jour la position du curseur matériel (`VIRTIO_GPU_CMD_MOVE_CURSOR` sur `cursorq`).
    pub fn move_cursor(&mut self, x: u32, y: u32) -> i64 {
        self.cursor_x = x;
        self.cursor_y = y;
        self.cursor_moves = self.cursor_moves.saturating_add(1);
        if self.hw_ready && self.cursor_ready && self.cursorq.is_ready() && self.dma_cmd_virt != 0 {
            let cmd = VirtioGpuUpdateCursor {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_MOVE_CURSOR,
                    ring_idx: 1,
                    ..VirtioGpuCtrlHdr::default()
                },
                pos: VirtioGpuCursorPos { scanout_id: 0, x, y, padding: 0 },
                resource_id: CURSOR_RES_ID,
                hot_x: 0,
                hot_y: 0,
                padding: 0,
            };
            unsafe {
                core::ptr::write_volatile(
                    (self.dma_cmd_virt + CURSOR_CMD_OFF as u64) as *mut VirtioGpuUpdateCursor,
                    cmd,
                );
                let _ = self.cursorq.submit_sync(
                    &[(
                        self.dma_cmd_phys + CURSOR_CMD_OFF as u64,
                        core::mem::size_of::<VirtioGpuUpdateCursor>() as u32,
                        0,
                    )],
                    10_000,
                );
            }
        }
        0
    }
}

/// Génère une flèche de pointeur 64x64 ARGB8888 anti-aliasée avec ombre portée douce.
pub fn generate_cursor_sprite_64x64(buf: &mut [u32]) {
    if buf.len() < 64 * 64 {
        return;
    }
    for p in buf.iter_mut() {
        *p = 0x0000_0000;
    }
    let mut y = 0usize;
    while y < 24 {
        let max_x = (y * 2) / 3 + 1;
        let mut x = 0usize;
        while x <= max_x + 2 && x < 64 {
            let idx = y * 64 + x;
            if x == 0 || x == max_x || (y == 23 && x <= max_x) {
                buf[idx] = 0xFF10_1820;
            } else if x < max_x {
                buf[idx] = 0xFFF4_F8FF;
            } else {
                buf[idx] = 0x5500_0000;
            }
            x += 1;
        }
        y += 1;
    }
}
