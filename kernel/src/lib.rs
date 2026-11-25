//! # Exo-OS Kernel — Public Module Root & Re-exports
//!
//! `lib.rs` exposes the kernel's public module hierarchy to external crates and tests.
//! - Entry point (`_start` → `kernel_main`): `main.rs` + `arch/x86_64/boot/multiboot2_trampoline.rs`
//! - Layered boot sequence & panic/OOM handlers: `boot_seq.rs`
//!
//! ## Layer Architecture (docs/refonte)
//! ```
//! Layer 0   : memory/    — no kernel dependencies
//! Layer 1   : scheduler/ — depends on memory/
//! Layer 1.5 : process/   — depends on memory/ + scheduler/
//! Layer 2a  : security/  — depends on memory/ + scheduler/process bootstrap
//! Layer 2b  : ipc/       — depends on memory/ + scheduler/ + process/ + security/
//! Layer 3   : fs/        — depends on everything except direct ipc/
//! Cross-cutting : arch/  — can call any layer
//! ```

#![cfg_attr(all(target_os = "none", not(kani)), no_std)]
#![cfg_attr(all(target_os = "none", not(kani)), feature(alloc_error_handler))]
#![allow(binary_asm_labels)]
#![allow(unexpected_cfgs)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::manual_div_ceil)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::explicit_auto_deref)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::unnecessary_safety_doc)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::declare_interior_mutable_const)]
#![allow(clippy::mut_from_ref)]

#[cfg(all(not(target_arch = "x86_64"), not(test)))]
compile_error!("ExoOS kernel v0.2.0 supporte uniquement x86_64 comme cible de boot.");

#[cfg(all(test, target_os = "none"))]
compile_error!(
    "exo-os-kernel: `cargo test` sur cible bare-metal est non supporté. \
Utiliser une cible hôte pour les tests (ex: --target x86_64-unknown-linux-gnu), \
ou utiliser le check bare-metal via run_tests.sh/Makefile."
);

extern crate alloc;
extern crate core as rust_core;

#[cfg(all(test, not(target_os = "none")))]
extern crate std;

// ── Kernel Modules ────────────────────────────────────────────────────────────

pub mod arch;
pub mod boot_seq;
pub mod drivers;
pub mod exophoenix;
pub mod fs;
pub mod ipc;
pub mod memory;
pub mod process;
pub mod scheduler;
pub mod security;
pub mod syscall;
pub mod userspace_boot;

// ── Public Re-exports ─────────────────────────────────────────────────────────

pub use boot_seq::kernel_init;

#[cfg(target_arch = "x86_64")]
pub use arch::x86_64::{
    arch_info,
    boot::early_init::arch_boot_init,
    halt_cpu,
    memory_barrier,
    KERNEL_BASE,
    PAGE_SIZE,
};
#[cfg(target_arch = "x86_64")]
pub use arch::ArchInfo;

#[cfg(target_arch = "x86_64")]
pub use arch::x86_64::cpu::{
    features::{CpuFeatures, CPU_FEATURES},
    tsc::read_tsc,
};
