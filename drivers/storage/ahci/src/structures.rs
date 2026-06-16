//! Structures DMA AHCI : Command Header, PRDT entry, FIS Register H2D.
//!
//! Encodage **pur** (pas de MMIO) → testable. C'est ici que vivent les bugs de
//! driver classiques (DBC non 0-based, LBA48 mal empaqueté, CFL faux).

// ── Commandes ATA ────────────────────────────────────────────────────────────

pub mod ata {
    pub const READ_DMA_EXT: u8 = 0x25;
    pub const WRITE_DMA_EXT: u8 = 0x35;
    pub const IDENTIFY_DEVICE: u8 = 0xEC;
    pub const FLUSH_CACHE_EXT: u8 = 0xEA;
}

/// Type de FIS Register Host-to-Device.
pub const FIS_TYPE_REG_H2D: u8 = 0x27;

// ── Command Header (32 octets ; 32 par liste, alignée 1 Kio) ─────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CmdHeader {
    /// DW0 bits 15:0 : CFL[4:0], A[5], W[6], P[7], R[8], B[9], C[10], PMP[15:12].
    pub flags: u16,
    /// DW0 bits 31:16 : PRDTL (nombre d'entrées PRDT).
    pub prdtl: u16,
    /// DW1 : PRDBC (octets transférés ; écrit par le HBA).
    pub prdbc: u32,
    /// DW2 : CTBA (base de la command table, alignée 128 octets).
    pub ctba: u32,
    /// DW3 : CTBAU (base haute).
    pub ctbau: u32,
    /// DW4-7 : réservé.
    pub _rsv: [u32; 4],
}

const _: () = assert!(core::mem::size_of::<CmdHeader>() == 32);

impl CmdHeader {
    /// `cfl_dwords` = longueur du Command FIS en dwords (5 pour FIS_REG_H2D).
    /// `write` → bit W. `prdtl` = nombre d'entrées PRDT. `ctba_phys` aligné 128.
    pub fn new(cfl_dwords: u8, write: bool, prdtl: u16, ctba_phys: u64) -> Self {
        let mut flags = (cfl_dwords as u16) & 0x1F;
        if write {
            flags |= 1 << 6; // W
        }
        Self {
            flags,
            prdtl,
            prdbc: 0,
            ctba: ctba_phys as u32,
            ctbau: (ctba_phys >> 32) as u32,
            _rsv: [0; 4],
        }
    }

    #[inline]
    pub fn is_write(&self) -> bool {
        self.flags & (1 << 6) != 0
    }
    #[inline]
    pub fn cfl(&self) -> u8 {
        (self.flags & 0x1F) as u8
    }
}

// ── PRDT entry (16 octets) ───────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PrdtEntry {
    pub dba: u32,  // Data Base Address (bas)
    pub dbau: u32, // Data Base Address (haut)
    pub _rsv: u32,
    /// DBC[21:0] (octets - 1, 0-based, max 4 Mio), I[31] = interrupt.
    pub dbc: u32,
}

const _: () = assert!(core::mem::size_of::<PrdtEntry>() == 16);

/// Taille max d'un transfert par entrée PRDT : 4 Mio (DBC 22 bits, 0-based).
pub const PRDT_MAX_BYTES: usize = 4 * 1024 * 1024;

impl PrdtEntry {
    /// `byte_count` = octets de la région (1..=4 Mio). `interrupt` → bit I.
    /// Retourne `None` si la taille est nulle ou > 4 Mio (refus, pas d'encodage
    /// silencieusement tronqué — anti-corruption).
    pub fn new(addr: u64, byte_count: usize, interrupt: bool) -> Option<Self> {
        if byte_count == 0 || byte_count > PRDT_MAX_BYTES {
            return None;
        }
        let mut dbc = ((byte_count - 1) as u32) & 0x003F_FFFF; // 0-based, 22 bits
        if interrupt {
            dbc |= 1 << 31;
        }
        Some(Self {
            dba: addr as u32,
            dbau: (addr >> 32) as u32,
            _rsv: 0,
            dbc,
        })
    }

    #[inline]
    pub fn byte_count(&self) -> usize {
        ((self.dbc & 0x003F_FFFF) as usize) + 1
    }
}

// ── FIS Register Host-to-Device (20 octets) ──────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FisRegH2D {
    pub fis_type: u8, // 0x27
    pub pmport_c: u8, // bit 7 = C (1 = commande)
    pub command: u8,  // commande ATA
    pub featurel: u8,
    pub lba0: u8, // LBA 7:0
    pub lba1: u8, // LBA 15:8
    pub lba2: u8, // LBA 23:16
    pub device: u8,
    pub lba3: u8, // LBA 31:24
    pub lba4: u8, // LBA 39:32
    pub lba5: u8, // LBA 47:40
    pub featureh: u8,
    pub countl: u8,
    pub counth: u8,
    pub icc: u8,
    pub control: u8,
    pub _rsv: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<FisRegH2D>() == 20);

