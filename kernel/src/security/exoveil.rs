// kernel/src/security/exoveil.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoVeil — PKS (Protection Keys for Supervisor) Domains (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoVeil implements memory isolation via PKS domains (Intel PKS, SDM Vol.3 §5.5.2).
// Each PKS domain controls access to pages marked with a protection key
// (pkey) in their PTEs. Revoking a domain is O(1), ~20 cycles, without TLB
// shootdown — the CPU checks permissions in MSR IA32_PKRS on every access.
//
// PKS DOMAINS v1.0:
//   Domain 0 (Default)    : Standard kernel code — always accessible
//   Domain 1 (Caps)       : CapToken tables — revoked if compromise detected
//   Domain 2 (Credentials): Crypto keys — revoked at boot, restored as needed
//   Domain 4 (TcbHot)     : TCB cache lines CL3-CL4 — revoked upon critical compromise
//
// RULE EXOVEIL-01: Reactive dynamic revocation DISABLED in v1.0.
//   PKS revocation is used solely:
//   a) At boot (Credentials revoked by default — ExoSeal step 0)
//   b) During HANDOFF (all domains revoked except Default)
//   c) Upon explicit decision from Kernel B (no automatic scoring)
//
// RULE EXOVEIL-02: `revoke_domain()` = O(1), ~20 cycles, ZERO TLB shootdown.
//   PKS modifies solely the MSR IA32_PKRS — zero modification of PTEs.
//
// RULE EXOVEIL-03: TCB CL3-CL4 (domain TcbHot, pkey=4) are allocated with
//   `pte |= (4u64 << 59)` to guarantee that TCB ExoShield fields are
//   in the TcbHot domain.
//
// REFERENCES:
//   Intel SDM Vol.3 §5.5.2 (PKS — Protection Keys for Supervisor Pages)
//   Intel SDM Vol.4 §2 (MSR IA32_PKRS = 0x6E1)
//   ExoShield_v1_Production.md — MODULE 4: ExoVeil
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::cpu::msr;

// ─────────────────────────────────────────────────────────────────────────────
// MSR constants
// ─────────────────────────────────────────────────────────────────────────────

/// MSR IA32_PKRS — Supervisor Protection Keys Rights Register.
/// Each 2-bit pair (for 16 pkeys) encodes: 00=RWX, 01=none, 10=RO, 11=none.
const MSR_IA32_PKRS: u32 = 0x6E1;

/// Maximum number of PKS pkeys (pkey 0..15). Intel SDM: 4 bits in PTE bits [62:59].
const MAX_PKS_KEYS: usize = 16;

/// Default PKRS value: all accessible (all domains = RWX = 0b00).
#[allow(dead_code)]
const PKRS_DEFAULT_ALL_ACCESS: u64 = 0x0000_0000_0000_0000;

/// PKRS value with all revoked: all domains = 0b11 (Access Disabled + Write Disabled).
/// In PKS, each pkey domain has 2 bits (AD=bit0, WD=bit1): 0b11 = 0xF per nibble.
const PKRS_ALL_REVOKED: u64 = 0xFFFF_FFFF_FFFF_FFFF; // each pair = 0b11 (AUDIT-SEC1)

// ─────────────────────────────────────────────────────────────────────────────
// PksDomain — Protection domains
// ─────────────────────────────────────────────────────────────────────────────

/// ExoOS PKS domains.
///
/// Each domain corresponds to a protection key (pkey) in PTEs.
/// The domain value is the pkey number (0..15).
///
/// # PKR Layout in IA32_PKRS (Intel SDM Vol 3A §4.6.2.2)
///
/// For domain `d`, bits `PKRS[2*d+1 : 2*d]` control access:
/// - Bit `2*d` : AD (Access-Disable, 1 = all data access forbidden)
/// - Bit `2*d+1` : WD (Write-Disable, 1 = write forbidden)
/// - `0b00` : Read/Write (AD=0, WD=0)
/// - `0b10` : Read-only (AD=0, WD=1)
/// - `0b11` : Access Disabled / Revoked (AD=1, WD=1)
///
/// # PTE Correspondence
///
/// Pages associated with domain `d` have PTE bits[62:59] = `d` (4 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PksDomain {
    /// Domain 0: Standard kernel code — ALWAYS accessible.
    Default = 0,
    /// Domain 1: CapToken tables — revoked if capabilities compromised.
    Caps = 1,
    /// Domain 2: Crypto keys, credentials — revoked at boot by default.
    Credentials = 2,
    /// Domain 4: TCB cache lines CL3-CL4 — revoked upon critical compromise.
    TcbHot = 4,
}

