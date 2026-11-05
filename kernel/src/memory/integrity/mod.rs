// kernel/src/memory/integrity/mod.rs
//
// Integrity module — canary / guard pages / KASAN-lite.
//
// Init order:
//   1. canary::init()      — BSP canary
//   2. guard_pages::init() — BSP stack guard pages
//   3. sanitizer::init()   — KASAN enabled (shadow map already mapped)

pub mod canary;
pub mod guard_pages;
pub mod sanitizer;

// Canary re-exports
pub use canary::{
    canary_violation_handler, cpu_canary, init_cpu_canary, rotate_all_canaries, thread_canary,
    verify_thread_canary, CanaryStats, CANARY_STATS,
};

// Guard pages re-exports
pub use guard_pages::{
    check_guard_fault, clear_guard_pte, cpu_stack_range, guard_page_violation_handler,
    is_guard_pte, register_cpu_stack_guards, register_guard_region, unregister_guard_region,
    write_guard_pte, GuardFaultResult, GuardPageStats, GuardRegion, GuardRegionId, GuardRegionKind,
    GUARD_PTE_TAG, GUARD_PTE_VALUE, GUARD_STATS, VMALLOC_GUARD_THRESHOLD,
};

// Sanitizer re-exports
pub use sanitizer::{
    kasan_check_access, kasan_disable, kasan_enable, kasan_is_enabled, kasan_on_alloc,
    kasan_on_free, kasan_poison, kasan_poison_redzone, kasan_report, kasan_unpoison, KasanError,
    KasanStats, KASAN_STATS, SHADOW_ACCESSIBLE, SHADOW_FREED, SHADOW_REDZONE, SHADOW_UNINIT,
};

/// Initializes all memory integrity subsystems.
///
/// # Safety
/// CPL 0, heap shadow map mapped.
pub unsafe fn init() {
    canary::init();
    guard_pages::init();
    sanitizer::init();
}
