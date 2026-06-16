//! Test d'intégration : **HBA AHCI simulé** en mémoire. Valide le chemin complet
//! du driver (enable AHCI → détection port SATA → rebase → IDENTIFY → read/write/
//! flush) en exécutant réellement les Command Headers / FIS / PRDT que le driver
//! produit. `phys == virt` (un seul espace d'adressage en test).

extern crate std;

use super::*;
use crate::structures::{ata, CmdHeader, FisRegH2D, PrdtEntry};
use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::vec::Vec;
use core::cell::RefCell;

const MOCK_SECTOR: usize = 512;
const MOCK_BLOCKS: u64 = 64;
const MOCK_DISK_BYTES: usize = (MOCK_BLOCKS as usize) * EXOFS_BLOCK_SIZE;

struct MockInner {
    ghc: u32,
    clb: u64,
    fb: u64,
    cmd: u32,
    disk: Vec<u8>,
    allocs: Vec<(*mut u8, Layout)>,
}

pub struct MockAhci {
    inner: RefCell<MockInner>,
}

impl MockAhci {
    fn new() -> Self {
        Self {
            inner: RefCell::new(MockInner {
                ghc: 0,
                clb: 0,
                fb: 0,
                cmd: 0,
                disk: alloc::vec![0u8; MOCK_DISK_BYTES],
                allocs: Vec::new(),
            }),
        }
    }
}

impl Drop for MockAhci {
    fn drop(&mut self) {
        for &(ptr, layout) in self.inner.borrow().allocs.iter() {
            // SAFETY: (ptr, layout) issu de dma_alloc.
            unsafe { dealloc(ptr, layout) };
        }
    }
}

unsafe fn read_struct<T: Copy>(phys: u64, byte_off: usize) -> T {
    core::ptr::read_volatile((phys as *const u8).add(byte_off) as *const T)
}

