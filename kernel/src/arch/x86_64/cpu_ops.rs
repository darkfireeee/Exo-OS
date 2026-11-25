//! # arch/x86_64/cpu_ops.rs — Centralized x86_64 ISA Primitives
//!
//! Single architectural boundary for inline assembly operations needed by upper
//! kernel layers (`memory/`, `scheduler/`, `process/`, `security/`, `syscall/`,
//! `exophoenix/`, `boot_seq.rs`, `main.rs`). Upper layers must call these
//! helpers instead of embedding raw `core::arch::asm!` directly.

// ── Debug Port 0xE9 & CPU Hints ──────────────────────────────────────────────

/// Writes a single diagnostic byte to QEMU debug port `0xE9` on bare-metal x86_64.
/// No-op on host unit-test builds where `out` is Ring-0 privileged.
#[inline(always)]
pub fn debug_e9_byte(b: u8) {
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    // SAFETY: port 0xE9 is the QEMU ISA debug console; harmless on bare metal.
    unsafe {
        core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack, preserves_flags));
    }
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    let _ = b;
}

/// CPU pause hint (`pause` / REPNE NOP) for spin loops.
#[inline(always)]
pub fn cpu_pause() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: `pause` is an architectural spin-wait hint without side effects.
    unsafe {
        core::arch::asm!("pause", options(nostack, nomem, preserves_flags));
    }
    #[cfg(not(target_arch = "x86_64"))]
    core::hint::spin_loop();
}

/// Executes a single `hlt` instruction (waits for next interrupt).
///
/// # Safety
/// Must be executed in Ring 0.
#[inline(always)]
pub unsafe fn halt_once() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
}

/// Reads the current stack pointer (`RSP`).
#[inline(always)]
pub fn read_rsp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let rsp: u64;
        // SAFETY: reading RSP has no side effects.
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
        }
        rsp
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Reads the current instruction pointer (`RIP`).
#[inline(always)]
pub fn read_rip() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let rip: u64;
        // SAFETY: RIP-relative LEA is unprivileged and side-effect free.
        unsafe {
            core::arch::asm!("lea {}, [rip]", out(reg) rip, options(nostack, nomem, preserves_flags));
        }
        rip
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

// ── Interrupt Flags Save / Restore ───────────────────────────────────────────

/// Atomically reads RFLAGS and executes `cli`, returning saved RFLAGS.
/// Safe no-op (`0`) on host unit-test builds.
#[inline(always)]
pub fn save_and_cli() -> u64 {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        let rflags: u64;
        // SAFETY: Ring 0 pushfq/pop/cli sequence.
        unsafe {
            core::arch::asm!(
                "pushfq",
                "pop {flags}",
                "cli",
                flags = out(reg) rflags,
                options(nomem),
            );
        }
        rflags
    }
    #[cfg(any(not(target_arch = "x86_64"), test))]
    0
}

/// Executes `sti` if bit 9 (`IF`) was set in `rflags`.
///
/// # Safety
/// `rflags` must have been captured on the current CPU via [`save_and_cli`].
#[inline(always)]
pub unsafe fn restore_if_enabled(rflags: u64) {
    if (rflags & (1 << 9)) != 0 {
        #[cfg(all(target_arch = "x86_64", not(test)))]
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

// ── Control Registers & TLB ──────────────────────────────────────────────────

/// Reads `CR0`.
///
/// # Safety
/// Must be called in Ring 0.
#[inline(always)]
pub unsafe fn read_cr0() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nostack, nomem, preserves_flags));
        cr0
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Writes `CR0`.
///
/// # Safety
/// Must be called in Ring 0 with a valid CR0 value.
#[inline(always)]
pub unsafe fn write_cr0(val: u64) {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("mov cr0, {}", in(reg) val, options(nostack, nomem));
}

/// Executes `clts` (clears `CR0.TS`).
///
/// # Safety
/// Must be called in Ring 0.
#[inline(always)]
pub unsafe fn clts() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("clts", options(nostack, nomem));
}

/// Reloads `CR3` with its current value to flush non-global TLB entries.
///
/// # Safety
/// Must be called in Ring 0 with a valid active PML4.
#[inline(always)]
pub unsafe fn reload_cr3() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "mov {tmp}, cr3",
        "mov cr3, {tmp}",
        tmp = out(reg) _,
        options(nomem, nostack),
    );
}

