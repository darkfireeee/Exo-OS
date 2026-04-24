//! # drivers/pci_topology.rs
//!
//! Graphe de topologie PCI.
//! Source: GI-03_Drivers_IRQ_DMA.md §8
//!
//! DRV-45 : Lock d'écriture avec garantie IRQ_SAVE.
//! 0 STUB, 0 TODO.

use spin::RwLock;

use crate::arch::x86_64::irq_save;

const MAX_TOPOLOGY_ENTRIES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciBdf {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PciError {
    TopologyTableFull,
}

#[derive(Clone, Copy, Debug)]
struct TopologyEntry {
    child: PciBdf,
    parent: PciBdf,
}

struct TopologyEntries {
    slots: [Option<TopologyEntry>; MAX_TOPOLOGY_ENTRIES],
}

impl TopologyEntries {
    const fn new() -> Self {
        Self {
            slots: [None; MAX_TOPOLOGY_ENTRIES],
        }
    }
}

pub struct PciTopology {
    entries: RwLock<TopologyEntries>,
}

impl PciTopology {
    pub const fn new() -> Self {
        Self {
            entries: RwLock::new(TopologyEntries::new()),
        }
    }

    /// Enregistre un lien dans la topologie.
    /// DRV-45 : irq_save est OBLIGATOIRE avant toute prise de lock WRITE
    /// pour éviter un deadlock avec d'éventuelles interruptions.
    pub fn register(&self, child: PciBdf, parent: PciBdf) -> Result<(), PciError> {
        let _irq_guard = irq_save();
        let mut table = self.entries.write();

        if let Some(slot) = table
            .slots
            .iter_mut()
            .find(|slot| slot.map(|entry| entry.child == child).unwrap_or(false))
        {
            *slot = Some(TopologyEntry { child, parent });
            return Ok(());
        }

        let Some(slot) = table.slots.iter_mut().find(|slot| slot.is_none()) else {
            return Err(PciError::TopologyTableFull);
        };

        *slot = Some(TopologyEntry { child, parent });
        Ok(())
    }

    /// Retourne le parent d'un device PCI avec un lock READ thread-safe.
    pub fn parent_bridge(&self, child: PciBdf) -> Option<PciBdf> {
        let table = self.entries.read();
        table
            .slots
            .iter()
            .flatten()
            .find(|entry| entry.child == child)
            .map(|entry| entry.parent)
    }
}

pub static PCI_TOPOLOGY: PciTopology = PciTopology::new();

pub fn get_parent_bridge(child: PciBdf) -> Option<PciBdf> {
    PCI_TOPOLOGY.parent_bridge(child)
}

pub fn register_bridge_link(child: PciBdf, parent: PciBdf) -> Result<(), PciError> {
    PCI_TOPOLOGY.register(child, parent)
}
