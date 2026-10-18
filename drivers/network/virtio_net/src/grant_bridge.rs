//! grant_bridge.rs — Pont NetGrant V5 pour VirtIO-Net (Modern PCI)
//!
//! Assure l'adaptation entre les tampons des Virtqueues VirtIO (TX / RX)
//! et le contrat zéro-copie NetGrant V5 (NetGrantDesc / NetGrantSlot).
//! Respecte la politique de sécurité S39/S40 (IommuNicWhitelist).

use exo_network_common::netgrant::{
    NetGrantDesc, NetGrantRegion, NetGrantSlot, NETGRANT_F_COMPLETED, NETGRANT_F_DMA_LOCKED,
    NETGRANT_F_READ, NETGRANT_F_RX, NETGRANT_F_TX, NETGRANT_F_WRITE,
    NETGRANT_MAX_SLOTS, NETGRANT_SLOT_SIZE,
};

pub struct VirtioNetGrantBridge {
    pub rx_region: Option<NetGrantRegion>,
    pub tx_region: Option<NetGrantRegion>,
    pub rx_slots: [NetGrantSlot; NETGRANT_MAX_SLOTS],
    pub tx_slots: [NetGrantSlot; NETGRANT_MAX_SLOTS],
    pub hdr_size: usize,
}

impl VirtioNetGrantBridge {
    pub const fn new(hdr_size: usize) -> Self {
        Self {
            rx_region: None,
            tx_region: None,
            rx_slots: [NetGrantSlot::empty(0); NETGRANT_MAX_SLOTS],
            tx_slots: [NetGrantSlot::empty(0); NETGRANT_MAX_SLOTS],
            hdr_size,
        }
    }

    /// Attache la région NetGrant de réception (RX)
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

    /// Adresse physique de l'en-tête VirtIO pour un slot RX
    pub fn rx_slot_header_phys(&self, slot_id: u32) -> Option<u64> {
        let idx = slot_id as usize;
        if idx < NETGRANT_MAX_SLOTS && self.rx_slots[idx].phys_addr != 0 {
            Some(self.rx_slots[idx].phys_addr)
        } else {
            None
        }
    }

    /// Adresse physique du payload (après hdr_size) pour un slot RX
    pub fn rx_slot_payload_phys(&self, slot_id: u32) -> Option<u64> {
        let idx = slot_id as usize;
        if idx < NETGRANT_MAX_SLOTS && self.rx_slots[idx].phys_addr != 0 {
            Some(self.rx_slots[idx].phys_addr.saturating_add(self.hdr_size as u64))
        } else {
            None
        }
    }

    /// Finalise la réception d'un paquet VirtIO en descripteur NetGrantDesc
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
            self.hdr_size as u16,
            packet_len.min(NETGRANT_SLOT_SIZE as u16),
            NETGRANT_F_RX | NETGRANT_F_COMPLETED,
        );

        slot.generation = slot.generation.wrapping_add(1);
        Some(desc)
    }

    /// Valide et prépare un paquet TX soumis via NetGrant
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

    /// Marque la complétion d'un envoi VirtIO TX
    pub fn complete_tx_packet(&mut self, slot_id: u32) {
        let idx = slot_id as usize;
        if idx < NETGRANT_MAX_SLOTS {
            self.tx_slots[idx].generation = self.tx_slots[idx].generation.wrapping_add(1);
        }
    }
}
