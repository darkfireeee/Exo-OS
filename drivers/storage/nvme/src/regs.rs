//! NVMe controller register layout (NVM Express base spec 1.4, §3.1).
//!
//! Toutes les fonctions ici sont du **calcul de bits pur** (offsets, champs de
//! CAP/CC/CSTS, formule de doorbell). Aucun effet de bord → entièrement
//! testable unitairement, ce qui ferme la classe de bugs « décodage de registre
//! erroné » (offset/largeur de champ faux) qui est une source fréquente de CVE
//! dans les drivers de bas niveau.

// ─────────────────────────────────────────────────────────────────────────────
// Offsets des registres contrôleur (dans le BAR0 MMIO)
// ─────────────────────────────────────────────────────────────────────────────

pub const REG_CAP: usize = 0x00; // Controller Capabilities (64-bit)
pub const REG_VS: usize = 0x08; // Version (32-bit)
pub const REG_INTMS: usize = 0x0C; // Interrupt Mask Set
pub const REG_INTMC: usize = 0x10; // Interrupt Mask Clear
pub const REG_CC: usize = 0x14; // Controller Configuration (32-bit)
pub const REG_CSTS: usize = 0x1C; // Controller Status (32-bit)
pub const REG_AQA: usize = 0x24; // Admin Queue Attributes (32-bit)
pub const REG_ASQ: usize = 0x28; // Admin Submission Queue Base (64-bit)
pub const REG_ACQ: usize = 0x30; // Admin Completion Queue Base (64-bit)
pub const REG_DOORBELL_BASE: usize = 0x1000;

// ─────────────────────────────────────────────────────────────────────────────
// CAP — Controller Capabilities (64-bit)
// ─────────────────────────────────────────────────────────────────────────────

/// MQES (Maximum Queue Entries Supported), bits 15:0 — **0-based**.
#[inline]
pub fn cap_mqes(cap: u64) -> u32 {
    (cap & 0xFFFF) as u32
}

/// Capacité max de file = MQES + 1 entrées (champ 0-based).
#[inline]
pub fn cap_max_queue_entries(cap: u64) -> u32 {
    cap_mqes(cap).saturating_add(1)
}

/// DSTRD (Doorbell Stride), bits 35:32. Stride = 4 << DSTRD octets.
#[inline]
pub fn cap_dstrd(cap: u64) -> u32 {
    ((cap >> 32) & 0xF) as u32
}

/// Stride de doorbell en octets = 4 << DSTRD.
#[inline]
pub fn cap_doorbell_stride(cap: u64) -> usize {
    4usize << cap_dstrd(cap)
}

/// TO (Timeout), bits 31:24 — en unités de 500 ms (worst-case CSTS.RDY).
#[inline]
pub fn cap_timeout_500ms_units(cap: u64) -> u32 {
    ((cap >> 24) & 0xFF) as u32
}

/// MPSMIN (Memory Page Size Minimum), bits 51:48. Taille = 2^(12 + MPSMIN).
#[inline]
pub fn cap_mpsmin_shift(cap: u64) -> u32 {
    12 + ((cap >> 48) & 0xF) as u32
}

// ─────────────────────────────────────────────────────────────────────────────
// CC — Controller Configuration (32-bit)
// ─────────────────────────────────────────────────────────────────────────────

pub const CC_EN: u32 = 1 << 0;

/// Construit une valeur CC : EN + CSS=0 (NVM cmd set) + MPS + AMS=0 (RR) +
/// IOSQES=6 (2^6=64 octets) + IOCQES=4 (2^4=16 octets).
///
/// `mps` = log2(page_size) - 12 (doit être >= CAP.MPSMIN).
#[inline]
pub fn cc_value(enable: bool, mps: u32) -> u32 {
    let mut cc = 0u32;
    if enable {
        cc |= CC_EN;
    }
    // CSS (bits 6:4) = 0 : NVM command set.
    cc |= (mps & 0xF) << 7; // MPS bits 10:7
    // AMS (bits 13:11) = 0 : round-robin.
    cc |= 6 << 16; // IOSQES bits 19:16 = 6  (entrée SQ = 64 octets)
    cc |= 4 << 20; // IOCQES bits 23:20 = 4  (entrée CQ = 16 octets)
    cc
}