/// Toggles `CR4.PGE` (bit 7) to flush all TLB entries including global pages.
///
/// # Safety
/// Must be called in Ring 0 with interrupts disabled.
#[inline(always)]
pub unsafe fn flush_tlb_global_cr4() {
    #[cfg(target_arch = "x86_64")]
    {
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        core::arch::asm!("mov cr4, {}", in(reg) cr4 & !(1u64 << 7), options(nomem, nostack));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
    }
}

// ── CPUID ────────────────────────────────────────────────────────────────────

/// Executes `CPUID` for `(leaf, subleaf)` preserving LLVM-reserved `rbx`.
#[inline(always)]
pub fn cpuid_ex(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    #[cfg(target_arch = "x86_64")]
    {
        let (eax, ecx, edx): (u32, u32, u32);
        let ebx_r: u64;
        // SAFETY: CPUID is unprivileged; xchg preserves rbx reserved by LLVM.
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") leaf => eax,
                inout("ecx") subleaf => ecx,
                out("edx") edx,
                tmp = inout(reg) 0u64 => ebx_r,
                options(nostack, nomem),
            );
        }
        (eax, ebx_r as u32, ecx, edx)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (leaf, subleaf);
        (0, 0, 0, 0)
    }
}

/// Executes `CPUID` for `(leaf, 0)`.
#[inline(always)]
pub fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    cpuid_ex(leaf, 0)
}

// ── SMAP / PKU / CET / VMX / IDTR / MONITOR-MWAIT ───────────────────────────

/// Executes `stac` (sets `RFLAGS.AC = 1`).
///
/// # Safety
/// Ring 0 with SMAP enabled.
#[inline(always)]
pub unsafe fn stac_raw() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("stac", options(nostack, nomem));
}

/// Executes `clac` (clears `RFLAGS.AC = 0`).
///
/// # Safety
/// Ring 0 with SMAP enabled.
#[inline(always)]
pub unsafe fn clac_raw() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("clac", options(nostack, nomem));
}

/// Reads the PKRU register (`rdpkru`).
///
/// # Safety
/// Requires `CR4.PKE = 1`.
#[inline(always)]
pub unsafe fn rdpkru_raw() -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        let val: u32;
        core::arch::asm!(
            "xor ecx, ecx",
            "rdpkru",
            out("eax") val,
            lateout("edx") _,
            lateout("ecx") _,
            options(nostack, nomem),
        );
        val
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Writes the PKRU register (`wrpkru`).
///
/// # Safety
/// Requires `CR4.PKE = 1`.
#[inline(always)]
pub unsafe fn wrpkru_raw(val: u32) {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "xor ecx, ecx",
        "xor edx, edx",
        "wrpkru",
        in("eax") val,
        options(nostack, nomem),
    );
}

/// Executes Intel CET `WRSSQ [ptr], val`.
///
/// # Safety
/// `ptr` must point to a shadow-stack page and CET must be enabled.
#[inline(always)]
pub unsafe fn wrssq_raw(ptr: *mut u64, val: u64) {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        ".byte 0xF3, 0x48, 0x0F, 0x01, 0x3E",
        in("rdi") ptr,
        in("rsi") val,
        options(nostack, preserves_flags),
    );
}

/// Executes Intel CET `RSTORSSP [ptr]`.
///
/// # Safety
/// `ptr` must point to a valid supervisor shadow stack restore token.
#[inline(always)]
pub unsafe fn rstorssp_raw(ptr: u64) {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        ".byte 0xF3, 0x48, 0x0F, 0x01, 0x28",
        in("rax") ptr,
        options(nostack, preserves_flags),
    );
}

/// Executes Intel CET `RDSSPQ` to read the current Shadow Stack Pointer.
///
/// # Safety
/// Requires CET shadow stack enabled.
#[inline(always)]
pub unsafe fn rdsspq_raw() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let ssp: u64;
        core::arch::asm!(
            ".byte 0xF3, 0x48, 0x0F, 0x1E, 0xC8",
            out("rax") ssp,
            options(nostack, nomem, preserves_flags),
        );
        ssp
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Reads the current IDT register (`sidt`), returning `(base, limit)`.
#[inline(always)]
pub fn sidt_raw() -> (u64, u16) {
    #[cfg(target_arch = "x86_64")]
    {
        #[repr(C, packed)]
        struct Idtr {
            limit: u16,
            base: u64,
        }
        let mut idtr = Idtr { limit: 0, base: 0 };
        // SAFETY: `sidt` writes 10 bytes to the stack-local `idtr`.
        unsafe {
            core::arch::asm!(
                "sidt [{ptr}]",
                ptr = in(reg) &mut idtr,
                options(nostack, preserves_flags),
            );
        }
        (idtr.base, idtr.limit)
    }
    #[cfg(not(target_arch = "x86_64"))]
    (0, 0)
}

