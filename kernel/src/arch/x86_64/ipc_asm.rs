//! # arch/x86_64/ipc_asm.rs — Fast-path IPC Assembly Binding
//!
//! Includes `ipc/core/fastcall_asm.s` under `arch/x86_64/` so no `global_asm!`
//! resides outside the architecture layer.

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(include_str!("../../ipc/core/fastcall_asm.s"), options(att_syntax));
