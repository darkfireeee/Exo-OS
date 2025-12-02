//! Windowed Context Switch - Ultra-Fast Implementation
//!
//! This module provides the fastest context switch possible on x86_64:
//! - Windowed approach: Only save/restore RSP + RIP (16 bytes!)
//! - Assumes callee-saved registers (RBX, RBP, R12-R15) are on stack via ABI
//! - Target: < 350 cycles (vs ~2000 cycles for Linux)
//!
//! # Safety
//! This relies on correct calling convention (System V AMD64 ABI)
//!
//! v0.5.0: Integrated ASM via global_asm! (no external .S file needed)

use crate::scheduler::thread::ThreadContext;
use core::arch::global_asm;

// ═══════════════════════════════════════════════════════════════════════════
// INLINE ASM CONTEXT SWITCH - Compiled directly into the kernel
// ═══════════════════════════════════════════════════════════════════════════

global_asm!(
    ".intel_syntax noprefix",
    "",
    "# windowed_context_switch - Ultra-fast RSP-only switch",
    "# Arguments (x86_64 System V ABI):",
    "#   rdi = *mut u64: pointer to save old RSP (can be null for first switch)",
    "#   rsi = u64: new RSP value to restore",
    ".global windowed_context_switch",
    "windowed_context_switch:",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    test rdi, rdi",
    "    jz 2f",
    "    mov [rdi], rsp",
    "2:",
    "    mov rsp, rsi",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    ret",
    "",
    "# windowed_context_switch_full - Full context with ThreadContext struct",
    "# Arguments:",
    "#   rdi = *mut ThreadContext: old context to save",
    "#   rsi = *const ThreadContext: new context to restore",
    "# ThreadContext layout: rsp(0), rip(8), cr3(16), rflags(24)",
    ".global windowed_context_switch_full",
    "windowed_context_switch_full:",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    test rdi, rdi",
    "    jz 3f",
    "    mov [rdi], rsp",
    "    lea rax, [rip + 4f]",
    "    mov [rdi + 8], rax",
    "3:",
    "    mov rsp, [rsi]",
    "    jmp QWORD PTR [rsi + 8]",
    "4:",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    ret",
    "",
    "# windowed_init_context - Initialize a new thread's stack for first switch",
    "# Arguments:",
    "#   rdi = *mut ThreadContext: context to initialize",
    "#   rsi = u64: stack_top (highest address of stack)",
    "#   rdx = u64: entry_point (function to call)",
    ".global windowed_init_context",
    "windowed_init_context:",
    "    mov rax, rsi",
    "    sub rax, 8",
    "    mov [rax], rdx",
    "    sub rax, 8",
    "    mov QWORD PTR [rax], 0",
    "    sub rax, 8",
    "    mov QWORD PTR [rax], 0",
    "    sub rax, 8",
    "    mov QWORD PTR [rax], 0",
    "    sub rax, 8",
    "    mov QWORD PTR [rax], 0",
    "    sub rax, 8",
    "    mov QWORD PTR [rax], 0",
    "    sub rax, 8",
    "    mov QWORD PTR [rax], 0",
    "    mov [rdi], rax",
    "    mov [rdi + 8], rdx",
    "    mov QWORD PTR [rdi + 16], 0",
    "    mov QWORD PTR [rdi + 24], 0x202",
    "    ret",
    "",
    ".att_syntax prefix",
);

// External assembly functions (defined above via global_asm!)
extern "C" {
    fn windowed_context_switch(old_rsp_ptr: *mut u64, new_rsp: u64);
    fn windowed_context_switch_full(old_ctx: *mut ThreadContext, new_ctx: *const ThreadContext);
    fn windowed_init_context(ctx: *mut ThreadContext, stack_top: u64, entry_point: u64);
}

/// Initialize windowed context switch subsystem
pub fn init() {
    crate::logger::info("Windowed context switch initialized (16-byte contexts)");
}

/// Perform windowed context switch between two threads
#[inline(always)]
pub unsafe fn switch(
    old_ctx: *mut ThreadContext,
    new_ctx: *const ThreadContext,
) {
    let old_rsp_ptr = if !old_ctx.is_null() {
        old_ctx as *mut u64
    } else {
        core::ptr::null_mut()
    };
    let new_rsp = (*new_ctx).rsp;
    windowed_context_switch(old_rsp_ptr, new_rsp);
}

/// Full context switch
#[inline(always)]
pub unsafe fn switch_full(
    old_ctx: *mut ThreadContext,
    new_ctx: *const ThreadContext,
) {
    windowed_context_switch_full(old_ctx, new_ctx);
}

/// Initialize a new thread's context
#[inline(always)]
pub unsafe fn init_context(
    ctx: *mut ThreadContext,
    stack_top: u64,
    entry_point: u64,
) {
    windowed_init_context(ctx, stack_top, entry_point);
}

/// Switch to a thread without saving current context
#[inline(always)]
pub unsafe fn switch_to(new_ctx: *const ThreadContext) -> ! {
    windowed_context_switch(core::ptr::null_mut(), (*new_ctx).rsp);
    core::hint::unreachable_unchecked()
}