impl PksDomain {
    /// Returns pkey (domain number) as u32.
    #[inline(always)]
    pub fn pkey(self) -> u32 {
        self as u32
    }

    /// Returns bit shift in IA32_PKRS for this domain.
    /// Each domain occupies 2 bits: shift = domain * 2.
    #[inline(always)]
    pub fn shift(self) -> u32 {
        (self as u32) * 2
    }

    /// PKRS mask for this domain (2 bits).
    #[inline(always)]
    pub fn mask(self) -> u64 {
        0b11u64 << self.shift()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PKS permissions
// ─────────────────────────────────────────────────────────────────────────────

/// Access permission for a PKS domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PksPermission {
    /// Read and write authorized (PKRS value = 0b00: AD=0, WD=0).
    ReadWrite = 0b00,
    /// Read-only (PKRS value = 0b10: AD=0, WD=1, compliant with Intel SDM).
    ReadOnly = 0b10,
    /// Access fully forbidden / revoked (PKRS value = 0b11: AD=1, WD=1).
    Disabled = 0b11,
}

impl PksPermission {
    /// 2-bit value to write to PKRS.
    #[inline(always)]
    pub fn bits(self) -> u64 {
        (self as u8 as u64) & 0b11
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global ExoVeil state
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum CPUs tracked in the per-CPU PKRS shadow table (FIX-20).
const MAX_EXOVEIL_CPUS: usize = 64;

/// PTE bit shift for Intel PKS 4-bit protection key (bits [62:59]).
pub const PTE_PKEY_SHIFT: u32 = 59;
/// PTE mask for Intel PKS 4-bit protection key (bits [62:59]).
pub const PTE_PKEY_MASK: u64 = 0xFu64 << PTE_PKEY_SHIFT;

/// FIX-20: Tags a supervisor PTE with the 4-bit PKS protection key of `domain`
/// (PTE bits `[62:59]`, Intel SDM Vol. 3A §4.6.2.2).
#[inline(always)]
pub const fn pte_with_pks_domain(pte: u64, domain: PksDomain) -> u64 {
    (pte & !PTE_PKEY_MASK) | (((domain as u32 as u64) & 0xF) << PTE_PKEY_SHIFT)
}

/// Extracts the 4-bit PKS protection key stored in PTE bits `[62:59]`.
#[inline(always)]
pub const fn pte_pks_domain(pte: u64) -> u32 {
    ((pte & PTE_PKEY_MASK) >> PTE_PKEY_SHIFT) as u32
}

#[inline(always)]
fn current_cpu_slot() -> usize {
    #[cfg(target_os = "none")]
    {
        (crate::arch::x86_64::smp::percpu::try_current_cpu_id().unwrap_or(0) as usize)
            % MAX_EXOVEIL_CPUS
    }
    #[cfg(not(target_os = "none"))]
    {
        0
    }
}

/// Per-CPU software shadow of MSR IA32_PKRS (FIX-20: eliminates SMP shadow race).
/// Enables fast reads without RDMSR (~1 cycle vs ~20 cycles) while keeping each
/// logical core's PKRS state strictly isolated.
static CURRENT_PKRS_PER_CPU: [AtomicU64; MAX_EXOVEIL_CPUS] = {
    const INIT: AtomicU64 = AtomicU64::new(PKRS_ALL_REVOKED);
    [INIT; MAX_EXOVEIL_CPUS]
};

#[inline(always)]
fn load_cpu_pkrs(cpu: usize) -> u64 {
    CURRENT_PKRS_PER_CPU[cpu % MAX_EXOVEIL_CPUS].load(Ordering::Acquire)
}

#[inline(always)]
fn store_cpu_pkrs(cpu: usize, val: u64) {
    CURRENT_PKRS_PER_CPU[cpu % MAX_EXOVEIL_CPUS].store(val, Ordering::Release);
}

/// PKS supported by CPU (detected at boot by ExoSeal).
static PKS_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// ExoVeil initialized (ExoSeal step 0 completed).
static EXOVEIL_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Revocation counter per domain (instrumentation).
static REVOKE_COUNT: [AtomicU64; MAX_PKS_KEYS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; 16]
};

// ─────────────────────────────────────────────────────────────────────────────
// Low-level MSR functions
// ─────────────────────────────────────────────────────────────────────────────

/// Reads current MSR IA32_PKRS.
///
/// # Safety
/// Must be called from Ring 0. CPU must support PKS.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn rdpkrs() -> u64 {
    msr::read_msr(MSR_IA32_PKRS)
}

/// Writes MSR IA32_PKRS.
///
/// # Safety
/// Must be called from Ring 0. Value must follow PKRS layout
/// (2 bits per domain, max 16 domains).
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn wrpkrs(val: u64) {
    msr::write_msr(MSR_IA32_PKRS, val);
}

#[inline(always)]
fn apply_domain_permission(pkrs: u64, domain: PksDomain, perm: PksPermission) -> u64 {
    let mask = domain.mask();
    let perm_bits = (perm.bits() as u64) << domain.shift();
    (pkrs & !mask) | perm_bits
}

#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn write_pkrs_shadowed(new_val: u64) {
    let _irq = crate::scheduler::core::preempt::IrqGuard::new();
    let cpu = current_cpu_slot();
    // SAFETY: caller guarantees Ring 0 and a valid PKRS value.
    unsafe {
        wrpkrs(new_val);
    }
    store_cpu_pkrs(cpu, new_val);
}

