use core::sync::atomic::{AtomicBool, Ordering};

use exo_syscall_abi as syscall;

pub const PAGE_SIZE: usize = 4096;
pub const RX_POOL_SIZE: usize = 256;
pub const TX_POOL_SIZE: usize = 256;
pub const VIRTIO_NET_HDR_SIZE_LEGACY: usize = 10;
pub const VIRTIO_NET_HDR_SIZE_MRGBUF: usize = 12;

const DMA_DIR_FROM_DEVICE: u64 = 1;
const DMA_DIR_TO_DEVICE: u64 = 2;
const DMA_PINNED: u64 = 1;

pub struct NetBufPool {
    rx_base_virt: u64,
    rx_base_iova: u64,
    tx_base_virt: u64,
    tx_base_iova: u64,
    hdr_size: usize,
    ready: bool,
    tx_alloc: [AtomicBool; TX_POOL_SIZE],
}

impl NetBufPool {
    pub const fn empty() -> Self {
        Self {
            rx_base_virt: 0,
            rx_base_iova: 0,
            tx_base_virt: 0,
            tx_base_iova: 0,
            hdr_size: VIRTIO_NET_HDR_SIZE_LEGACY,
            ready: false,
            tx_alloc: [const { AtomicBool::new(false) }; TX_POOL_SIZE],
        }
    }

    pub fn init(hdr_size: usize) -> Result<Self, i64> {
        let mut rx_virt = 0u64;
        let rx_iova = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                (RX_POOL_SIZE * PAGE_SIZE) as u64,
                DMA_DIR_FROM_DEVICE,
                &mut rx_virt as *mut u64 as u64,
                DMA_PINNED,
                0,
            )
        };
        if rx_iova < 0 {
            return Err(rx_iova);
        }

        let mut tx_virt = 0u64;
        let tx_iova = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                (TX_POOL_SIZE * PAGE_SIZE) as u64,
                DMA_DIR_TO_DEVICE,
                &mut tx_virt as *mut u64 as u64,
                DMA_PINNED,
                0,
            )
        };
        if tx_iova < 0 {
            unsafe {
                let _ = syscall::syscall3(
                    syscall::SYS_DMA_FREE,
                    rx_iova as u64,
                    (RX_POOL_SIZE * PAGE_SIZE) as u64,
                    0,
                );
            }
            return Err(tx_iova);
        }

        Ok(Self {
            rx_base_virt: rx_virt,
            rx_base_iova: rx_iova as u64,
            tx_base_virt: tx_virt,
            tx_base_iova: tx_iova as u64,
            hdr_size,
            ready: true,
            tx_alloc: [const { AtomicBool::new(false) }; TX_POOL_SIZE],
        })
    }

    pub const fn ready(&self) -> bool {
        self.ready
    }

    pub const fn hdr_size(&self) -> usize {
        self.hdr_size
    }

    pub const fn rx_base_iova(&self) -> u64 {
        self.rx_base_iova
    }

    pub const fn tx_base_iova(&self) -> u64 {
        self.tx_base_iova
    }

    pub fn rx_payload_ptr_mut(&self, idx: usize) -> *mut u8 {
        (self.rx_base_virt + (idx * PAGE_SIZE + self.hdr_size) as u64) as *mut u8
    }

    pub fn tx_payload_ptr_mut(&self, idx: usize) -> *mut u8 {
        (self.tx_base_virt + (idx * PAGE_SIZE + self.hdr_size) as u64) as *mut u8
    }

    pub fn tx_header_ptr_mut(&self, idx: usize) -> *mut u8 {
        (self.tx_base_virt + (idx * PAGE_SIZE) as u64) as *mut u8
    }

    pub fn tx_iova(&self, idx: usize) -> u64 {
        self.tx_base_iova + (idx * PAGE_SIZE) as u64
    }

    pub fn tx_alloc(&self) -> Option<u16> {
        for (idx, used) in self.tx_alloc.iter().enumerate() {
            if used
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Some(idx as u16);
            }
        }
        None
    }

    pub fn tx_free(&self, idx: u16) {
        if (idx as usize) < TX_POOL_SIZE {
            self.tx_alloc[idx as usize].store(false, Ordering::Release);
        }
    }
}
