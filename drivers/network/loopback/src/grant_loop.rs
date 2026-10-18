//! grant_loop.rs — Support NetGrant V5 pour l'interface de boucle locale (loopback)
//!
//! Permet l'échange de paquets réseau zéro-copie par rotation de slots NetGrant
//! sans copie mémoire entre network_server et le pilote de boucle locale.

use exo_network_common::netgrant::{
    NetGrantDesc, NetGrantRegion, NetGrantSlot, NETGRANT_F_COMPLETED,
    NETGRANT_F_READ, NETGRANT_F_RX, NETGRANT_F_TX, NETGRANT_F_WRITE,
    NETGRANT_MAX_SLOTS, NETGRANT_SLOT_SIZE,
};

pub struct LoopbackGrantEngine {
    pub rx_region: Option<NetGrantRegion>,
    pub tx_region: Option<NetGrantRegion>,
    pub rx_slots: [NetGrantSlot; NETGRANT_MAX_SLOTS],
    pub tx_slots: [NetGrantSlot; NETGRANT_MAX_SLOTS],
    pub loop_count: u64,
}

impl LoopbackGrantEngine {
    pub const fn new() -> Self {
        Self {
            rx_region: None,
            tx_region: None,
            rx_slots: [NetGrantSlot::empty(0); NETGRANT_MAX_SLOTS],
            tx_slots: [NetGrantSlot::empty(0); NETGRANT_MAX_SLOTS],
            loop_count: 0,
        }
    }

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
                    flags: NETGRANT_F_RX | NETGRANT_F_WRITE,
                };
            }
            i += 1;
        }
    }

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
                    flags: NETGRANT_F_TX | NETGRANT_F_READ,
                };
            }
            i += 1;
        }
    }

    /// Réfracte un paquet émis (TX) vers un slot de réception (RX) disponible
    pub fn loop_packet(
        &mut self,
        tx_desc: &NetGrantDesc,
        target_rx_slot: u32,
    ) -> Option<NetGrantDesc> {
        if !tx_desc.validate_bounds(NETGRANT_SLOT_SIZE) {
            return None;
        }
        let tx_idx = tx_desc.slot_id as usize;
        let rx_idx = target_rx_slot as usize;
        if tx_idx >= NETGRANT_MAX_SLOTS || rx_idx >= NETGRANT_MAX_SLOTS {
            return None;
        }

        let tx_slot = &self.tx_slots[tx_idx];
        let rx_slot = &mut self.rx_slots[rx_idx];

        if tx_slot.virt_addr != 0 && rx_slot.virt_addr != 0 {
            unsafe {
                let src_ptr = (tx_slot.virt_addr as *const u8).add(tx_desc.offset as usize);
                let dst_ptr = rx_slot.virt_addr as *mut u8;
                core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, tx_desc.len as usize);

                // Inverse adresses IP source / destination si IPv4
                let buf = core::slice::from_raw_parts_mut(dst_ptr, tx_desc.len as usize);
                let _ = crate::echo::swap_ipv4_endpoints(buf);
            }
        }

        self.loop_count = self.loop_count.saturating_add(1);

        let rx_desc = NetGrantDesc::new(
            target_rx_slot,
            rx_slot.generation,
            0,
            tx_desc.len,
            NETGRANT_F_RX | NETGRANT_F_COMPLETED,
        );

        rx_slot.generation = rx_slot.generation.wrapping_add(1);
        Some(rx_desc)
    }
}
