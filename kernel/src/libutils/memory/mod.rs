//! Abstractions mémoire de bas niveau
//! 
//! Ce module fournit des types et des structures pour gérer la mémoire virtuelle
//! et physique dans le noyau.

pub mod paging;
pub mod address;

// Réexportations
pub use address::{VirtualAddress, PhysicalAddress};
pub use paging::{Page, PageSize, PageTable, PageTableEntry};