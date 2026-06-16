//! Test d'intégration : un **contrôleur NVMe simulé** en mémoire valide le
//! chemin complet du driver (reset → enable → create I/O queues → identify →
//! read/write/flush). Prouve que `submit_and_poll`, la séquence d'init, le phase
//! tag et l'encodage de commande fonctionnent ensemble — pas seulement isolément.
//!
//! Le mock pose `phys == virt` (un seul espace d'adressage en test) : il
//! interprète les adresses physiques des files/PRP comme des pointeurs.

extern crate std;

use super::*;
use crate::cmd::{admin, cns, nvm};
use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::vec::Vec;
use core::cell::RefCell;

const MOCK_LBA_SIZE: u32 = 512;
const MOCK_BLOCKS: u64 = 64; // 64 blocs ExoFS de 4096 o
const MOCK_DISK_BYTES: usize = (MOCK_BLOCKS as usize) * EXOFS_BLOCK_SIZE;

struct QueueDesc {
    base: u64,
    size: u16,
    head: u16, // côté contrôleur (SQ : prochaine à traiter ; CQ : prochain slot à écrire)
    phase: bool,
}

struct MockInner {
    cc: u32,
    aqa: u32,
    asq: u64,
    acq: u64,
    enabled: bool,
    admin_sq: Option<QueueDesc>,
    admin_cq: Option<QueueDesc>,
    io_sq: Option<QueueDesc>,
    io_cq: Option<QueueDesc>,
    disk: Vec<u8>,
    allocs: Vec<(u64, *mut u8, Layout)>,
}

pub struct MockNvme {
    inner: RefCell<MockInner>,
}

impl MockNvme {
    fn new() -> Self {
        Self {
            inner: RefCell::new(MockInner {
                cc: 0,
                aqa: 0,
                asq: 0,
                acq: 0,
                enabled: false,
                admin_sq: None,
                admin_cq: None,
                io_sq: None,
                io_cq: None,
                disk: alloc::vec![0u8; MOCK_DISK_BYTES],
                allocs: Vec::new(),
            }),
        }
    }
}

impl Drop for MockNvme {
    fn drop(&mut self) {
        let inner = self.inner.borrow();
        for &(_, ptr, layout) in inner.allocs.iter() {
            // SAFETY: chaque (ptr, layout) provient de dma_alloc ci-dessous.
            unsafe { dealloc(ptr, layout) };
        }
    }
}

// ── Lecture/écriture mémoire « device » via phys==virt ──────────────────────

unsafe fn rd32(phys: u64, dword_index: usize) -> u32 {
    let p = (phys as *const u32).add(dword_index);
    core::ptr::read_volatile(p)
}

unsafe fn read_sqe(sq_base: u64, slot: u16) -> Sqe {
    let p = (sq_base as *const Sqe).add(slot as usize);
    core::ptr::read_volatile(p)
}

unsafe fn write_cqe(cq_base: u64, slot: u16, sq_head: u16, sqid: u16, cid: u16, phase: bool) {
    let p = (cq_base as *mut u32).add((slot as usize) * 4);
    core::ptr::write_volatile(p, 0); // DW0 result
    core::ptr::write_volatile(p.add(1), 0); // DW1
    core::ptr::write_volatile(p.add(2), (sq_head as u32) | ((sqid as u32) << 16));
    let dw3 = (cid as u32) | ((phase as u32) << 16); // status 0 = succès
    core::ptr::write_volatile(p.add(3), dw3);
}

impl MockInner {
    fn post_completion(cq: &mut QueueDesc, sq_head: u16, sqid: u16, cid: u16) {
        // SAFETY: cq.base est une page DMA allouée ; slot < size.
        unsafe { write_cqe(cq.base, cq.head, sq_head, sqid, cid, cq.phase) };
        cq.head += 1;
        if cq.head >= cq.size {
            cq.head = 0;
            cq.phase = !cq.phase;
        }
    }

