/*
 * kernel/src/syscall/entry.s — Exo-OS syscall entry (SYSCALL/SYSRET, x86_64)
 *
 * This file documents the ASM entry of the syscall/ module by explicitly
 * referencing the main implementation in arch/x86_64/syscall.rs.
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * SYSCALL ENTRY ARCHITECTURE
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * The actual ASM entry is defined in:
 *   kernel/src/arch/x86_64/syscall.rs → global_asm!("syscall_entry_asm")
 *
 * Reason: Rust `global_asm!` integrates machine code directly into the kernel
 * .rlib object, without requiring a separate .s file to compile and link.
 * The entry point `syscall_entry_asm` is declared `extern "C"` and its
 * address is loaded into MSR_LSTAR by `arch::x86_64::syscall::init_syscall()`.
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * EXECUTION SEQUENCE (reference for reading)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * [Ring 3 - userspace]
 *   SYSCALL instruction
 *     → CPU saves RIP → RCX
 *     → CPU saves RFLAGS → R11
 *     → CPU masks RFLAGS via MSR_SFMASK (IF, TF, DF, AC)
 *     → CPU loads CS:RIP from MSR_LSTAR  ← "syscall_entry_asm"
 *     → CPU loads SS from MSR_STAR[47:32] + 8
 *
 * [Ring 0 - kernel mode, GS = user GS before SWAPGS]
 *   swapgs                          → GS = kernel GS
 *   mov gs:[0x08], rsp              → save user RSP
 *   mov rsp, gs:[0x00]              → load kernel RSP (RSP0 of the TSS)
 *   push rcx                        → [rsp+120] return RIP
 *   push r11                        → [rsp+112] userspace RFLAGS
 *   push rbp                        → [rsp+104]
 *   push rbx                        → [rsp+ 96]
 *   push r12..r15                   → [rsp 64..88]
 *   push gs:[0x08]                  → [rsp+ 56] userspace RSP
 *   push rsi..rax                   → [rsp 0..48]  args + syscall number
 *   mov rbx, rsp                    → frame pointer (callee-saved)
 *   and rsp, -16                    → AMD64 ABI alignment
 *   mov rdi, rbx                    → arg1 = &SyscallFrame
 *   call syscall_rust_handler       → main Rust handler
 *   mov rsp, rbx                    → restore rsp before pops
 *   mov rax, [rsp]                  → return value (frame.rax)
 *   [pop all registers...]
 *   mov rsp, gs:[0x08]              → restore userspace RSP
 *   swapgs                          → restore userspace GS
 *   sysretq                         → return to Ring 3
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * SyscallFrame LAYOUT (MUST match arch/x86_64/syscall.rs)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * Offset  Register  Usage
 * ──────  ────────  ───────────────────────────────────────────────────────
 *   0     rax       syscall number (input) / return value (output)
 *   8     r9        arg6
 *  16     r8        arg5
 *  24     r10       arg4
 *  32     rdx       arg3
 *  40     rdi       arg1
 *  48     rsi       arg2
 *  56     (rsp)     saved userspace RSP
 *  64     r15       callee-saved
 *  72     r14       callee-saved
 *  80     r13       callee-saved
 *  88     r12       callee-saved
 *  96     rbx       callee-saved
 * 104     rbp       callee-saved
 * 112     r11       userspace RFLAGS (saved by hw SYSCALL)
 * 120     rcx       userspace return RIP (saved by hw SYSCALL)
 *
 * Total size: 128 bytes (16 × 8)
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * MSR CONFIGURATION (arch/x86_64/syscall.rs → init_syscall())
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * MSR_IA32_EFER  : SCE bit enabled  (EFER_SCE = 0x0001)
 * MSR_STAR       : [47:32]=KERNEL_CS  [63:48]=USER_CS32
 * MSR_LSTAR      : address of syscall_entry_asm (64-bit mode)
 * MSR_CSTAR      : address of syscall_cstar_noop (32-bit compat, ENOSYS)
 * MSR_SFMASK     : mask IF(9) | TF(8) | DF(10) | AC(18)
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * RUST PIPELINE (called from syscall_rust_handler)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 *   syscall_rust_handler(&mut SyscallFrame)
 *     └─ syscall::dispatch::dispatch(&mut SyscallFrame)
 *           ├─ fast_path::try_fast_path()   — O(1), <150 cycles
 *           ├─ compat::linux::translate_linux_nr()
 *           ├─ table::get_handler(nr)
 *           │     └─ handler(arg1..arg6) → i64
 *           └─ post_dispatch()
 *                 └─ signal::delivery::handle_pending_signals()  [if pending]
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * KPTI / PCID (future integration)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * When KPTI is enabled, a CR3 switch will be inserted into the ASM stub
 * BEFORE accessing the kernel stack (to transition from the userspace
 * "shadow page table" to the complete kernel page table).
 *
 * KPTI sequence (unimplemented, marked for future implementation):
 *   SWAPGS
 *   mov gs:[0x08], rsp              → save user RSP
 *   mov rsp, gs:[0x30]              → trampoline RSP (shared U/K page)
 *   push rcx / push r11             → on trampoline stack
 *   mov rax, gs:[0x38]              → kernel CR3 (with PCID flush bit)
 *   mov cr3, rax                    → switch to kernel CR3
 *   mov rsp, gs:[0x00]              → actual kernel RSP
 *   [push frame, call handler, pop frame...]
 *   mov rax, gs:[0x40]              → user CR3 (non-flush PCID)
 *   mov cr3, rax                    → return to user page table
 *   SWAPGS
 *   SYSRETQ
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * SECURITY
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * SPECTRE-V2: IA32_SPEC_CTRL is read/written by arch/x86_64/spectre/
 *             to enable IBRS on entry and disable it on return.
 *             Unimplemented here; syscall_entry_asm will do it via spectre_mitigate_entry().
 *
 * SMEP: enabled in CR4 (boot) — prevents execution of userspace addresses in Ring 0.
 * SMAP: enabled in CR4 (boot) — prevents reading userspace addresses without STAC.
 *       Userspace accesses in validation.rs must wrap with STAC/CLAC.
 */

/*
 * No ASM code here: the actual implementation is in
 * arch/x86_64/syscall.rs in the form of `core::arch::global_asm!`.
 *
 * This file serves as reference documentation and is included via
 * `build.rs` only for documentation (never assembled standalone).
 */
