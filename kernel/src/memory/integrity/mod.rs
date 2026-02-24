// kernel/src/memory/integrity/mod.rs
//
// Module integrity — canary / guard pages / KASAN-lite.
//
// Ordre d'init :
//   1. canary::init()      — canary BSP
//   2. guard_pages::init() — pages garde stack BSP
//   3. sanitizer::init()   — KASAN activé (shadow map déjà mappée)

pub mod canary;
pub mod guard_pages;
pub mod sanitizer;

// Re-exports canary
pub use canary::{
    CanaryStats, CANARY_STATS,
    init_cpu_canary, cpu_canary, thread_canary,
    verify_thread_canary, canary_violation_handler, rotate_all_canaries,
};

// Re-exports guard pages
pub use guard_pages::{
    GUARD_PTE_TAG, GUARD_PTE_VALUE, VMALLOC_GUARD_THRESHOLD,
    GuardPageStats, GUARD_STATS,
    GuardRegionId, GuardRegionKind, GuardRegion,
    is_guard_pte, write_guard_pte, clear_guard_pte,
    register_guard_region, unregister_guard_region,
    check_guard_fault, GuardFaultResult,
    guard_page_violation_handler,
    cpu_stack_range, register_cpu_stack_guards,
};

// Re-exports sanitizer
pub use sanitizer::{
    SHADOW_ACCESSIBLE, SHADOW_REDZONE, SHADOW_FREED, SHADOW_UNINIT,
    KasanStats, KASAN_STATS,
    kasan_is_enabled, kasan_enable, kasan_disable,
    kasan_poison, kasan_unpoison, kasan_poison_redzone,
    kasan_check_access, KasanError, kasan_report,
    kasan_on_alloc, kasan_on_free,
};

/// Initialise tous les sous-systèmes d'intégrité mémoire.
///
/// # Safety : CPL 0, shadow map heap mappée.
pub unsafe fn init() {
    canary::init();
    guard_pages::init();
    sanitizer::init();
}
