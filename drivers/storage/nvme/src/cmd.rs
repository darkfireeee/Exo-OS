//! Encodage des commandes NVMe (Submission Queue Entry 64 octets) et décodage
//! des complétions (Completion Queue Entry 16 octets).
//!
//! Tout est **pur** (pas de MMIO/DMA) → unit-testable. C'est ici que vivent les
//! bugs classiques de driver (mauvais offset de champ, NLB non 0-based, PRP hors
//! borne) ; les isoler ici permet de les tester exhaustivement.

// ── Opcodes ─────────────────────────────────────────────────────────────────

/// Opcodes admin.
pub mod admin {
    pub const CREATE_IO_SQ: u8 = 0x01;
    pub const CREATE_IO_CQ: u8 = 0x05;
    pub const IDENTIFY: u8 = 0x06;
}

/// Opcodes NVM (I/O).
pub mod nvm {
    pub const FLUSH: u8 = 0x00;
    pub const WRITE: u8 = 0x01;
    pub const READ: u8 = 0x02;
}

/// CNS pour Identify.
pub mod cns {
    pub const NAMESPACE: u32 = 0x00;
    pub const CONTROLLER: u32 = 0x01;
}

// ── Submission Queue Entry (64 octets = 16 dwords) ───────────────────────────

/// Entrée de file de soumission NVMe. `repr(C)` 64 octets, écrite telle quelle
/// dans la mémoire DMA de la SQ.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sqe {
    pub dword: [u32; 16],
}

const _: () = assert!(core::mem::size_of::<Sqe>() == 64);

impl Sqe {
    pub const fn zeroed() -> Self {
        Self { dword: [0; 16] }
    }

    /// CDW0 = opcode | (CID << 16). FUSE=0, PSDT=0 (PRP).
    #[inline]
    fn set_cdw0(&mut self, opcode: u8, cid: u16) {
        self.dword[0] = opcode as u32 | ((cid as u32) << 16);
    }

    #[inline]
    fn set_nsid(&mut self, nsid: u32) {
        self.dword[1] = nsid;
    }

    #[inline]
    fn set_prp1(&mut self, prp1: u64) {
        self.dword[6] = prp1 as u32;
        self.dword[7] = (prp1 >> 32) as u32;
    }

    #[inline]
    fn set_prp2(&mut self, prp2: u64) {
        self.dword[8] = prp2 as u32;
        self.dword[9] = (prp2 >> 32) as u32;
    }

    #[inline]
    pub fn cid(&self) -> u16 {
        (self.dword[0] >> 16) as u16
    }

    #[inline]
    pub fn opcode(&self) -> u8 {
        (self.dword[0] & 0xFF) as u8
    }

    /// Identify (admin 0x06) → buffer de 4096 octets en PRP1.
    pub fn identify(cid: u16, nsid: u32, cns: u32, buf_phys: u64) -> Self {
        let mut s = Self::zeroed();
        s.set_cdw0(admin::IDENTIFY, cid);
        s.set_nsid(nsid);
        s.set_prp1(buf_phys);
        s.dword[10] = cns & 0xFF; // CDW10: CNS bits 7:0
        s
    }

    /// Create I/O Completion Queue (admin 0x05).
    /// `cq_phys` doit être contigu physiquement (PC=1). `ien`=interruptions.
    pub fn create_io_cq(cid: u16, qid: u16, qsize: u16, cq_phys: u64, ien: bool) -> Self {
        let mut s = Self::zeroed();
        s.set_cdw0(admin::CREATE_IO_CQ, cid);
        s.set_prp1(cq_phys);
        // CDW10 : QSIZE (0-based) bits 31:16 | QID bits 15:0.
        s.dword[10] = ((qsize.saturating_sub(1) as u32) << 16) | qid as u32;
        // CDW11 : PC bit0=1 (contigu), IEN bit1.
        s.dword[11] = 0x1 | if ien { 0x2 } else { 0 };
        s
    }