/// RAII guard restoring previous PKRS value on scope exit.
///
/// This guard is designed for very short critical sections:
/// momentary access to PKS-protected data without leaving a domain
/// restored longer than necessary.
#[must_use = "conserver la garde pendant toute la durée de l'accès PKS"]
pub(crate) struct ScopedPkrsAccess {
    previous_pkrs: u64,
    active: bool,
}

impl ScopedPkrsAccess {
    #[inline(always)]
    const fn inactive() -> Self {
        Self {
            previous_pkrs: 0,
            active: false,
        }
    }
}

impl Drop for ScopedPkrsAccess {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let _irq = crate::scheduler::core::preempt::IrqGuard::new();
        let cpu = current_cpu_slot();
        // SAFETY: guard can only be active if PKS is available and
        // a valid previous PKRS value was saved.
        unsafe {
            wrpkrs(self.previous_pkrs);
        }
        store_cpu_pkrs(cpu, self.previous_pkrs);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// revoke_domain — O(1) revocation of a PKS domain
// ─────────────────────────────────────────────────────────────────────────────

/// Revokes access to a PKS domain — O(1), ~20 cycles, ZERO TLB shootdown.
///
/// Sets domain 2-bit field in IA32_PKRS to `0b11` (Access Disabled).
/// The CPU will reject all accesses to pages marked with this pkey.
///
/// # ExoShield Spec
/// "Boot revocation only (Credentials) — NO reactive dynamic revocation
///  in v1.0."
///
/// Permitted calls in v1.0 are:
/// - `exoveil_init()` at boot (Credentials revoked by default)
/// - `exoveil_revoke_all_on_handoff()` during ExoPhoenix HANDOFF
/// - Explicit `revoke_domain()` by Kernel B (external decision)
///
/// # Safety
/// - Ring 0 only
/// - PKS must be supported and initialized
/// - DO NOT call from an ISR context (WRMSR is not ISR-safe
///   on all implementations — check SDM §5.5.2 for your CPU)
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn revoke_domain(domain: PksDomain) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return; // PKS not available — silent noop
    }

    // Read-Modify-Write of MSR PKRS on current CPU (FIX-20)
    let cur = load_cpu_pkrs(current_cpu_slot());
    let new_val = apply_domain_permission(cur, domain, PksPermission::Disabled);

    // SAFETY: Ring 0, PKS supported, valid PKRS value.
    unsafe {
        write_pkrs_shadowed(new_val);
    }

    // Instrumentation
    REVOKE_COUNT[domain.pkey() as usize].fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// restore_domain — Restoration of a PKS domain
