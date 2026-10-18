//! netgrant.rs — ABI et contrats NetGrant ExoNet V5
//!
//! Spécification: docs/recast/EXONET_V5_NETGRANT_ABI_AND_MIGRATION.md
//! Fournit les structures zéro-copie bornées pour le partage de tampons DMA
//! entre network_server et les pilotes matériels réseau (e1000, virtio_net, loopback).


pub const NETGRANT_VERSION: u16 = 1;
pub const NETGRANT_SLOT_SIZE: usize = 2048;
pub const NETGRANT_MAX_SLOTS: usize = 256;

// Drapeaux NetGrant
pub const NETGRANT_F_READ: u32 = 1 << 0;
pub const NETGRANT_F_WRITE: u32 = 1 << 1;
pub const NETGRANT_F_DMA_LOCKED: u32 = 1 << 2;
pub const NETGRANT_F_TX: u32 = 1 << 3;
pub const NETGRANT_F_RX: u32 = 1 << 4;
pub const NETGRANT_F_COMPLETED: u32 = 1 << 5;
pub const NETGRANT_F_ERROR: u32 = 1 << 6;

/// Identifiant unique d'un NetGrant (combinaison slot_id et génération)
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct NetGrantId(pub u32);

impl NetGrantId {
    pub const INVALID: Self = Self(u32::MAX);

    #[inline(always)]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }
}

/// Descripteur de transfert NetGrant (16 à 24 octets en repr(C))
/// Échangé dans les anneaux SPSC entre network_server et les drivers réseau.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetGrantDesc {
    /// Index du slot dans la région NetGrant
    pub slot_id: u32,
    /// Numéro de génération pour prévenir les Use-After-Free / TOCTOU
    pub generation_id: u32,
    /// Décalage effectif de charge utile dans le slot
    pub offset: u16,
    /// Longueur de charge utile en octets (bornée par NETGRANT_SLOT_SIZE)
    pub len: u16,
    /// Drapeaux de statut (NETGRANT_F_*)
    pub flags: u32,
    /// Réservé pour alignement 64-bit et extensions futures
    pub _reserved: u32,
}

impl NetGrantDesc {
    #[inline]
    pub const fn new(slot_id: u32, generation_id: u32, offset: u16, len: u16, flags: u32) -> Self {
        Self {
            slot_id,
            generation_id,
            offset,
            len,
            flags,
            _reserved: 0,
        }
    }

    /// Validation anti-TOCTOU des bornes dans un slot donné
    #[inline]
    pub fn validate_bounds(&self, max_slot_size: usize) -> bool {
        let off = self.offset as usize;
        let l = self.len as usize;
        match off.checked_add(l) {
            Some(end) => end <= max_slot_size,
            None => false,
        }
    }

    #[inline]
    pub fn matches_generation(&self, expected_gen: u32) -> bool {
        self.generation_id == expected_gen
    }
}

/// Description d'un slot individuel au sein du pool
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetGrantSlot {
    pub slot_id: u32,
    pub generation: u32,
    pub phys_addr: u64,
    pub virt_addr: u64,
    pub flags: u32,
}

impl NetGrantSlot {
    pub const fn empty(slot_id: u32) -> Self {
        Self {
            slot_id,
            generation: 1,
            phys_addr: 0,
            virt_addr: 0,
            flags: 0,
        }
    }
}

/// Région mémoire de pool NetGrant enregistrée dans l'IOMMU
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetGrantRegion {
    pub phys_base: u64,
    pub virt_base: u64,
    pub total_size: usize,
    pub slot_size: usize,
    pub num_slots: usize,
}

impl NetGrantRegion {
    pub const fn new(phys_base: u64, virt_base: u64, total_size: usize, slot_size: usize) -> Self {
        let num_slots = if slot_size > 0 { total_size / slot_size } else { 0 };
        Self {
            phys_base,
            virt_base,
            total_size,
            slot_size,
            num_slots,
        }
    }

    #[inline]
    pub fn slot_phys_addr(&self, slot_id: usize) -> Option<u64> {
        if slot_id < self.num_slots {
            let offset = (slot_id as u64).checked_mul(self.slot_size as u64)?;
            self.phys_base.checked_add(offset)
        } else {
            None
        }
    }

    #[inline]
    pub fn slot_virt_addr(&self, slot_id: usize) -> Option<u64> {
        if slot_id < self.num_slots {
            let offset = (slot_id as u64).checked_mul(self.slot_size as u64)?;
            self.virt_base.checked_add(offset)
        } else {
            None
        }
    }
}