    fn process_admin(&mut self, new_tail: u16) {
        let (sq_base, sq_size) = {
            let sq = self.admin_sq.as_ref().unwrap();
            (sq.base, sq.size)
        };
        loop {
            let head = self.admin_sq.as_ref().unwrap().head;
            if head == new_tail {
                break;
            }
            // SAFETY: head < size ; SQ allouée.
            let sqe = unsafe { read_sqe(sq_base, head) };
            let opcode = sqe.opcode();
            let cid = sqe.cid();
            match opcode {
                admin::IDENTIFY => {
                    let prp1 = (sqe.dword[6] as u64) | ((sqe.dword[7] as u64) << 32);
                    let cns_v = sqe.dword[10] & 0xFF;
                    // SAFETY: prp1 = buffer DMA de 4096 o.
                    unsafe {
                        let buf = prp1 as *mut u8;
                        core::ptr::write_bytes(buf, 0, 4096);
                        if cns_v == cns::NAMESPACE {
                            // NSZE (octets 0-7) = capacité en LBA.
                            let nsze = (MOCK_DISK_BYTES as u64) / (MOCK_LBA_SIZE as u64);
                            for (i, b) in nsze.to_le_bytes().iter().enumerate() {
                                core::ptr::write_volatile(buf.add(i), *b);
                            }
                            // FLBAS (octet 26) = 0 → format 0.
                            core::ptr::write_volatile(buf.add(26), 0);
                            // LBAF0 (octet 128) : LBADS (bits 23:16) = log2(512)=9.
                            let lbaf: u32 = 9 << 16;
                            for (i, b) in lbaf.to_le_bytes().iter().enumerate() {
                                core::ptr::write_volatile(buf.add(128 + i), *b);
                            }
                        }
                    }
                }
                admin::CREATE_IO_CQ => {
                    let prp1 = (sqe.dword[6] as u64) | ((sqe.dword[7] as u64) << 32);
                    let qsize = ((sqe.dword[10] >> 16) & 0xFFFF) as u16 + 1;
                    self.io_cq = Some(QueueDesc {
                        base: prp1,
                        size: qsize,
                        head: 0,
                        phase: true,
                    });
                }
                admin::CREATE_IO_SQ => {
                    let prp1 = (sqe.dword[6] as u64) | ((sqe.dword[7] as u64) << 32);
                    let qsize = ((sqe.dword[10] >> 16) & 0xFFFF) as u16 + 1;
                    self.io_sq = Some(QueueDesc {
                        base: prp1,
                        size: qsize,
                        head: 0,
                        phase: true,
                    });
                }
                _ => {}
            }
            // Avancer le head SQ contrôleur.
            let next = {
                let sq = self.admin_sq.as_mut().unwrap();
                sq.head += 1;
                if sq.head >= sq_size {
                    sq.head = 0;
                }
                sq.head
            };
            let acq = self.admin_cq.as_mut().unwrap();
            MockInner::post_completion(acq, next, 0, cid);
        }
    }