// ─────────────────────────────────────────────────────────────────────────────
// CSTS — Controller Status (32-bit)
// ─────────────────────────────────────────────────────────────────────────────

pub const CSTS_RDY: u32 = 1 << 0;
pub const CSTS_CFS: u32 = 1 << 1; // Controller Fatal Status

#[inline]
pub fn csts_ready(csts: u32) -> bool {
    csts & CSTS_RDY != 0
}

#[inline]
pub fn csts_fatal(csts: u32) -> bool {
    csts & CSTS_CFS != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// AQA — Admin Queue Attributes (32-bit)
// ─────────────────────────────────────────────────────────────────────────────

/// AQA = ASQS (bits 11:0, 0-based) | ACQS (bits 27:16, 0-based).
/// `entries` = nombre d'entrées de file (≥ 2, ≤ 4096).
#[inline]
pub fn aqa_value(sq_entries: u32, cq_entries: u32) -> u32 {
    let asqs = sq_entries.saturating_sub(1) & 0xFFF;
    let acqs = cq_entries.saturating_sub(1) & 0xFFF;
    asqs | (acqs << 16)
}

// ─────────────────────────────────────────────────────────────────────────────
// Doorbells (NVMe base spec §3.1.24/25)
// ─────────────────────────────────────────────────────────────────────────────

/// Offset du Submission Queue `qid` Tail Doorbell.
/// `1000h + (2 * qid) * stride`.
#[inline]
pub fn sq_tail_doorbell(qid: u32, stride: usize) -> usize {
    REG_DOORBELL_BASE + (2 * qid as usize) * stride
}

/// Offset du Completion Queue `qid` Head Doorbell.
/// `1000h + (2 * qid + 1) * stride`.
#[inline]
pub fn cq_head_doorbell(qid: u32, stride: usize) -> usize {
    REG_DOORBELL_BASE + (2 * qid as usize + 1) * stride
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doorbell_offsets_match_spec_stride0() {
        // DSTRD=0 → stride 4. SQ0TDBL=0x1000, CQ0HDBL=0x1004, SQ1TDBL=0x1008.
        let stride = cap_doorbell_stride(0);
        assert_eq!(stride, 4);
        assert_eq!(sq_tail_doorbell(0, stride), 0x1000);
        assert_eq!(cq_head_doorbell(0, stride), 0x1004);
        assert_eq!(sq_tail_doorbell(1, stride), 0x1008);
        assert_eq!(cq_head_doorbell(1, stride), 0x100C);
    }

    #[test]
    fn doorbell_offsets_respect_stride() {
        // DSTRD=2 → stride 16.
        let cap = 2u64 << 32;
        let stride = cap_doorbell_stride(cap);
        assert_eq!(stride, 16);
        assert_eq!(sq_tail_doorbell(0, stride), 0x1000);
        assert_eq!(cq_head_doorbell(0, stride), 0x1010);
        assert_eq!(sq_tail_doorbell(1, stride), 0x1020);
    }

    #[test]
    fn cap_fields_decode() {
        // MQES=63 (64 entrées), DSTRD=0, TO=20 (10s), MPSMIN=0 (4KiB).
        let cap: u64 = 63 | (20u64 << 24);
        assert_eq!(cap_mqes(cap), 63);
        assert_eq!(cap_max_queue_entries(cap), 64);
        assert_eq!(cap_timeout_500ms_units(cap), 20);
        assert_eq!(cap_mpsmin_shift(cap), 12);
    }

    #[test]
    fn cc_value_has_correct_entry_sizes() {
        let cc = cc_value(true, 0);
        assert_eq!(cc & CC_EN, CC_EN);
        assert_eq!((cc >> 16) & 0xF, 6, "IOSQES doit coder 2^6=64 octets");
        assert_eq!((cc >> 20) & 0xF, 4, "IOCQES doit coder 2^4=16 octets");
    }

    #[test]
    fn aqa_packs_zero_based() {
        let aqa = aqa_value(64, 64);
        assert_eq!(aqa & 0xFFF, 63);
        assert_eq!((aqa >> 16) & 0xFFF, 63);
    }

    #[test]
    fn csts_predicates() {
        assert!(csts_ready(CSTS_RDY));
        assert!(!csts_ready(0));
        assert!(csts_fatal(CSTS_CFS));
    }
}