impl MockInner {
    /// Exécute la commande présente au slot 0 de la command list.
    fn process_slot(&mut self, slot: u32) {
        let header: CmdHeader =
            // SAFETY: clb pointe sur une page DMA ; slot < 32.
            unsafe { read_struct(self.clb, (slot as usize) * 32) };
        let ctba = (header.ctba as u64) | ((header.ctbau as u64) << 32);
        // SAFETY: ctba = command table allouée par le driver.
        let fis: FisRegH2D = unsafe { read_struct(ctba, CTBL_FIS_OFFSET) };
        let prdt: PrdtEntry = unsafe { read_struct(ctba, CTBL_PRDT_OFFSET) };
        let dba = (prdt.dba as u64) | ((prdt.dbau as u64) << 32);

        let lba = (fis.lba0 as u64)
            | ((fis.lba1 as u64) << 8)
            | ((fis.lba2 as u64) << 16)
            | ((fis.lba3 as u64) << 24)
            | ((fis.lba4 as u64) << 32)
            | ((fis.lba5 as u64) << 40);
        let count = (fis.countl as u64) | ((fis.counth as u64) << 8);
        let byte_off = (lba as usize) * MOCK_SECTOR;
        let byte_len = (count as usize) * MOCK_SECTOR;

        match fis.command {
            ata::IDENTIFY_DEVICE => {
                // SAFETY: dba = buffer DMA ≥ 512 octets.
                unsafe {
                    let buf = dba as *mut u8;
                    core::ptr::write_bytes(buf, 0, 512);
                    // words 100-103 : total secteurs LBA48.
                    let total = (MOCK_DISK_BYTES / MOCK_SECTOR) as u64;
                    for i in 0..4 {
                        let w = ((total >> (16 * i)) & 0xFFFF) as u16;
                        core::ptr::write_volatile(buf.add((100 + i) * 2), w as u8);
                        core::ptr::write_volatile(buf.add((100 + i) * 2 + 1), (w >> 8) as u8);
                    }
                    // word 106 = 0 → secteurs de 512 octets.
                }
            }
            ata::READ_DMA_EXT => {
                if byte_off + byte_len <= self.disk.len() {
                    // SAFETY: dba buffer ≥ byte_len.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.disk.as_ptr().add(byte_off),
                            dba as *mut u8,
                            byte_len,
                        );
                    }
                }
            }
            ata::WRITE_DMA_EXT => {
                if byte_off + byte_len <= self.disk.len() {
                    // SAFETY: dba buffer ≥ byte_len.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            dba as *const u8,
                            self.disk.as_mut_ptr().add(byte_off),
                            byte_len,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

impl AhciHal for MockAhci {
    fn dma_alloc(&self, pages: usize) -> Option<DmaRegion> {
        let layout = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).ok()?;
        // SAFETY: layout valide.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return None;
        }
        self.inner.borrow_mut().allocs.push((ptr, layout));
        Some(DmaRegion {
            phys: ptr as u64,
            virt: ptr,
            pages,
        })
    }

    unsafe fn dma_dealloc(&self, _region: DmaRegion) {
        // Libérées en bloc au Drop (simplicité test).
    }

    fn mmio_read32(&self, off: usize) -> u32 {
        let inner = self.inner.borrow();
        // Globaux.
        match off {
            regs::HBA_CAP => return (31 << 8) | 0, // NCS=32, NP=1 port
            regs::HBA_GHC => return inner.ghc,
            regs::HBA_PI => return 0x1, // port 0 implémenté
            regs::HBA_VS => return 0x0001_0301,
            _ => {}
        }
        // Port 0.
        let base = regs::port_base(0);
        match off.checked_sub(base) {
            Some(regs::PORT_SSTS) => 0x113, // DET=3, IPM=1
            Some(regs::PORT_SIG) => regs::SIG_SATA,
            // CR/FR toujours 0 (pas de moteur réellement en marche dans le mock).
            Some(regs::PORT_CMD) => inner.cmd & !(regs::CMD_CR | regs::CMD_FR),
            Some(regs::PORT_TFD) => 0, // jamais busy
            Some(regs::PORT_CI) => 0,  // commandes traitées synchrones → toujours 0
            Some(regs::PORT_IS) => 0,  // pas d'erreur
            _ => 0,
        }
    }

    fn mmio_write32(&self, off: usize, val: u32) {
        let mut inner = self.inner.borrow_mut();
        if off == regs::HBA_GHC {
            inner.ghc = val;
            return;
        }
        let base = regs::port_base(0);
        match off.checked_sub(base) {
            Some(regs::PORT_CLB) => inner.clb = (inner.clb & !0xFFFF_FFFF) | val as u64,
            Some(regs::PORT_CLBU) => inner.clb = (inner.clb & 0xFFFF_FFFF) | ((val as u64) << 32),
            Some(regs::PORT_FB) => inner.fb = (inner.fb & !0xFFFF_FFFF) | val as u64,
            Some(regs::PORT_FBU) => inner.fb = (inner.fb & 0xFFFF_FFFF) | ((val as u64) << 32),
            Some(regs::PORT_CMD) => inner.cmd = val,
            Some(regs::PORT_CI) => {
                // Émission : traiter chaque slot demandé puis « compléter ».
                let mut slot = 0u32;
                while slot < 32 {
                    if val & (1 << slot) != 0 {
                        inner.process_slot(slot);
                    }
                    slot += 1;
                }
            }
            _ => {}
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn detects_sata_and_identifies() {
    let dev = AhciDevice::new(MockAhci::new()).expect("init AHCI");
    assert_eq!(dev.block_size(), 4096);
    assert_eq!(dev.sector_size, 512);
    assert_eq!(dev.total_blocks(), MOCK_BLOCKS);
}

#[test]
fn write_then_read_roundtrip() {
    let mut dev = AhciDevice::new(MockAhci::new()).expect("init");
    let mut wbuf = [0u8; EXOFS_BLOCK_SIZE];
    for (i, b) in wbuf.iter_mut().enumerate() {
        *b = (i ^ 0x3C) as u8;
    }
    dev.write_block(20, &wbuf).expect("write");
    let mut rbuf = [0u8; EXOFS_BLOCK_SIZE];
    dev.read_block(20, &mut rbuf).expect("read");
    assert_eq!(rbuf, wbuf);
}

#[test]
fn unwritten_block_reads_zero() {
    let mut dev = AhciDevice::new(MockAhci::new()).expect("init");
    let mut rbuf = [0xAAu8; EXOFS_BLOCK_SIZE];
    dev.read_block(7, &mut rbuf).expect("read");
    assert!(rbuf.iter().all(|&b| b == 0));
}

#[test]
fn many_blocks_roundtrip() {
    let mut dev = AhciDevice::new(MockAhci::new()).expect("init");
    for blk in 0..MOCK_BLOCKS {
        let buf = [(blk as u8).wrapping_add(1); EXOFS_BLOCK_SIZE];
        dev.write_block(blk, &buf).expect("write");
    }
    for blk in 0..MOCK_BLOCKS {
        let mut buf = [0u8; EXOFS_BLOCK_SIZE];
        dev.read_block(blk, &mut buf).expect("read");
        assert!(buf.iter().all(|&b| b == (blk as u8).wrapping_add(1)));
    }
}

#[test]
fn out_of_bounds_rejected() {
    let mut dev = AhciDevice::new(MockAhci::new()).expect("init");
    let mut buf = [0u8; EXOFS_BLOCK_SIZE];
    assert_eq!(dev.read_block(MOCK_BLOCKS, &mut buf), Err(AhciError::OutOfBounds));
}

#[test]
fn wrong_buffer_size_rejected() {
    let mut dev = AhciDevice::new(MockAhci::new()).expect("init");
    let mut small = [0u8; 1024];
    assert_eq!(dev.read_block(0, &mut small), Err(AhciError::InvalidBuffer));
}

#[test]
fn flush_succeeds() {
    let mut dev = AhciDevice::new(MockAhci::new()).expect("init");
    dev.flush().expect("flush");
}