/// Executes `vmxoff` in Ring 0.
///
/// # Safety
/// Must only be invoked in Ring 0 when VMX operation is active.
#[inline(always)]
pub unsafe fn vmxoff_raw() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("vmxoff", options(nostack));
}

/// Enters an architectural `MONITOR`/`MWAIT` sleep state.
///
/// # Safety
/// Caller must verify `CPUID.01H:ECX[3]` (MONITOR/MWAIT) and keep IRQs enabled.
#[inline(always)]
pub unsafe fn monitor_mwait(mon_addr: u64, hint: u32) {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "mov rax, {addr}",
        "xor ecx, ecx",
        "xor edx, edx",
        "monitor",
        "mov eax, {hint:e}",
        "xor ecx, ecx",
        "mwait",
        addr = in(reg) mon_addr,
        hint = in(reg) hint,
        out("rax") _,
        out("ecx") _,
        out("edx") _,
        options(nomem, nostack, preserves_flags),
    );
}

// ── Hardware Entropy & FPU Init ──────────────────────────────────────────────

/// Executes a single `rdrand` instruction, returning `Some(val)` if `CF=1`.
///
/// # Safety
/// Caller must check `CPUID.01H:ECX[30]` (RDRAND) beforehand.
#[inline(always)]
pub unsafe fn rdrand64_once() -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        let val: u64;
        let ok: u8;
        core::arch::asm!(
            "rdrand {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) ok,
            options(nostack, nomem),
        );
        if ok != 0 { Some(val) } else { None }
    }
    #[cfg(not(target_arch = "x86_64"))]
    None
}

/// Executes a single `rdseed` instruction, returning `Some(val)` if `CF=1`.
///
/// # Safety
/// Caller must check `CPUID.07H.0:EBX[18]` (RDSEED) beforehand.
#[inline(always)]
pub unsafe fn rdseed64_once() -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        let val: u64;
        let ok: u8;
        core::arch::asm!(
            "rdseed {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) ok,
            options(nostack, nomem),
        );
        if ok != 0 { Some(val) } else { None }
    }
    #[cfg(not(target_arch = "x86_64"))]
    None
}

/// Initializes x87/SSE control registers (`fninit` + `ldmxcsr`).
///
/// # Safety
/// Must be called with `CR0.TS = 0` and SSE enabled.
#[inline(always)]
pub unsafe fn fninit_ldmxcsr(mxcsr: u32) {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "fninit",
        "ldmxcsr [{mxcsr}]",
        mxcsr = in(reg) &mxcsr,
        options(nostack),
    );
}

// ── Atomic 128-bit (`CMPXCHG16B`) & AES-NI ──────────────────────────────────

/// Atomic 128-bit load via `lock cmpxchg16b`.
///
/// # Safety
/// `ptr` must be 16-byte aligned and valid for 16 bytes of atomic read/write.
#[cfg(target_arch = "x86_64")]
#[inline]
pub unsafe fn cmpxchg16b_load(ptr: *mut u64) -> (u64, u64) {
    let mut lo = 0u64;
    let mut hi = 0u64;
    core::arch::asm!(
        "mov r8, rbx",
        "xor ebx, ebx",
        "xor ecx, ecx",
        "lock cmpxchg16b [rdi]",
        "mov rbx, r8",
        in("rdi") ptr,
        inout("rax") lo,
        inout("rdx") hi,
        out("rcx") _,
        out("r8") _,
        options(nostack),
    );
    (lo, hi)
}

/// Atomic 128-bit store via `lock cmpxchg16b` retry loop.
///
/// # Safety
/// `ptr` must be 16-byte aligned and valid for 16 bytes of atomic read/write.
#[cfg(target_arch = "x86_64")]
#[inline]
pub unsafe fn cmpxchg16b_store(ptr: *mut u64, new_lo: u64, new_hi: u64) {
    let mut prev_lo = ptr.read_volatile();
    let mut prev_hi = ptr.add(1).read_volatile();
    loop {
        let ok: u64;
        core::arch::asm!(
            "mov r8, rbx",
            "mov rbx, rsi",
            "lock cmpxchg16b [rdi]",
            "sete r9b",
            "mov rbx, r8",
            "movzx r9d, r9b",
            in("rdi") ptr,
            in("rsi") new_lo,
            in("rcx") new_hi,
            inout("rax") prev_lo,
            inout("rdx") prev_hi,
            out("r8") _,
            lateout("r9") ok,
            options(nostack),
        );
        if ok != 0 {
            break;
        }
    }
}

