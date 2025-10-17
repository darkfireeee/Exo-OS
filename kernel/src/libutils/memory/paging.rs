//! Wrappers autour des tables de pages
//! 
//! Ce module fournit des abstractions pour travailler avec les tables de pages
//! et la pagination mémoire.

use crate::memory::{VirtualAddress, PhysicalAddress, PAGE_SIZE};
use core::marker::PhantomData;
use core::ops::{Index, IndexMut};

/// Tailles de page supportées
pub trait PageSize: Copy + Eq + PartialEq + Ord + PartialOrd {
    /// La taille de la page en octets
    const SIZE: usize;
}

/// Page de 4KB
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Size4KiB {}

impl PageSize for Size4KiB {
    const SIZE: usize = PAGE_SIZE;
}

/// Représente une page de mémoire virtuelle
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Page<S: PageSize = Size4KiB> {
    start_address: VirtualAddress,
    size: PhantomData<S>,
}

impl<S: PageSize> Page<S> {
    /// Crée une nouvelle page à partir d'une adresse virtuelle
    pub fn containing_address(address: VirtualAddress) -> Self {
        Self {
            start_address: address.align_down_to_page(),
            size: PhantomData,
        }
    }

    /// Retourne l'adresse de début de la page
    pub fn start_address(self) -> VirtualAddress {
        self.start_address
    }

    /// Retourne l'adresse de fin de la page (non incluse)
    pub fn end_address(self) -> VirtualAddress {
        self.start_address + S::SIZE
    }

    /// Retourne la plage d'adresses couverte par cette page
    pub fn address_range(self) -> core::ops::Range<VirtualAddress> {
        core::ops::Range {
            start: self.start_address,
            end: self.end_address(),
        }
    }
}

/// Entrée dans une table de pages
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(usize);

impl PageTableEntry {
    /// Crée une nouvelle entrée de table de pages
    pub const fn new() -> Self {
        Self(0)
    }

    /// Vérifie si l'entrée est présente
    pub fn is_present(self) -> bool {
        self.0 & 0x1 != 0
    }

    /// Définit le bit de présence
    pub fn set_present(&mut self, present: bool) {
        if present {
            self.0 |= 0x1;
        } else {
            self.0 &= !0x1;
        }
    }

    /// Vérifie si l'entrée est accessible en écriture
    pub fn is_writable(self) -> bool {
        self.0 & 0x2 != 0
    }

    /// Définit le bit d'accès en écriture
    pub fn set_writable(&mut self, writable: bool) {
        if writable {
            self.0 |= 0x2;
        } else {
            self.0 &= !0x2;
        }
    }

    /// Vérifie si l'entrée est accessible depuis le mode utilisateur
    pub fn is_user_accessible(self) -> bool {
        self.0 & 0x4 != 0
    }

    /// Définit le bit d'accès utilisateur
    pub fn set_user_accessible(&mut self, user_accessible: bool) {
        if user_accessible {
            self.0 |= 0x4;
        } else {
            self.0 &= !0x4;
        }
    }

    /// Vérifie si l'entrée a été accédée
    pub fn is_accessed(self) -> bool {
        self.0 & 0x20 != 0
    }

    /// Définit le bit d'accès
    pub fn set_accessed(&mut self, accessed: bool) {
        if accessed {
            self.0 |= 0x20;
        } else {
            self.0 &= !0x20;
        }
    }

    /// Vérifie si l'entrée a été modifiée (dirty bit)
    pub fn is_dirty(self) -> bool {
        self.0 & 0x40 != 0
    }

    /// Définit le bit dirty
    pub fn set_dirty(&mut self, dirty: bool) {
        if dirty {
            self.0 |= 0x40;
        } else {
            self.0 &= !0x40;
        }
    }

    /// Vérifie si l'entrée pointe vers une énorme page (2MB/1GB)
    pub fn is_huge(self) -> bool {
        self.0 & 0x80 != 0
    }

