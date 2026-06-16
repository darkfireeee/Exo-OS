//! Layout des registres AHCI (Serial ATA AHCI 1.3.1).
//!
//! Calcul de bits/offsets **pur** → testable. Verrouille la classe de bugs
//! « offset de registre / champ erroné » fréquente dans les drivers AHCI.

// ─────────────────────────────────────────────────────────────────────────────
// HBA_MEM — registres globaux (relatifs à l'ABAR / BAR5)
// ─────────────────────────────────────────────────────────────────────────────

pub const HBA_CAP: usize = 0x00; // Host Capabilities
pub const HBA_GHC: usize = 0x04; // Global Host Control
pub const HBA_IS: usize = 0x08; // Interrupt Status (par port, bitmask)
pub const HBA_PI: usize = 0x0C; // Ports Implemented (bitmask)
pub const HBA_VS: usize = 0x10; // Version

/// Base des registres du port `n` (chaque port = 0x80 octets, à partir de 0x100).
#[inline]
pub fn port_base(n: u32) -> usize {
    0x100 + (n as usize) * 0x80
}

// GHC bits
pub const GHC_HR: u32 = 1 << 0; // HBA Reset
pub const GHC_IE: u32 = 1 << 1; // Interrupt Enable
pub const GHC_AE: u32 = 1 << 31; // AHCI Enable

// CAP bits
/// NCS (Number of Command Slots), bits 12:8 — **0-based**.
#[inline]
pub fn cap_ncs(cap: u32) -> u32 {
    ((cap >> 8) & 0x1F) + 1
}
/// NP (Number of Ports), bits 4:0 — **0-based**.
#[inline]
pub fn cap_np(cap: u32) -> u32 {
    (cap & 0x1F) + 1
}
/// S64A : adressage 64 bits supporté (bit 31).
#[inline]
pub fn cap_s64a(cap: u32) -> bool {
    cap & (1 << 31) != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// HBA_PORT — registres par port (relatifs à port_base)
// ─────────────────────────────────────────────────────────────────────────────

pub const PORT_CLB: usize = 0x00; // Command List Base (1K aligné)
pub const PORT_CLBU: usize = 0x04;
pub const PORT_FB: usize = 0x08; // FIS Base (256 aligné)
pub const PORT_FBU: usize = 0x0C;
pub const PORT_IS: usize = 0x10; // Interrupt Status
pub const PORT_IE: usize = 0x14; // Interrupt Enable
pub const PORT_CMD: usize = 0x18; // Command and Status
pub const PORT_TFD: usize = 0x20; // Task File Data
pub const PORT_SIG: usize = 0x24; // Signature
pub const PORT_SSTS: usize = 0x28; // SATA Status (SCR0:SStatus)
pub const PORT_SCTL: usize = 0x2C; // SATA Control
pub const PORT_SERR: usize = 0x30; // SATA Error
pub const PORT_SACT: usize = 0x34; // SATA Active
pub const PORT_CI: usize = 0x38; // Command Issue

/// Offset absolu d'un registre de port `n`.
#[inline]
pub fn port_reg(n: u32, reg: usize) -> usize {
    port_base(n) + reg
}

// PORT_CMD bits
pub const CMD_ST: u32 = 1 << 0; // Start
pub const CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
pub const CMD_FR: u32 = 1 << 14; // FIS Receive Running
pub const CMD_CR: u32 = 1 << 15; // Command list Running

// PORT_TFD bits (status)
pub const TFD_ERR: u32 = 1 << 0;
pub const TFD_DRQ: u32 = 1 << 3;
pub const TFD_BSY: u32 = 1 << 7;

/// Le périphérique est-il occupé (BSY ou DRQ) ?
#[inline]
pub fn tfd_busy(tfd: u32) -> bool {
    tfd & (TFD_BSY | TFD_DRQ) != 0
}

// PORT_IS bits
pub const IS_TFES: u32 = 1 << 30; // Task File Error Status

// Signatures
pub const SIG_SATA: u32 = 0x0000_0101; // disque SATA
pub const SIG_SATAPI: u32 = 0xEB14_0101;

/// SSTS : périphérique présent et communication établie ?
/// DET (bits 3:0) == 3 ET IPM (bits 11:8) == 1 (actif).
#[inline]
pub fn ssts_device_ready(ssts: u32) -> bool {
    let det = ssts & 0x0F;
    let ipm = (ssts >> 8) & 0x0F;
    det == 3 && ipm == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_base_offsets() {
        assert_eq!(port_base(0), 0x100);
        assert_eq!(port_base(1), 0x180);
        assert_eq!(port_base(31), 0x100 + 31 * 0x80);
        assert_eq!(port_reg(2, PORT_CI), 0x200 + 0x38);
    }

    #[test]
    fn cap_decodes_zero_based_counts() {
        // NCS field=31 → 32 slots ; NP field=0 → 1 port.
        let cap = (31u32 << 8) | 0;
        assert_eq!(cap_ncs(cap), 32);
        assert_eq!(cap_np(cap), 1);
        assert!(cap_s64a((1 << 31) | cap));
    }

    #[test]
    fn tfd_busy_detects_bsy_or_drq() {
        assert!(tfd_busy(TFD_BSY));
        assert!(tfd_busy(TFD_DRQ));
        assert!(!tfd_busy(0));
        assert!(!tfd_busy(TFD_ERR));
    }

    #[test]
    fn ssts_ready_requires_det3_ipm1() {
        assert!(ssts_device_ready(0x0123 & !0xFF | 0x0103)); // DET=3, IPM=1
        assert!(ssts_device_ready(0x113));
        assert!(!ssts_device_ready(0x0)); // pas de device
        assert!(!ssts_device_ready(0x3)); // DET=3 mais IPM=0 (partial/slumber)
        assert!(!ssts_device_ready(0x101)); // DET=1, IPM=1
    }

    #[test]
    fn sata_signature_constant() {
        assert_eq!(SIG_SATA, 0x0000_0101);
    }
}
