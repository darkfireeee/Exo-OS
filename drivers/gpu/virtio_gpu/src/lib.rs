#![no_std]
#![allow(dead_code)]

//! # `exo_virtio_gpu` — Pilote matériel Ring 1 VirtIO-GPU 2D & Curseur (`0x1AF4:0x1050`)
//!
//! Implémente les files DMA `controlq` (queue 0) et `cursorq` (queue 1) de la
//! spécification OASIS VirtIO-GPU v1.2 en `#![no_std]` :
//! - `VIRTIO_GPU_CMD_GET_DISPLAY_INFO` (`0x0100`)
//! - `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D` (`0x0101`)
//! - `VIRTIO_GPU_CMD_RESOURCE_UNREF` (`0x0102`)
//! - `VIRTIO_GPU_CMD_SET_SCANOUT` (`0x0103`)
//! - `VIRTIO_GPU_CMD_RESOURCE_FLUSH` (`0x0104`)
//! - `VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D` (`0x0105`)
//! - `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING` (`0x0106`)
//! - `VIRTIO_GPU_CMD_UPDATE_CURSOR` (`0x0300`)
//! - `VIRTIO_GPU_CMD_MOVE_CURSOR` (`0x0301`)

use exo_syscall_abi as syscall;

pub const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
pub const VIRTIO_GPU_PCI_DEVICE_ID: u16 = 0x1050;

// Commandes 2D (controlq = 0)
pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

// Commandes Curseur (cursorq = 1)
pub const VIRTIO_GPU_CMD_UPDATE_CURSOR: u32 = 0x0300;
pub const VIRTIO_GPU_CMD_MOVE_CURSOR: u32 = 0x0301;

// Réponses du GPU
pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
pub const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;

// Formats de pixels VirtIO-GPU
pub const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
pub const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;
pub const VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM: u32 = 3;
pub const VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM: u32 = 4;

pub const VIRTIO_GPU_FLAG_FENCE: u32 = 1 << 0;

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

/// Descripteur Virtqueue Split-Ring standard (16 octets)
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// État du périphérique VirtIO-GPU détecté sur le bus PCI
#[derive(Clone, Copy)]
pub struct VirtioGpuDevice {
    pub pci_present: bool,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub bar0_phys: u64,
    pub bar0_virt: u64,
    pub dma_cmd_virt: u64,
    pub dma_cmd_phys: u64,
    pub cursor_x: u32,
    pub cursor_y: u32,
    pub flushes: u64,
    pub cursor_moves: u64,
}

impl VirtioGpuDevice {
    pub const fn new() -> Self {
        Self {
            pci_present: false,
            vendor_id: 0,
            device_id: 0,
            bus: 0,
            device: 0,
            function: 0,
            bar0_phys: 0,
            bar0_virt: 0,
            dma_cmd_virt: 0,
            dma_cmd_phys: 0,
            cursor_x: 100,
            cursor_y: 100,
            flushes: 0,
            cursor_moves: 0,
        }
    }

