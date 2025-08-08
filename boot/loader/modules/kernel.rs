use alloc::vec::Vec;
use crate::memory::{VirtAddr, PhysAddr};

pub struct KernelLoader {
    kernel_data: Vec<u8>,
    entry_point: VirtAddr,
}

impl KernelLoader {
    pub fn new(kernel_data: Vec<u8>) -> Self {
        Self {
            kernel_data,
            entry_point: VirtAddr::new(0),
        }
    }

    pub fn load(&mut self, phys_base: PhysAddr) -> Result<VirtAddr, &'static str> {
        // Chargement du kernel en mémoire
        // Parsing de l'ELF et mise en place des sections
        Ok(self.entry_point)
    }

    pub fn get_symbols(&self) -> Vec<KernelSymbol> {
        // Récupération de la table des symboles
        Vec::new()
    }
}

#[derive(Debug, Clone)]
pub struct KernelSymbol {
    pub name: String,
    pub address: VirtAddr,
    pub size: usize,
}