/// Single-block AES-256 encryption using raw-encoded AES-NI opcodes (avoids
/// compiler SSE codegen under `-sse2` soft-float kernel target).
///
/// # Safety
/// Caller must verify AES-NI support (`CPUID.01H:ECX[25]`), valid 16-byte
/// `block`, and 15×16-byte `round_keys`.
#[cfg(target_arch = "x86_64")]
pub unsafe fn aes256_encrypt_block_ni(block: *mut u8, round_keys: *const u8) {
    core::arch::asm!(
        "sub rsp, 32",
        ".byte 0xF3, 0x0F, 0x7F, 0x04, 0x24",
        ".byte 0xF3, 0x0F, 0x7F, 0x4C, 0x24, 0x10",
        ".byte 0xF3, 0x0F, 0x6F, 0x07",
        ".byte 0xF3, 0x0F, 0x6F, 0x0E",
        ".byte 0x66, 0x0F, 0xEF, 0xC1",
        "lea rax, [rsi + 16]",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1", "add rax, 16",
        ".byte 0xF3, 0x0F, 0x6F, 0x08", ".byte 0x66, 0x0F, 0x38, 0xDD, 0xC1",
        ".byte 0xF3, 0x0F, 0x7F, 0x07",
        ".byte 0xF3, 0x0F, 0x6F, 0x04, 0x24",
        ".byte 0xF3, 0x0F, 0x6F, 0x4C, 0x24, 0x10",
        "add rsp, 32",
        in("rdi") block,
        in("rsi") round_keys,
        out("rax") _,
    );
}

// ── ExoPhoenix Hardware Primitives ───────────────────────────────────────────

// Live `.text` integrity sentinel block used by `exophoenix::golden_mirror`.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
core::arch::global_asm!(
    ".section .text",
    ".balign 64",
    ".global __exo_phoenix_sentinel_start",
    ".global exo_phoenix_integrity_sentinel",
    ".type exo_phoenix_integrity_sentinel, @function",
    "__exo_phoenix_sentinel_start:",
    "exo_phoenix_integrity_sentinel:",
    // movabs rax, 0x50484F454E495837 ("PHOENIX7") ; ret
    ".byte 0x48, 0xB8, 0x37, 0x58, 0x49, 0x4E, 0x45, 0x4F, 0x48, 0x50, 0xC3",
    ".fill 53, 1, 0x90",
    ".global __exo_phoenix_sentinel_end",
    "__exo_phoenix_sentinel_end:",
    ".size exo_phoenix_integrity_sentinel, 64",
);

/// Temporarily clears `CR0.WP` under `cli`, runs `f`, restores `CR0` (re-enforcing
/// W^X), and serializes ICache/TLB (`mfence` + `cpuid` + `CR3` reload).
///
/// # Safety
/// Must be called from Ring 0 exclusively for Golden RAM Mirror `.text` healing.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub unsafe fn with_cr0_wp_disabled<R>(f: impl FnOnce() -> R) -> R {
    let rflags = save_and_cli();
    let cr0_orig = read_cr0();
    write_cr0(cr0_orig & !(1u64 << 16));

    let res = f();

    crate::arch::x86_64::memory_barrier();
    write_cr0(cr0_orig);
    let _ = cpuid(0);
    reload_cr3();
    restore_if_enabled(rflags);
    res
}

/// Host fallback for [`with_cr0_wp_disabled`].
#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
pub unsafe fn with_cr0_wp_disabled<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// Triggers a controlled 2-byte `ud2` (`#UD`) trap for ExoPhoenix F1 qualification.
///
/// # Safety
/// Runtime recovery must be armed prior to calling.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
#[inline(never)]
pub unsafe fn phoenix_ud2_probe() -> u64 {
    let out: u64;
    core::arch::asm!(
        "xor eax, eax",
        "ud2",
        "mov {flag}, rax",
        flag = lateout(reg) out,
        out("rax") _,
        options(nostack, nomem),
    );
    out
}

/// Triggers a hardware `#DE` divide-by-zero trap for ExoPhoenix resurrection self-test.
///
/// # Safety
/// Test-only self-destruct path (`cfg(exophoenix_resurrection_test)`).
#[cfg(exophoenix_resurrection_test)]
#[inline(never)]
pub unsafe fn phoenix_div0_trap() {
    core::arch::asm!(
        "xor edx, edx",
        "xor eax, eax",
        "div eax",
        options(nostack, nomem),
    );
}