    /// Create I/O Submission Queue (admin 0x01), associée à `cqid`.
    pub fn create_io_sq(cid: u16, qid: u16, qsize: u16, sq_phys: u64, cqid: u16) -> Self {
        let mut s = Self::zeroed();
        s.set_cdw0(admin::CREATE_IO_SQ, cid);
        s.set_prp1(sq_phys);
        s.dword[10] = ((qsize.saturating_sub(1) as u32) << 16) | qid as u32;
        // CDW11 : PC bit0=1 | QPRIO bits 2:1=0 | CQID bits 31:16.
        s.dword[11] = 0x1 | ((cqid as u32) << 16);
        s
    }

    /// Read (NVM 0x02) ou Write (NVM 0x01) d'un seul transfert tenant en
    /// `prp1`(+`prp2`). `slba` = adresse de bloc logique de départ ;
    /// `nlb_zero_based` = (nombre de blocs - 1).
    pub fn read_write(
        write: bool,
        cid: u16,
        nsid: u32,
        slba: u64,
        nlb_zero_based: u16,
        prp1: u64,
        prp2: u64,
    ) -> Self {
        let mut s = Self::zeroed();
        s.set_cdw0(if write { nvm::WRITE } else { nvm::READ }, cid);
        s.set_nsid(nsid);
        s.set_prp1(prp1);
        s.set_prp2(prp2);
        s.dword[10] = slba as u32; // SLBA bits 31:0
        s.dword[11] = (slba >> 32) as u32; // SLBA bits 63:32
        s.dword[12] = nlb_zero_based as u32; // NLB bits 15:0 (0-based)
        s
    }
}

// ── PRP (Physical Region Page) ───────────────────────────────────────────────

/// Calcule (PRP1, PRP2) pour un transfert `len` octets démarrant au buffer
/// **physique** `buf_phys`, avec une page de `page_size` octets.
///
/// Restriction (volontaire, anti-bug) : ce driver borne les transferts à **2
/// pages** maximum (≤ 2 × page_size). Au-delà, une *PRP list* serait requise ;
/// on renvoie `None` plutôt que d'émettre une commande mal formée. Comme la
/// présentation bloc opère par blocs de 4096 octets (= 1 page), un transfert
/// tient toujours dans PRP1 seul.
pub fn build_prp(buf_phys: u64, len: usize, page_size: usize) -> Option<(u64, u64)> {
    if len == 0 || page_size == 0 {
        return None;
    }
    let offset = (buf_phys as usize) & (page_size - 1);
    let first = page_size - offset; // octets couverts par PRP1
    if len <= first {
        // Tient dans une seule page.
        return Some((buf_phys, 0));
    }
    let remaining = len - first;
    if remaining <= page_size {
        // Deuxième page : PRP2 = base de la page suivante (alignée).
        let prp2 = (buf_phys + first as u64) & !((page_size as u64) - 1);
        return Some((buf_phys, prp2));
    }
    None // > 2 pages : nécessiterait une PRP list — refus explicite.
}

// ── Completion Queue Entry (16 octets = 4 dwords) ────────────────────────────

/// Vue décodée d'une entrée de complétion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub sq_head: u16,
    pub sq_id: u16,
    pub cid: u16,
    pub phase: bool,
    /// Status Field (bits 31:17 de DW3). 0 = succès.
    pub status: u16,
}

impl Completion {
    /// Décode depuis les 4 dwords lus dans la CQ.
    pub fn from_dwords(dw: [u32; 4]) -> Self {
        Self {
            sq_head: (dw[2] & 0xFFFF) as u16,
            sq_id: (dw[2] >> 16) as u16,
            cid: (dw[3] & 0xFFFF) as u16,
            phase: (dw[3] >> 16) & 1 != 0,
            status: ((dw[3] >> 17) & 0x7FFF) as u16,
        }
    }

