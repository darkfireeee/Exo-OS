//! exo-storage-common-hal — Implémentation concrète des traits HAL pour l'espace utilisateur Exo-OS
//!
//! Fournit les primitives matérielles nécessaires à l'exécution en Ring 3 des
//! pilotes de stockage (AHCI, NVMe, VirtIO-Blk) conformément aux invariants formels TLA+ :
//! - S19/S20 : Revendication atomique BDF/MMIO (`SYS_PCI_CLAIM`).
//! - S24 : Topologie PCI avant IRQ (`SYS_PCI_SET_TOPOLOGY`).
//! - S39/S40 : Allocation DMA contiguë verrouillée dans les domaines IOMMU (`SYS_DMA_ALLOC`/`SYS_DMA_FREE`).
//! - S31 : Acquittement synchrone FLUSH avant mise à jour de l'arbre VFS.
//! - SRV-02 : #![no_std] strict, arithmétique entière pure.

#![no_std]

pub mod block_ipc;

use exo_syscall_abi as syscall;
pub use block_ipc::{BlockCmdWire, BlockDeviceOps, BlockIpcReply, BlockIpcRequest, BlockReplyWire, run_block_server};

/// Abstraction matérielle pour les pilotes de stockage en espace utilisateur
#[derive(Clone, Copy, Debug)]
pub struct ExoUserspaceStorageHal {
    pub mmio_base: *mut u8,
    pub mmio_size: usize,
    pub bar_phys: u64,
    pub bdf: u32,
}

unsafe impl Send for ExoUserspaceStorageHal {}
unsafe impl Sync for ExoUserspaceStorageHal {}

impl ExoUserspaceStorageHal {
    pub const fn empty() -> Self {
        Self {
            mmio_base: core::ptr::null_mut(),
            mmio_size: 0,
            bar_phys: 0,
            bdf: 0,
        }
    }

    /// Revendique et mappe les ressources PCI nécessaires au contrôleur
    pub fn claim_and_map_pci(bar_phys: u64, bar_size: u64, bdf: u32) -> Result<Self, i64> {
        let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
        if pid <= 0 {
            return Err(syscall::EACCES);
        }

        // 1. Revendication atomique BDF / MMIO (TLA+ S19, S20)
        let claim = unsafe {
            syscall::syscall5(
                syscall::SYS_PCI_CLAIM,
                bar_phys,
                bar_size,
                pid as u64,
                bdf as u64,
                1,
            )
        };
        if claim < 0 {
            return Err(claim);
        }

        // 2. Mapping MMIO en espace virtuel utilisateur
        let mapped = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, bar_phys, bar_size) };
        if mapped < 0 {
            return Err(mapped);
        }

        // 3. Activation du PCI Bus Master
        let bm = unsafe { syscall::syscall1(syscall::SYS_PCI_BUS_MASTER, 1) };
        if bm < 0 {
            return Err(bm);
        }

        // 4. Invariant TLA+ S24 : Définition de la topologie avant tout enregistrement d'IRQ
        let topo = unsafe {
            syscall::syscall3(
                syscall::SYS_PCI_SET_TOPOLOGY,
                bdf as u64,
                0, // Root bridge direct
                1, // has_parent
            )
        };
        if topo < 0 {
            return Err(topo);
        }

        Ok(Self {
            mmio_base: mapped as u64 as *mut u8,
            mmio_size: bar_size as usize,
            bar_phys,
            bdf,
        })
    }

    /// Allocation DMA contiguë via syscall SYS_DMA_ALLOC (TLA+ S39/S40)
    pub fn dma_alloc(&self, pages: usize) -> Option<(u64, *mut u8)> {
        let size = pages.checked_mul(4096)?;
        let mut virt: u64 = 0;
        let iova = unsafe {
            syscall::syscall6(
                syscall::SYS_DMA_ALLOC,
                size as u64,
                0, // Bidirectionnel
                &mut virt as *mut u64 as u64,
                0,
                0,
                0,
            )
        };
        if iova > 0 && virt != 0 {
            Some((iova as u64, virt as *mut u8))
        } else {
            None
        }
    }

    /// Libération DMA contiguë via syscall SYS_DMA_FREE
    pub unsafe fn dma_dealloc(&self, phys: u64, pages: usize) {
        if let Some(size) = pages.checked_mul(4096) {
            let _ = syscall::syscall6(
                syscall::SYS_DMA_FREE,
                phys,
                size as u64,
                0,
                0,
                0,
                0,
            );
        }
    }

    #[inline]
    pub fn mmio_read32(&self, off: usize) -> u32 {
        if off + 4 <= self.mmio_size && !self.mmio_base.is_null() {
            unsafe { core::ptr::read_volatile(self.mmio_base.add(off) as *const u32) }
        } else {
            0
        }
    }

    #[inline]
    pub fn mmio_write32(&self, off: usize, val: u32) {
        if off + 4 <= self.mmio_size && !self.mmio_base.is_null() {
            unsafe { core::ptr::write_volatile(self.mmio_base.add(off) as *mut u32, val) }
        }
    }

    #[inline]
    pub fn mmio_read64(&self, off: usize) -> u64 {
        if off + 8 <= self.mmio_size && !self.mmio_base.is_null() {
            unsafe { core::ptr::read_volatile(self.mmio_base.add(off) as *const u64) }
        } else {
            0
        }
    }

    #[inline]
    pub fn mmio_write64(&self, off: usize, val: u64) {
        if off + 8 <= self.mmio_size && !self.mmio_base.is_null() {
            unsafe { core::ptr::write_volatile(self.mmio_base.add(off) as *mut u64, val) }
        }
    }
}