    fn process_io(&mut self, new_tail: u16) {
        let (sq_base, sq_size) = {
            let sq = self.io_sq.as_ref().unwrap();
            (sq.base, sq.size)
        };
        loop {
            let head = self.io_sq.as_ref().unwrap().head;
            if head == new_tail {
                break;
            }
            // SAFETY: head < size.
            let sqe = unsafe { read_sqe(sq_base, head) };
            let opcode = sqe.opcode();
            let cid = sqe.cid();
            let prp1 = (sqe.dword[6] as u64) | ((sqe.dword[7] as u64) << 32);
            let slba = (sqe.dword[10] as u64) | ((sqe.dword[11] as u64) << 32);
            let nlb = (sqe.dword[12] & 0xFFFF) as u64 + 1;
            let byte_off = (slba * MOCK_LBA_SIZE as u64) as usize;
            let byte_len = (nlb * MOCK_LBA_SIZE as u64) as usize;
            match opcode {
                nvm::READ => {
                    if byte_off + byte_len <= self.disk.len() {
                        // SAFETY: prp1 = buffer DMA ≥ byte_len.
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                self.disk.as_ptr().add(byte_off),
                                prp1 as *mut u8,
                                byte_len,
                            );
                        }
                    }
                }
                nvm::WRITE => {
                    if byte_off + byte_len <= self.disk.len() {
                        // SAFETY: prp1 = buffer DMA ≥ byte_len.
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                prp1 as *const u8,
                                self.disk.as_mut_ptr().add(byte_off),
                                byte_len,
                            );
                        }
                    }
                }
                nvm::FLUSH => {}
                _ => {}
            }
            let next = {
                let sq = self.io_sq.as_mut().unwrap();
                sq.head += 1;
                if sq.head >= sq_size {
                    sq.head = 0;
                }
                sq.head
            };
            let iocq = self.io_cq.as_mut().unwrap();
            MockInner::post_completion(iocq, next, IO_QID, cid);
        }
    }
}