    /// Définit le bit huge page
    pub fn set_huge(&mut self, huge: bool) {
        if huge {
            self.0 |= 0x80;
        } else {
            self.0 &= !0x80;
        }
    }

    /// Vérifie si l'entrée n'est pas exécutable (NX bit)
    pub fn is_no_execute(self) -> bool {
        self.0 & (1 << 63) != 0
    }

    /// Définit le bit NX
    pub fn set_no_execute(&mut self, no_execute: bool) {
        if no_execute {
            self.0 |= 1 << 63;
        } else {
            self.0 &= !(1 << 63);
        }
    }

    /// Retourne l'adresse physique pointée par cette entrée
    pub fn addr(self) -> PhysicalAddress {
        PhysicalAddress::new(self.0 & 0x000ffffffffff000)
    }

    /// Définit l'adresse physique pointée par cette entrée
    pub fn set_addr(&mut self, addr: PhysicalAddress) {
        self.0 = (self.0 & !0x000ffffffffff000) | addr.value();
    }

    /// Définit tous les bits de l'entrée
    pub fn set_flags(&mut self, flags: PageTableFlags) {
        self.0 = flags.bits();
    }

    /// Crée une entrée qui pointe vers une table de pages
    pub fn new_table(addr: PhysicalAddress, flags: PageTableFlags) -> Self {
        let mut entry = Self(0);
        entry.set_addr(addr);
        entry.set_flags(flags);
        entry
    }

    /// Crée une entrée qui mappe une page
    pub fn new_page(addr: PhysicalAddress, flags: PageTableFlags) -> Self {
        let mut entry = Self(0);
        entry.set_addr(addr);
        entry.set_flags(flags);
        entry
    }
}

/// Drapeaux pour les entrées de table de pages
#[derive(Clone, Copy)]
pub struct PageTableFlags(usize);

impl PageTableFlags {
    /// Crée de nouveaux drapeaux
    pub const fn new() -> Self {
        Self(0)
    }

    /// Active le bit de présence
    pub const fn present(mut self) -> Self {
        self.0 |= 0x1;
        self
    }

    /// Active le bit d'écriture
    pub const fn writable(mut self) -> Self {
        self.0 |= 0x2;
        self
    }

    /// Active le bit d'accès utilisateur
    pub const fn user_accessible(mut self) -> Self {
        self.0 |= 0x4;
        self
    }

    /// Active le bit d'accès
    pub const fn accessed(mut self) -> Self {
        self.0 |= 0x20;
        self
    }

    /// Active le bit dirty
    pub const fn dirty(mut self) -> Self {
        self.0 |= 0x40;
        self
    }

    /// Active le bit huge page
    pub const fn huge(mut self) -> Self {
        self.0 |= 0x80;
        self
    }

    /// Active le bit NX
    pub const fn no_execute(mut self) -> Self {
        self.0 |= 1 << 63;
        self
    }

    /// Retourne la valeur brute des drapeaux
    pub const fn bits(self) -> usize {
        self.0
    }
}

/// Table de pages
#[repr(align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Crée une nouvelle table de pages vide
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::new(); 512],
        }
    }

    /// Efface toutes les entrées de la table
    pub fn zero(&mut self) {
        for entry in &mut self.entries {
            *entry = PageTableEntry::new();
        }
    }

    /// Retourne une référence à l'entrée à l'index spécifié
    pub fn entry(&self, index: usize) -> Option<&PageTableEntry> {
        self.entries.get(index)
    }

    /// Retourne une référence mutable à l'entrée à l'index spécifié
    pub fn entry_mut(&mut self, index: usize) -> Option<&mut PageTableEntry> {
        self.entries.get_mut(index)
    }
}

impl Index<usize> for PageTable {
    type Output = PageTableEntry;
    fn index(&self, index: usize) -> &Self::Output {
        &self.entries[index]
    }
}

impl IndexMut<usize> for PageTable {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.entries[index]
    }
}