// ─────────────────────────────────────────────────────────────────────────────

/// Restores read/write access to a PKS domain.
///
/// Sets domain 2-bit field in IA32_PKRS to `0b00` (ReadWrite).
///
/// # Safety
/// - Ring 0 only
/// - Only restore domains explicitly authorized by security policy
///   (ExoSeal + Kernel B)
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn restore_domain(domain: PksDomain) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    let cur = load_cpu_pkrs(current_cpu_slot());
    let new_val = apply_domain_permission(cur, domain, PksPermission::ReadWrite);

    // SAFETY: Ring 0, PKS supported, valid PKRS value.
    unsafe {
        write_pkrs_shadowed(new_val);
    }
}

/// Restores a PKS domain with a specific permission.
///
/// # Safety
/// Same requirements as `restore_domain()`.
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn restore_domain_with_permission(domain: PksDomain, perm: PksPermission) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    let cur = load_cpu_pkrs(current_cpu_slot());
    let new_val = apply_domain_permission(cur, domain, perm);

    // SAFETY: Ring 0, PKS supported, valid PKRS value.
    unsafe {
        write_pkrs_shadowed(new_val);
    }
}

/// Momentarily restores a PKS domain and automatically restores PKRS
/// on scope exit.
///
/// # Safety
/// - Ring 0 only
/// - Protected scope must not modify IA32_PKRS itself
/// - Reserve for very short critical sections without allocation
// SAFETY: validated low-level operation — see block documentation.
pub(crate) unsafe fn scoped_domain_access(
    domain: PksDomain,
    perm: PksPermission,
) -> ScopedPkrsAccess {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return ScopedPkrsAccess::inactive();
    }

    let previous_pkrs = load_cpu_pkrs(current_cpu_slot());
    let new_val = apply_domain_permission(previous_pkrs, domain, perm);

    // SAFETY: Ring 0, PKS supported, valid PKRS value.
    unsafe {
        write_pkrs_shadowed(new_val);
    }

    ScopedPkrsAccess {
        previous_pkrs,
        active: true,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoVeil initialization (ExoSeal step 0)
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the ExoVeil PKS subsystem.
///
/// Called by ExoSeal in step 0 of boot (Kernel B, Core 0).
///
/// # Sequence
/// 1. Detect PKS support via CPUID.7.0:ECX bit 6
/// 2. Revoke ALL domains (IA32_PKRS = 0xFFFFFFFF)
/// 3. Restore Default (pkey=0) to ReadWrite
/// 4. Keep Credentials (pkey=2) as Disabled (revoked at boot)
///
/// # Safety
/// - Must be called from Ring 0, Core 0, before Kernel A starts
/// - No other CPU must be active
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn exoveil_init() {
    // Detect PKS: CPUID.07H.0H:ECX bit 6
    let (_, _, ecx, _) = crate::arch::x86_64::cpuid_ex(7, 0);
    let pks_supported = ecx & (1 << 6) != 0;

    PKS_AVAILABLE.store(pks_supported, Ordering::Release);

    if !pks_supported {
        // FIX-20: Explicitly audit degraded non-PKS hardware into ExoLedger (never silent).
        crate::security::exoledger::exo_ledger_append(
            crate::security::exoledger::ActionTag::Custom {
                tag: 0x504B_5330, // "PKS0"
                data: 0,
            },
        );
        return;
    }

    // 1. Revoke all domains at boot (default-deny) across all per-CPU shadow slots.
    wrpkrs(PKRS_ALL_REVOKED);
    for slot in &CURRENT_PKRS_PER_CPU {
        slot.store(PKRS_ALL_REVOKED, Ordering::Release);
    }

    // 2. Restore Default (pkey=0) to ReadWrite — standard kernel code
    restore_domain(PksDomain::Default);

    // 3. Caps (pkey=1) remains Disabled until security_init()
    // 4. Credentials (pkey=2) remains Disabled — revoked at boot by default
    // 5. TcbHot (pkey=4) remains Disabled until TCB allocation

    EXOVEIL_INITIALIZED.store(true, Ordering::Release);
}

/// Restores PKS domains for normal operations (step 18 SECURITY_READY).
///
/// Called in step 18 of boot, after security_init(), when system
/// transitions to operational mode.
///
/// # Safety
/// Ring 0 only. Must be called AFTER exoveil_init().
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn pks_restore_for_normal_ops() {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    // Restore operational domains:
    // - Caps: ReadWrite (capability tables must be accessible)
    // - TcbHot: ReadWrite (TCB _cold_reserve accessible by scheduler)
    // - Credentials: REMAINS Disabled (access solely via crypto_server)
    //   (will be temporarily restored by crypto operations)
    restore_domain(PksDomain::Caps);
    restore_domain(PksDomain::TcbHot);
    // Credentials remains revoked — must be explicitly restored by
    // crypto_server via restore_domain(PksDomain::Credentials)
}

// ─────────────────────────────────────────────────────────────────────────────
// HANDOFF — Revocation of all domains
// ─────────────────────────────────────────────────────────────────────────────

/// Revokes ALL PKS domains during an ExoPhoenix HANDOFF.
///
/// Called by HANDOFF handler to immediately isolate all data
//  protected by PKS. Only Default domain remains accessible.
///
/// # Safety
/// Ring 0 only. Called from a HANDOFF context (no active scheduler).
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn exoveil_revoke_all_on_handoff() {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    // Revoke all domains except Default
    revoke_domain(PksDomain::Caps);
    revoke_domain(PksDomain::Credentials);
    revoke_domain(PksDomain::TcbHot);

    // Verify that only Default is accessible
    let pkrs = rdpkrs();
    debug_assert!(
        (pkrs & PksDomain::Default.mask()) == 0, // Default = RW
        "PKRS: Default domain must be RW after handoff revocation"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Accesseurs publics
// ─────────────────────────────────────────────────────────────────────────────

/// Returns true if PKS is supported by CPU.
#[inline(always)]
pub fn pks_available() -> bool {
    PKS_AVAILABLE.load(Ordering::Acquire)
}

/// Returns true if ExoVeil has been initialized.
#[inline(always)]
pub fn exoveil_initialized() -> bool {
    EXOVEIL_INITIALIZED.load(Ordering::Acquire)
}

/// Reads current CPU's PKRS value (per-CPU software shadow, not RDMSR).
#[inline(always)]
pub fn current_pkrs() -> u64 {
    load_cpu_pkrs(current_cpu_slot())
}

/// Checks if a specific domain is revoked on the current CPU.
#[inline(always)]
pub fn is_domain_revoked(domain: PksDomain) -> bool {
    let pkrs = load_cpu_pkrs(current_cpu_slot());
    let perm_bits = (pkrs >> domain.shift()) & 0b11;
    perm_bits == PksPermission::Disabled as u64
}

/// Returns revocation count for a domain (instrumentation).
#[inline(always)]
pub fn revoke_count(domain: PksDomain) -> u64 {
    REVOKE_COUNT[domain.pkey() as usize].load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// TCB Integration — PKRS write into TCB
// ─────────────────────────────────────────────────────────────────────────────

/// Saves current CPU's PKRS value in thread TCB.
///
/// Called on context switch OUT (thread preemption).
///
/// # Safety
/// TCB must be valid. Thread is being descheduled.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn save_pkrs_to_tcb(tcb: &mut crate::scheduler::core::task::ThreadControlBlock) {
    tcb.pkrs = load_cpu_pkrs(current_cpu_slot()) as u32;
}

/// Restores PKRS value from thread TCB.
///
/// Called on context switch IN (thread resumption).
///
/// # Safety
/// TCB must be valid. Thread is about to be scheduled.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn restore_pkrs_from_tcb(tcb: &crate::scheduler::core::task::ThreadControlBlock) {
    if !PKS_AVAILABLE.load(Ordering::Acquire) {
        return;
    }
    let pkrs_val = tcb.pkrs as u64;
    // SAFETY: validated low-level operation — see block documentation.
    unsafe {
        write_pkrs_shadowed(pkrs_val);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoVeil statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of ExoVeil statistics.
#[derive(Debug, Clone, Copy)]
pub struct ExoVeilStats {
    /// PKS supported by CPU.
    pub pks_available: bool,
    /// ExoVeil initialized.
    pub initialized: bool,
    /// Current PKRS value.
    pub current_pkrs: u64,
    /// Revocations of Caps domain.
    pub caps_revokes: u64,
    /// Revocations of Credentials domain.
    pub creds_revokes: u64,
    /// Revocations of TcbHot domain.
    pub tcbhot_revokes: u64,
}

/// Returns a snapshot of ExoVeil statistics.
pub fn exoveil_stats() -> ExoVeilStats {
    ExoVeilStats {
        pks_available: PKS_AVAILABLE.load(Ordering::Relaxed),
        initialized: EXOVEIL_INITIALIZED.load(Ordering::Relaxed),
        current_pkrs: load_cpu_pkrs(current_cpu_slot()),
        caps_revokes: REVOKE_COUNT[PksDomain::Caps.pkey() as usize].load(Ordering::Relaxed),
        creds_revokes: REVOKE_COUNT[PksDomain::Credentials.pkey() as usize].load(Ordering::Relaxed),
        tcbhot_revokes: REVOKE_COUNT[PksDomain::TcbHot.pkey() as usize].load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_domain_permission, load_cpu_pkrs, pte_pks_domain, pte_with_pks_domain,
        store_cpu_pkrs, PksDomain, PksPermission,
    };

    #[test]
    fn apply_domain_permission_updates_only_target_domain() {
        let initial = 0x5555_5555_5555_5555u64;
        let updated =
            apply_domain_permission(initial, PksDomain::Credentials, PksPermission::ReadOnly);

        assert_eq!(
            (updated >> PksDomain::Credentials.shift()) & 0b11,
            PksPermission::ReadOnly.bits()
        );
        assert_eq!(
            updated & !PksDomain::Credentials.mask(),
            initial & !PksDomain::Credentials.mask()
        );
    }

    #[test]
    fn apply_domain_permission_can_restore_rw_after_revoke() {
        let revoked = apply_domain_permission(0, PksDomain::Caps, PksPermission::Disabled);
        let restored = apply_domain_permission(revoked, PksDomain::Caps, PksPermission::ReadWrite);

        assert_eq!(
            (restored >> PksDomain::Caps.shift()) & 0b11,
            PksPermission::ReadWrite.bits()
        );
        assert_eq!(
            restored & !PksDomain::Caps.mask(),
            revoked & !PksDomain::Caps.mask()
        );
    }

    #[test]
    fn var20_exoveil_per_cpu_pkrs_shadow_and_pte_pkey_tagging() {
        store_cpu_pkrs(1, 0x0000_0000_0000_000C);
        store_cpu_pkrs(2, 0x0000_0000_0000_0030);
        assert_eq!(load_cpu_pkrs(1), 0x0000_0000_0000_000C);
        assert_eq!(load_cpu_pkrs(2), 0x0000_0000_0000_0030);

        let base_pte = 0x8000_0000_0010_0003u64; // NX + Present + RW
        let tagged = pte_with_pks_domain(base_pte, PksDomain::TcbHot);
        assert_eq!(pte_pks_domain(tagged), PksDomain::TcbHot.pkey());
        assert_eq!(tagged & !super::PTE_PKEY_MASK, base_pte & !super::PTE_PKEY_MASK);
        let creds_pte = pte_with_pks_domain(tagged, PksDomain::Credentials);
        assert_eq!(pte_pks_domain(creds_pte), PksDomain::Credentials.pkey());
    }
}
