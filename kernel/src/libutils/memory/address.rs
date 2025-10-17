//! Types pour les adresses mémoire
//! 
//! Ce module définit des types sûrs pour représenter les adresses virtuelles
//! et physiques, avec des conversions appropriées.

use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};

/// Taille d'une page (4KB)
pub const PAGE_SIZE: usize = 4096;

/// Représente une adresse virtuelle
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    /// Crée une nouvelle adresse virtuelle
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    /// Retourne la valeur brute de l'adresse
    pub const fn value(self) -> usize {
        self.0
    }

    /// Vérifie si l'adresse est alignée sur une page
    pub const fn is_page_aligned(self) -> bool {
        self.0 % PAGE_SIZE == 0
    }

    /// Arrondit l'adresse au début de la page
    pub const fn align_down_to_page(self) -> Self {
        Self(self.0 & !(PAGE_SIZE - 1))
    }

    /// Arrondit l'adresse au début de la page suivante
    pub const fn align_up_to_page(self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1))
    }

    /// Convertit un pointeur en adresse virtuelle
    pub fn from_ptr<T>(ptr: *const T) -> Self {
        Self(ptr as usize)
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "VirtAddr({:#x})", self.0)
    }
}

impl Add<usize> for VirtualAddress {
    type Output = Self;
    fn add(self, rhs: usize) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl AddAssign<usize> for VirtualAddress {
    fn add_assign(&mut self, rhs: usize) {
        self.0 += rhs;
    }
}

impl Sub<usize> for VirtualAddress {
    type Output = Self;
    fn sub(self, rhs: usize) -> Self::Output {
        Self(self.0 - rhs)
    }
}

impl SubAssign<usize> for VirtualAddress {
    fn sub_assign(&mut self, rhs: usize) {
        self.0 -= rhs;
    }
}

/// Représente une adresse physique
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    /// Crée une nouvelle adresse physique
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    /// Retourne la valeur brute de l'adresse
    pub const fn value(self) -> usize {
        self.0
    }

    /// Vérifie si l'adresse est alignée sur une page
    pub const fn is_page_aligned(self) -> bool {
        self.0 % PAGE_SIZE == 0
    }

    /// Arrondit l'adresse au début de la page
    pub const fn align_down_to_page(self) -> Self {
        Self(self.0 & !(PAGE_SIZE - 1))
    }

    /// Arrondit l'adresse au début de la page suivante
    pub const fn align_up_to_page(self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1))
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PhysAddr({:#x})", self.0)
    }
}

impl Add<usize> for PhysicalAddress {
    type Output = Self;
    fn add(self, rhs: usize) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl AddAssign<usize> for PhysicalAddress {
    fn add_assign(&mut self, rhs: usize) {
        self.0 += rhs;
    }
}

impl Sub<usize> for PhysicalAddress {
    type Output = Self;
    fn sub(self, rhs: usize) -> Self::Output {
        Self(self.0 - rhs)
    }
}

impl SubAssign<usize> for PhysicalAddress {
    fn sub_assign(&mut self, rhs: usize) {
        self.0 -= rhs;
    }
}