impl NvmeHal for MockNvme {
    fn dma_alloc(&self, pages: usize) -> Option<DmaRegion> {
        let layout = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).ok()?;
        // SAFETY: layout taille>0, alignement page valide.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return None;
        }
        let phys = ptr as u64; // mock : phys == virt
        self.inner.borrow_mut().allocs.push((phys, ptr, layout));
        Some(DmaRegion {
            phys,
            virt: ptr,
            pages,
        })
    }

    unsafe fn dma_dealloc(&self, _region: DmaRegion) {
        // Les allocations sont libérées en bloc au Drop du mock (simplicité test).
    }

    fn mmio_read32(&self, off: usize) -> u32 {
        let inner = self.inner.borrow();
        match off {
            regs::REG_CSTS => {
                if inner.enabled {
                    regs::CSTS_RDY
                } else {
                    0
                }
            }
            regs::REG_CC => inner.cc,
            regs::REG_VS => 0x0001_0400, // 1.4.0
            _ => 0,
        }
    }

    fn mmio_write32(&self, off: usize, val: u32) {
        let mut inner = self.inner.borrow_mut();
        // Doorbells (stride 4) : SQ0=0x1000(admin), SQ1=0x1008(I/O).
        if off == regs::sq_tail_doorbell(0, 4) {
            drop(inner);
            self.inner.borrow_mut().process_admin(val as u16);
            return;
        }
        if off == regs::sq_tail_doorbell(IO_QID as u32, 4) {
            drop(inner);
            self.inner.borrow_mut().process_io(val as u16);
            return;
        }
        match off {
            regs::REG_CC => {
                inner.cc = val;
                inner.enabled = val & regs::CC_EN != 0;
                if inner.enabled {
                    // Fixer les files admin depuis AQA/ASQ/ACQ.
                    let asqs = (inner.aqa & 0xFFF) as u16 + 1;
                    let acqs = ((inner.aqa >> 16) & 0xFFF) as u16 + 1;
                    let (asq, acq) = (inner.asq, inner.acq);
                    inner.admin_sq = Some(QueueDesc {
                        base: asq,
                        size: asqs,
                        head: 0,
                        phase: true,
                    });
                    inner.admin_cq = Some(QueueDesc {
                        base: acq,
                        size: acqs,
                        head: 0,
                        phase: true,
                    });
                }
            }
            regs::REG_AQA => inner.aqa = val,
            _ => {}
        }
    }

    fn mmio_read64(&self, off: usize) -> u64 {
        match off {
            // CAP : MQES=63 (64 entrées), DSTRD=0, TO=1.
            regs::REG_CAP => 63 | (1u64 << 24),
            _ => 0,
        }
    }

    fn mmio_write64(&self, off: usize, val: u64) {
        let mut inner = self.inner.borrow_mut();
        match off {
            regs::REG_ASQ => inner.asq = val,
            regs::REG_ACQ => inner.acq = val,
            _ => {}
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn controller_initializes_and_identifies_namespace() {
    let dev = NvmeDevice::new(MockNvme::new()).expect("init NVMe");
    assert_eq!(dev.block_size(), 4096);
    assert_eq!(dev.lba_size, MOCK_LBA_SIZE);
    assert_eq!(dev.total_blocks(), MOCK_BLOCKS);
}

#[test]
fn write_then_read_roundtrip() {
    let mut dev = NvmeDevice::new(MockNvme::new()).expect("init");
    let mut wbuf = [0u8; EXOFS_BLOCK_SIZE];
    for (i, b) in wbuf.iter_mut().enumerate() {
        *b = (i ^ 0x5A) as u8;
    }
    dev.write_block(10, &wbuf).expect("write");
    let mut rbuf = [0u8; EXOFS_BLOCK_SIZE];
    dev.read_block(10, &mut rbuf).expect("read");
    assert_eq!(rbuf, wbuf, "le bloc relu doit égaler le bloc écrit");
}

#[test]
fn unwritten_block_reads_zero() {
    let mut dev = NvmeDevice::new(MockNvme::new()).expect("init");
    let mut rbuf = [0xAAu8; EXOFS_BLOCK_SIZE];
    dev.read_block(5, &mut rbuf).expect("read");
    assert!(rbuf.iter().all(|&b| b == 0));
}

#[test]
fn multiple_blocks_roundtrip_distinct_data() {
    let mut dev = NvmeDevice::new(MockNvme::new()).expect("init");
    for blk in 0..MOCK_BLOCKS {
        let fill = (blk as u8).wrapping_mul(7);
        let buf = [fill; EXOFS_BLOCK_SIZE];
        dev.write_block(blk, &buf).expect("write");
    }
    for blk in 0..MOCK_BLOCKS {
        let fill = (blk as u8).wrapping_mul(7);
        let mut buf = [0u8; EXOFS_BLOCK_SIZE];
        dev.read_block(blk, &mut buf).expect("read");
        assert!(buf.iter().all(|&b| b == fill), "bloc {} corrompu", blk);
    }
}

#[test]
fn out_of_bounds_block_rejected() {
    let mut dev = NvmeDevice::new(MockNvme::new()).expect("init");
    let mut buf = [0u8; EXOFS_BLOCK_SIZE];
    assert_eq!(dev.read_block(MOCK_BLOCKS, &mut buf), Err(NvmeError::OutOfBounds));
    assert_eq!(dev.write_block(9999, &buf), Err(NvmeError::OutOfBounds));
}

#[test]
fn wrong_buffer_size_rejected() {
    let mut dev = NvmeDevice::new(MockNvme::new()).expect("init");
    let mut small = [0u8; 512];
    assert_eq!(dev.read_block(0, &mut small), Err(NvmeError::InvalidBuffer));
}

#[test]
fn flush_succeeds() {
    let mut dev = NvmeDevice::new(MockNvme::new()).expect("init");
    dev.flush().expect("flush");
}

#[test]
fn phase_tag_holds_across_many_commands() {
    // Plus de commandes que la taille de file → force le wraparound CQ + le
    // basculement de phase. Si la logique de phase était fausse, le poll
    // bloquerait ou lirait une complétion périmée.
    let mut dev = NvmeDevice::new(MockNvme::new()).expect("init");
    let buf = [0x33u8; EXOFS_BLOCK_SIZE];
    for _ in 0..200 {
        dev.write_block(0, &buf).expect("write");
    }
    let mut rbuf = [0u8; EXOFS_BLOCK_SIZE];
    dev.read_block(0, &mut rbuf).expect("read");
    assert!(rbuf.iter().all(|&b| b == 0x33));
}
