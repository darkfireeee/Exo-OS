//! grant_adapter.rs — Adaptateur NetGrant V5 pour Intel e1000 (82540EM)
//!
//! Assure la conversion bidirectionnelle entre les descripteurs matériels e1000
//! (RxDesc / TxDesc) et l'ABI NetGrant V5 (NetGrantDesc / NetGrantSlot).
//! Conforme TLA+ S39/S40 : tampons pré-épinglés dans IommuNicWhitelist.

use exo_network_common::netgrant::{
    NetGrantDesc, NetGrantRegion, NetGrantSlot, NETGRANT_F_COMPLETED, NETGRANT_F_DMA_LOCKED,
    NETGRANT_F_READ, NETGRANT_F_RX, NETGRANT_F_TX, NETGRANT_F_WRITE,
    NETGRANT_MAX_SLOTS, NETGRANT_SLOT_SIZE,
};

pub struct E1000GrantAdapter {
    pub rx_region: Option<NetGrantRegion>,
    pub tx_region: Option<NetGrantRegion>,
    pub rx_slots: [NetGrantSlot; NETGRANT_MAX_SLOTS],
    pub tx_slots: [NetGrantSlot; NETGRANT_MAX_SLOTS],
}

impl E1000GrantAdapter {
    pub const fn new() -> Self {
        Self {
            rx_region: None,
            tx_region: None,
            rx_slots: [NetGrantSlot::empty(0); NETGRANT_MAX_SLOTS],
            tx_slots: [NetGrantSlot::empty(0); NETGRANT_MAX_SLOTS],
        }
    }

    /// Attache la région NetGrant de réception (RX) allouée et verrouillée IOMMU
    pub fn attach_rx_region(&mut self, region: NetGrantRegion) {
        self.rx_region = Some(region);
        let mut i = 0usize;
        while i < region.num_slots && i < NETGRANT_MAX_SLOTS {
            if let Some(phys) = region.slot_phys_addr(i) {
                let virt = region.slot_virt_addr(i).unwrap_or(0);
                self.rx_slots[i] = NetGrantSlot {
                    slot_id: i as u32,
                    generation: 1,
                    phys_addr: phys,
                    virt_addr: virt,
                    flags: NETGRANT_F_RX | NETGRANT_F_WRITE | NETGRANT_F_DMA_LOCKED,
                };
            }
            i += 1;
        }
    }

    /// Attache la région NetGrant d'émission (TX)
    pub fn attach_tx_region(&mut self, region: NetGrantRegion) {
        self.tx_region = Some(region);
        let mut i = 0usize;
        while i < region.num_slots && i < NETGRANT_MAX_SLOTS {
            if let Some(phys) = region.slot_phys_addr(i) {
                let virt = region.slot_virt_addr(i).unwrap_or(0);
                self.tx_slots[i] = NetGrantSlot {
                    slot_id: i as u32,
                    generation: 1,
                    phys_addr: phys,
                    virt_addr: virt,
                    flags: NETGRANT_F_TX | NETGRANT_F_READ | NETGRANT_F_DMA_LOCKED,
                };
            }
            i += 1;
        }
    }

    /// Obtient l'adresse physique matérielle pour un slot RX donné
    pub fn rx_slot_phys_addr(&self, slot_id: u32) -> Option<u64> {
        let idx = slot_id as usize;
        if idx < NETGRANT_MAX_SLOTS && self.rx_slots[idx].phys_addr != 0 {
            Some(self.rx_slots[idx].phys_addr)
        } else {
            None
        }
    }

    /// Convertit une complétion RX matérielle en descripteur NetGrantDesc
    pub fn complete_rx_packet(&mut self, slot_id: u32, packet_len: u16) -> Option<NetGrantDesc> {
        let idx = slot_id as usize;
        if idx >= NETGRANT_MAX_SLOTS {
            return None;
        }
        let slot = &mut self.rx_slots[idx];
        if slot.phys_addr == 0 {
            return None;
        }

        let desc = NetGrantDesc::new(
            slot_id,
            slot.generation,
            0,
            packet_len.min(NETGRANT_SLOT_SIZE as u16),
            NETGRANT_F_RX | NETGRANT_F_COMPLETED,
        );

        // Incrémente la génération anti-TOCTOU pour le prochain cycle
        slot.generation = slot.generation.wrapping_add(1);
        Some(desc)
    }

    /// Prépare une émission TX matérielle à partir d'un descripteur NetGrantDesc reçu
    pub fn prepare_tx_packet(&mut self, desc: &NetGrantDesc) -> Option<(u64, u16)> {
        if !desc.validate_bounds(NETGRANT_SLOT_SIZE) {
            return None;
        }
        let idx = desc.slot_id as usize;
        if idx >= NETGRANT_MAX_SLOTS {
            return None;
        }
        let slot = &self.tx_slots[idx];
        if slot.phys_addr == 0 || !desc.matches_generation(slot.generation) {
            return None;
        }

        let target_phys = slot.phys_addr.checked_add(desc.offset as u64)?;
        Some((target_phys, desc.len))
    }

    /// Acquitte la complétion d'un slot TX transmis par le matériel
    pub fn complete_tx_packet(&mut self, slot_id: u32) {
        let idx = slot_id as usize;
        if idx < NETGRANT_MAX_SLOTS {
            self.tx_slots[idx].generation = self.tx_slots[idx].generation.wrapping_add(1);
        }
    }
}
