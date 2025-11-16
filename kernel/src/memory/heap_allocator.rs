//! Allocateur de tas
//! 
//! Le tas est géré par linked_list_allocator qui est configuré dans lib.rs.
//! Ce module fournit des fonctions utilitaires pour l'allocateur de tas.

use core::ptr::NonNull;
use core::alloc::Layout;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::println;
use crate::drivers::serial;

/// Initialise le tas avec la zone fournie par le bootloader
static USED_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static DEALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    #[cfg(not(feature = "hybrid_allocator"))]
    println!("[HEAP] Allocateur de tas initialisé (via linked_list_allocator).");
    
    #[cfg(feature = "hybrid_allocator")]
    println!("[HEAP] Allocateur de tas initialisé (mode hybrid - fallback actif).");
}

/// Vérifie l'intégrité du tas (fonction de debug)
pub fn check_heap_integrity() -> bool {
    // Intégrité basique: allocations >= désallocations et utilisations non négatives
    let used = USED_BYTES.load(Ordering::Relaxed);
    let allocs = ALLOC_COUNT.load(Ordering::Relaxed);
    let deallocs = DEALLOC_COUNT.load(Ordering::Relaxed);
    if deallocs > allocs { return false; }
    // used peut être 0 même si allocs > 0 si tout libéré
    true
}

/// Obtient des statistiques du tas
pub fn get_stats() -> HeapStats {
    HeapStats {
        used: USED_BYTES.load(Ordering::Relaxed),
        // Taille libre inconnue sans introspection interne; laisser 0 pour l'instant
        free: 0,
        allocated_blocks: ALLOC_COUNT.load(Ordering::Relaxed),
        free_blocks: DEALLOC_COUNT.load(Ordering::Relaxed),
    }
}

/// Statistiques du tas
#[derive(Debug, Clone)]
pub struct HeapStats {
    pub used: usize,
    pub free: usize,
    pub allocated_blocks: usize,
    pub free_blocks: usize,
}

#[inline(always)]
fn print_usize_dec(mut n: usize) {
    // Conversion décimale sans allocation
    let mut buf = [0u8; 32];
    let mut i = 0;
    if n == 0 {
        serial::write_char(b'0');
        return;
    }
    while n > 0 && i < buf.len() {
        let d = (n % 10) as u8;
        buf[i] = b'0' + d;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        serial::write_char(buf[i]);
    }
}

/// Allocation manuelle pour tests (contourne l'API globale)
pub fn alloc(size: usize, align: usize) -> Option<NonNull<u8>> {
    let layout = Layout::from_size_align(size, align).ok()?;
    unsafe {
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() { return None; }
        USED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        Some(NonNull::new_unchecked(ptr))
    }
}

/// Libération manuelle pour tests
pub unsafe fn dealloc(ptr: NonNull<u8>, size: usize, align: usize) {
    if size == 0 { return; }
    if let Ok(layout) = Layout::from_size_align(size, align) {
        alloc::alloc::dealloc(ptr.as_ptr(), layout);
        USED_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

/// Auto-test simple du heap appelé au boot
pub fn selftest() {
    println!("[HEAP] Selftest démarrage...");
    // 1. Allocation petite
    if let Some(p) = alloc(256, 16) {
        unsafe {
            for i in 0..256 { p.as_ptr().add(i).write(i as u8); }
            for i in 0..256 { let v = p.as_ptr().add(i).read(); if v != i as u8 { println!("[HEAP] Mismatch @{}", i); return; } }
            dealloc(p, 256, 16);
        }
    } else { println!("[HEAP] Échec alloc 256 bytes"); return; }

    // 2. Allocation plus grande
    if let Some(p) = alloc(4096, 16) {
        unsafe {
            p.as_ptr().write_bytes(0xAA, 4096);
            for i in (0..4096).step_by(512) { if p.as_ptr().add(i).read() != 0xAA { println!("[HEAP] Pattern fail"); return; } }
            dealloc(p, 4096, 16);
        }
    } else { println!("[HEAP] Échec alloc 4096 bytes"); return; }

    // 3. Intégrité
    if check_heap_integrity() { println!("[HEAP] Selftest OK"); } else { println!("[HEAP] Selftest FAIL"); }
    let stats = get_stats();
    // Impression manuelle (évite les formats complexes si UART est capricieux)
    serial::write_str("[HEAP] Stats: used=");
    print_usize_dec(stats.used);
    serial::write_str(" allocs=");
    print_usize_dec(stats.allocated_blocks);
    serial::write_str(" frees=");
    print_usize_dec(stats.free_blocks);
    serial::write_str("\n");
}