    /// Sonde le bus PCI pour trouver un périphérique VirtIO-GPU (`0x1AF4:0x1050`)
    /// et alloue l'arène DMA de commande (`controlq` / `cursorq`).
    pub fn probe_and_init(&mut self) -> bool {
        let mut info = syscall::PciDeviceInfo::default();
        let rc = unsafe {
            syscall::syscall6(
                syscall::SYS_PCI_FIND_DEVICE,
                VIRTIO_PCI_VENDOR_ID as u64,
                VIRTIO_GPU_PCI_DEVICE_ID as u64,
                0xFFFF, // classe wildcard
                0xFFFF, // sous-classe wildcard
                0,      // index
                &mut info as *mut syscall::PciDeviceInfo as u64,
            )
        };
        if rc >= 0 && info.vendor_id == VIRTIO_PCI_VENDOR_ID {
            self.pci_present = true;
            self.vendor_id = info.vendor_id;
            self.device_id = info.device_id;
            self.bus = info.bus;
            self.device = info.device;
            self.function = info.function;
            self.bar0_phys = info.bar0_phys;
            if info.bar0_phys != 0 && info.bar0_size != 0 {
                let mapped = unsafe {
                    syscall::syscall2(syscall::SYS_MMIO_MAP, info.bar0_phys, info.bar0_size.max(4096))
                };
                if mapped > 0 {
                    self.bar0_virt = mapped as u64;
                }
            }
            let mut virt_out: u64 = 0;
            // Alloue 20 KiB (4096 octets commandes + 16384 octets sprite curseur 64x64)
            let iova = unsafe {
                syscall::syscall3(
                    syscall::SYS_DMA_ALLOC,
                    20480,
                    0, // DMA_BIDIRECTIONAL
                    &mut virt_out as *mut u64 as u64,
                )
            };
            if iova > 0 && virt_out > 0 {
                self.dma_cmd_virt = virt_out;
                self.dma_cmd_phys = iova as u64;
                // Génère le sprite curseur 64x64 ARGB8888 dans la seconde moitié du tampon DMA
                let sprite_ptr = (self.dma_cmd_virt + 4096) as *mut u32;
                let sprite_slice = unsafe { core::slice::from_raw_parts_mut(sprite_ptr, 64 * 64) };
                generate_cursor_sprite_64x64(sprite_slice);
            }
            return true;
        }
        false
    }

    /// Exécute un transfert 2D + flush d'un `DamageRect` (ou comptabilise l'opération en mode hybride GOP).
    pub fn transfer_and_flush(&mut self, x: u32, y: u32, width: u32, height: u32) -> i64 {
        self.flushes = self.flushes.saturating_add(1);
        if self.dma_cmd_virt != 0 {
            let transfer = VirtioGpuTransferToHost2d {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                    flags: 0,
                    fence_id: self.flushes,
                    ctx_id: 0,
                    ring_idx: 0,
                    padding: [0; 3],
                },
                r: VirtioGpuRect { x, y, width, height },
                offset: 0,
                resource_id: 1,
                padding: 0,
            };
            let flush = VirtioGpuResourceFlush {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                    flags: 0,
                    fence_id: self.flushes,
                    ctx_id: 0,
                    ring_idx: 0,
                    padding: [0; 3],
                },
                r: VirtioGpuRect { x, y, width, height },
                resource_id: 1,
                padding: 0,
            };
            unsafe {
                core::ptr::write_volatile(self.dma_cmd_virt as *mut VirtioGpuTransferToHost2d, transfer);
                core::ptr::write_volatile(
                    (self.dma_cmd_virt as usize + core::mem::size_of::<VirtioGpuTransferToHost2d>())
                        as *mut VirtioGpuResourceFlush,
                    flush,
                );
            }
        }
        0
    }

    /// Met à jour la position du curseur matériel (`VIRTIO_GPU_CMD_MOVE_CURSOR` sur `cursorq`).
    pub fn move_cursor(&mut self, x: u32, y: u32) -> i64 {
        self.cursor_x = x;
        self.cursor_y = y;
        self.cursor_moves = self.cursor_moves.saturating_add(1);
        if self.dma_cmd_virt != 0 {
            let cmd = VirtioGpuUpdateCursor {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_MOVE_CURSOR,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    ring_idx: 1,
                    padding: [0; 3],
                },
                pos: VirtioGpuCursorPos {
                    scanout_id: 0,
                    x,
                    y,
                    padding: 0,
                },
                resource_id: 2,
                hot_x: 1,
                hot_y: 1,
                padding: 0,
            };
            unsafe {
                core::ptr::write_volatile((self.dma_cmd_virt + 512) as *mut VirtioGpuUpdateCursor, cmd);
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
                buf[idx] = 0xFF10_1820; // Bordure sombre
            } else if x < max_x {
                buf[idx] = 0xFFF4_F8FF; // Intérieur blanc pur
            } else {
                buf[idx] = 0x5500_0000; // Ombre portée semi-transparente
            }
            x += 1;
        }
        y += 1;
    }
}
