//! disk.rs — Lecture disque via BIOS INT 13h extension (LBA 48-bit).
//!
//! Utilisé par le chemin BIOS pour lire le kernel ELF et la configuration
//! depuis le premier disque dur (disque 0x80 selon la convention BIOS).
//!
//! IMPORTANT : INT 13h n'est disponible qu'en mode réel (16-bit).
//! Cette implémentation suppose que stage2.asm a lu les secteurs nécessaires
//! en mémoire AVANT le passage en mode long 64-bit.
//!
//! En mode long, la lecture de disque utilise :
//!   - Les structures de données remplies par stage2 en mode réel
//!   - Un "disk_buffer" statique positionné en mémoire basse (<0x100000)
//!     pour l'accès DMA BIOS (INT 13h EDD requiert <0x10000 pour le buffer)
//!
//! Layout du disque Exo-OS (BIOS/GPT simplifié) :
//!   Secteur 0        : MBR (stage 1)
//!   Secteurs 1-3     : GPT header + partition table
//!   Secteurs 64-127  : Stage 2 (4KB max)
//!   Secteurs 128-..  : exo-boot.cfg + kernel.elf (partition dédiée)



// ─── Constantes ───────────────────────────────────────────────────────────────

/// Numéro du premier disque dur selon la convention BIOS.
pub const BIOS_PRIMARY_DISK: u8 = 0x80;

/// Taille d'un secteur logique (512 bytes standard).
pub const SECTOR_SIZE: usize = 512;

/// Numéro de secteur LBA de début de la partition du kernel.
/// Convention Exo-OS : partition kernel commence au secteur 2048 (1 MB offset).
pub const KERNEL_PARTITION_LBA_START: u64 = 2048;

/// Taille maximale du kernel en secteurs (64 MB / 512 = 131072 secteurs).
pub const KERNEL_MAX_SECTORS: usize = 131072;

// ─── Structures de transfert BIOS INT 13h EDD ─────────────────────────────────

/// Disk Address Packet (DAP) pour INT 13h AX=4200h (Extended Read)
/// Chaque champ doit être dans l'ordre exact imposé par la spec BIOS EDD 3.0.
///
/// DOIT être en mémoire basse (<64KB) pour l'accès BIOS en mode réel.
/// En mode long, cette structure n'est plus utilisée directement
/// (les lectures ont déjà été effectuées par stage2).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DiskAddressPacket {
    /// Taille du packet (0x10 pour EDD 3.0).
    pub size:           u8,
    /// Réservé (toujours 0).
    pub reserved:       u8,
    /// Nombre de secteurs à lire (max 127 selon certains BIOSes).
    pub sector_count:   u16,
    /// Offset dans le segment du buffer de destination.
    pub buffer_offset:  u16,
    /// Segment du buffer de destination (real-mode segment 16-bit).
    pub buffer_segment: u16,
    /// Numéro de secteur LBA 64-bit.
    pub lba_start:      u64,
}

impl DiskAddressPacket {
    /// Crée un DAP initialisé avec les paramètres de lecture.
    pub const fn new(lba: u64, count: u16, buf_phys: u32) -> Self {
        Self {
            size:           0x10,
            reserved:       0,
            sector_count:   count,
            buffer_offset:  (buf_phys & 0xFFFF) as u16,
            buffer_segment: ((buf_phys >> 4) & 0xFFFF) as u16,
            lba_start:      lba,
        }
    }
}

// ─── BiosDisk ─────────────────────────────────────────────────────────────────

/// Abstraction d'un disque BIOS.
///
/// NOTE : En mode long 64-bit (où ce code Rust s'exécute), les interruptions
/// BIOS (INT 13h) ne sont plus disponibles directement. Ce struct encapsule
/// l'accès aux données pré-chargées par stage2.asm en mémoire.
pub struct BiosDisk {
    /// Numéro de disque BIOS (0x80 = hda, 0x81 = hdb, etc.)
    #[allow(dead_code)]
    drive_number: u8,
}

impl BiosDisk {
    /// Crée un handle vers le disque primaire (0x80).
    pub fn primary() -> Self {
        Self { drive_number: BIOS_PRIMARY_DISK }
    }

    /// Crée un handle vers un disque spécifique.
    pub fn with_number(number: u8) -> Self {
        Self { drive_number: number }
    }