    #[inline]
    pub fn is_success(&self) -> bool {
        self.status == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_encodes_opcode_cid_cns_prp() {
        let s = Sqe::identify(0x1234, 0, cns::CONTROLLER, 0xDEAD_0000);
        assert_eq!(s.opcode(), admin::IDENTIFY);
        assert_eq!(s.cid(), 0x1234);
        assert_eq!(s.dword[10], cns::CONTROLLER);
        assert_eq!(s.dword[6], 0xDEAD_0000); // PRP1 low
        assert_eq!(s.dword[7], 0); // PRP1 high
    }

    #[test]
    fn read_command_nlb_is_zero_based() {
        // Lire 8 blocs logiques à partir de SLBA=0x1_0000_0002.
        let s = Sqe::read_write(false, 7, 1, 0x1_0000_0002, 7, 0xCAFE_0000, 0);
        assert_eq!(s.opcode(), nvm::READ);
        assert_eq!(s.dword[1], 1); // NSID
        assert_eq!(s.dword[10], 0x0000_0002); // SLBA low
        assert_eq!(s.dword[11], 0x0000_0001); // SLBA high
        assert_eq!(s.dword[12], 7, "NLB doit être 0-based (8 blocs → 7)");
        assert_eq!(s.dword[6], 0xCAFE_0000); // PRP1
    }

    #[test]
    fn write_command_sets_write_opcode() {
        let s = Sqe::read_write(true, 1, 1, 0, 0, 0x1000, 0);
        assert_eq!(s.opcode(), nvm::WRITE);
    }

    #[test]
    fn create_cq_packs_qsize_zero_based_and_pc() {
        let s = Sqe::create_io_cq(2, 1, 64, 0xF000, false);
        assert_eq!(s.opcode(), admin::CREATE_IO_CQ);
        assert_eq!(s.dword[10], (63 << 16) | 1); // qsize-1 | qid
        assert_eq!(s.dword[11], 0x1); // PC=1, IEN=0
    }

    #[test]
    fn create_sq_links_to_cqid() {
        let s = Sqe::create_io_sq(3, 1, 64, 0xE000, 1);
        assert_eq!(s.opcode(), admin::CREATE_IO_SQ);
        assert_eq!(s.dword[11], 0x1 | (1 << 16)); // PC=1 | CQID=1
    }

    #[test]
    fn prp_single_page_uses_prp1_only() {
        // Buffer aligné page, 4096 octets, page 4096 → 1 PRP.
        assert_eq!(build_prp(0x10_000, 4096, 4096), Some((0x10_000, 0)));
    }

    #[test]
    fn prp_two_pages_uses_prp2() {
        // Buffer aligné, 8192 octets → PRP1 + PRP2 (page suivante).
        assert_eq!(build_prp(0x10_000, 8192, 4096), Some((0x10_000, 0x11_000)));
    }

    #[test]
    fn prp_offset_buffer_spanning_two_pages() {
        // Offset 0x800, 4096 octets → traverse 2 pages.
        let (p1, p2) = build_prp(0x10_800, 4096, 4096).unwrap();
        assert_eq!(p1, 0x10_800);
        assert_eq!(p2, 0x11_000);
    }

    #[test]
    fn prp_over_two_pages_rejected() {
        // > 2 pages → refus (pas de PRP list dans ce driver).
        assert_eq!(build_prp(0x10_000, 3 * 4096, 4096), None);
        assert_eq!(build_prp(0, 4096, 4096), None.or(Some((0, 0)))); // base 0 ok mais sentinelle
    }

    #[test]
    fn completion_decodes_phase_status_cid() {
        // DW3 : CID=0x1234, P=1, status=0 (succès).
        let dw3 = 0x1234 | (1 << 16);
        let c = Completion::from_dwords([0, 0, (5) | (1 << 16), dw3]);
        assert_eq!(c.cid, 0x1234);
        assert!(c.phase);
        assert!(c.is_success());
        assert_eq!(c.sq_head, 5);
        assert_eq!(c.sq_id, 1);
    }

    #[test]
    fn completion_nonzero_status_is_failure() {
        let dw3 = 0x0001 | (1 << 16) | (0x02 << 17); // SC=2
        let c = Completion::from_dwords([0, 0, 0, dw3]);
        assert!(!c.is_success());
        assert_eq!(c.status, 0x02);
    }
}