/// Longueur du FIS Register H2D en dwords (pour CFL).
pub const FIS_H2D_DWORDS: u8 = 5;

impl FisRegH2D {
    /// Construit un FIS READ/WRITE DMA EXT (LBA48). `lba` = LBA de départ
    /// (48 bits utiles), `sectors` = nombre de secteurs.
    pub fn read_write(write: bool, lba: u64, sectors: u16) -> Self {
        Self {
            fis_type: FIS_TYPE_REG_H2D,
            pmport_c: 1 << 7, // C = 1 (commande)
            command: if write {
                ata::WRITE_DMA_EXT
            } else {
                ata::READ_DMA_EXT
            },
            featurel: 0,
            lba0: lba as u8,
            lba1: (lba >> 8) as u8,
            lba2: (lba >> 16) as u8,
            device: 1 << 6, // mode LBA
            lba3: (lba >> 24) as u8,
            lba4: (lba >> 32) as u8,
            lba5: (lba >> 40) as u8,
            featureh: 0,
            countl: sectors as u8,
            counth: (sectors >> 8) as u8,
            icc: 0,
            control: 0,
            _rsv: [0; 4],
        }
    }

    /// Construit un FIS IDENTIFY DEVICE (0xEC).
    pub fn identify() -> Self {
        Self {
            fis_type: FIS_TYPE_REG_H2D,
            pmport_c: 1 << 7,
            command: ata::IDENTIFY_DEVICE,
            device: 0,
            ..Default::default()
        }
    }

    /// Construit un FIS FLUSH CACHE EXT (0xEA).
    pub fn flush() -> Self {
        Self {
            fis_type: FIS_TYPE_REG_H2D,
            pmport_c: 1 << 7,
            command: ata::FLUSH_CACHE_EXT,
            device: 1 << 6,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_header_encodes_cfl_write_prdtl() {
        let h = CmdHeader::new(FIS_H2D_DWORDS, true, 1, 0xCAFE_0000);
        assert_eq!(h.cfl(), 5);
        assert!(h.is_write());
        assert_eq!(h.prdtl, 1);
        assert_eq!(h.ctba, 0xCAFE_0000);
        let r = CmdHeader::new(FIS_H2D_DWORDS, false, 2, 0x1_0000_0000);
        assert!(!r.is_write());
        assert_eq!(r.ctbau, 1);
    }

    #[test]
    fn prdt_dbc_is_zero_based_with_interrupt() {
        let p = PrdtEntry::new(0x2000, 4096, true).unwrap();
        assert_eq!(p.byte_count(), 4096);
        assert_eq!(p.dbc & 0x003F_FFFF, 4095, "DBC doit être 0-based");
        assert_eq!(p.dbc >> 31, 1, "bit interrupt");
        assert_eq!(p.dba, 0x2000);
    }

    #[test]
    fn prdt_rejects_zero_and_oversize() {
        assert!(PrdtEntry::new(0x1000, 0, false).is_none());
        assert!(PrdtEntry::new(0x1000, PRDT_MAX_BYTES + 1, false).is_none());
        assert!(PrdtEntry::new(0x1000, PRDT_MAX_BYTES, false).is_some());
    }

    #[test]
    fn fis_packs_lba48_and_count() {
        // LBA48 = 0x00AB_CDEF_1234, 8 secteurs, lecture.
        let f = FisRegH2D::read_write(false, 0x0000_ABCD_EF12_3456 & 0xFFFF_FFFF_FFFF, 8);
        assert_eq!(f.fis_type, FIS_TYPE_REG_H2D);
        assert_eq!(f.command, ata::READ_DMA_EXT);
        assert_eq!(f.pmport_c, 0x80);
        assert_eq!(f.device, 1 << 6);
        let lba: u64 = 0x0000_ABCD_EF12_3456 & 0xFFFF_FFFF_FFFF;
        assert_eq!(f.lba0, lba as u8);
        assert_eq!(f.lba1, (lba >> 8) as u8);
        assert_eq!(f.lba2, (lba >> 16) as u8);
        assert_eq!(f.lba3, (lba >> 24) as u8);
        assert_eq!(f.lba4, (lba >> 32) as u8);
        assert_eq!(f.lba5, (lba >> 40) as u8);
        assert_eq!(f.countl, 8);
        assert_eq!(f.counth, 0);
    }

    #[test]
    fn fis_write_uses_write_dma_ext() {
        let f = FisRegH2D::read_write(true, 0, 1);
        assert_eq!(f.command, ata::WRITE_DMA_EXT);
    }

    #[test]
    fn fis_identify_opcode() {
        assert_eq!(FisRegH2D::identify().command, ata::IDENTIFY_DEVICE);
        assert_eq!(FisRegH2D::flush().command, ata::FLUSH_CACHE_EXT);
    }
}