    /// Lit `sector_count` secteurs à partir de `lba_start` vers `buf`.
    ///
    /// En mode long post-stage2, cette fonction lit depuis le "disk shadow buffer"
    /// rempli par stage2.asm(Voir documentation stage2.asm pour le layout mémoire).
    ///
    /// Pour les lectures larges (kernel.elf), stage2 a copié l'intégralité
    /// des secteurs kernel en mémoire avant le passage en mode long.
    ///
    /// `buf` doit avoir une taille >= sector_count * SECTOR_SIZE.
    pub fn read_sectors(
        &mut self,
        lba_start:    u64,
        sector_count: usize,
        buf:          &mut [u8],
    ) -> Result<(), DiskError> {
        if buf.len() < sector_count * SECTOR_SIZE {
            return Err(DiskError::BufferTooSmall {
                needed: sector_count * SECTOR_SIZE,
                got:    buf.len(),
            });
        }
        if sector_count > KERNEL_MAX_SECTORS {
            return Err(DiskError::TooManySectors {
                requested: sector_count,
                max:       KERNEL_MAX_SECTORS,
            });
        }

        // En mode long 64-bit, lit depuis la mémoire shadow remplie par stage2.
        // Le stage2 a mappé le contenu des secteurs à l'adresse :
        //   STAGE2_DISK_SHADOW_BASE + (lba - KERNEL_PARTITION_LBA_START) * SECTOR_SIZE
        self.read_from_shadow(lba_start, sector_count, buf)
    }

    /// Lit depuis le "disk shadow" — mémoire pré-remplie par stage2.asm.
    ///
    /// stage2.asm copie les secteurs du kernel en mémoire AVANT le passage
    /// en mode long, car INT 13h n'est plus utilisable après.
    fn read_from_shadow(
        &self,
        lba:          u64,
        sector_count: usize,
        buf:          &mut [u8],
    ) -> Result<(), DiskError> {
        /// Base du disk shadow buffer. Doit correspondre à la constante
        /// dans stage2.asm (KERNEL_SHADOW_BASE). Stage2 copie les secteurs ici.
        const STAGE2_DISK_SHADOW_BASE: u64 = 0x200000; // 2 MB — en mémoire haute
        const SHADOW_MAX_BYTES: usize = 64 * 1024 * 1024; // 64 MB

        if lba < KERNEL_PARTITION_LBA_START {
            return Err(DiskError::LbaBeforeKernelPartition { lba });
        }

        let offset = (lba - KERNEL_PARTITION_LBA_START) as usize * SECTOR_SIZE;
        let byte_count = sector_count * SECTOR_SIZE;

        if offset + byte_count > SHADOW_MAX_BYTES {
            return Err(DiskError::ShadowOverflow {
                offset,
                count: byte_count,
                max:   SHADOW_MAX_BYTES,
            });
        }

        let src_ptr = (STAGE2_DISK_SHADOW_BASE as usize + offset) as *const u8;

        // SAFETY : L'adresse shadow_base + offset est dans la zone de données pré-chargée
        // par stage2.asm. La validité de cette zone dépend du contrat avec stage2.asm.
        unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, buf.as_mut_ptr(), byte_count);
        }

        Ok(())
    }
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum DiskError {
    /// Buffer de destination trop petit.
    BufferTooSmall { needed: usize, got: usize },
    /// Nombre de secteurs demandé dépasse le maximum.
    TooManySectors { requested: usize, max: usize },
    /// LBA avant le début de la partition kernel.
    LbaBeforeKernelPartition { lba: u64 },
    /// Dépassement du disk shadow buffer.
    ShadowOverflow { offset: usize, count: usize, max: usize },
    /// Erreur hardware BIOS (code AH de INT 13h).
    HardwareError { ah_code: u8 },
}

impl core::fmt::Display for DiskError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BufferTooSmall { needed, got } =>
                write!(f, "Buffer trop petit pour lecture disque : {} < {} bytes", got, needed),
            Self::TooManySectors { requested, max } =>
                write!(f, "Trop de secteurs demandés : {} > {}", requested, max),
            Self::LbaBeforeKernelPartition { lba } =>
                write!(f, "LBA {} avant la partition kernel (début = {})", lba, KERNEL_PARTITION_LBA_START),
            Self::ShadowOverflow { offset, count, max } =>
                write!(f, "Débordement disk shadow : offset {} + {} > {} bytes", offset, count, max),
            Self::HardwareError { ah_code } =>
                write!(f, "Erreur hardware disque BIOS : AH=0x{:02X}", ah_code),
        }
    }
}
