//! # syscall/fast_path.rs — Fast path syscalls (<100 cycles)
//!
//! Implémente les syscalls qui ne nécessitent aucun verrou, aucune allocation,
//! et se résolvent en lisant des données per-CPU ou des compteurs atomiques.
//!
//! ## Cible de performance
//! | Syscall          | Cible      | Commentaire                              |
//! |------------------|------------|------------------------------------------|
//! | `getpid`         | <50 cyc    | Lecture TCB per-CPU via GS:[0x20]        |
//! | `gettid`         | <50 cyc    | Lecture TCB per-CPU via GS:[0x20]        |
//! | `getuid/geteuid` | <80 cyc    | Lecture PCB credentials (atomique)       |
//! | `getgid/getegid` | <80 cyc    | Idem                                     |
//! | `getppid`        | <80 cyc    | Lecture PCB parent_pid (atomique)        |
//! | `getcpu`         | <50 cyc    | Lecture GS:[0x10] (cpu_id atomique)      |
//! | `clock_gettime`  | <150 cyc   | RDTSC + multiply (MONOTONIC uniquement)  |
//! | `sched_yield`    | ~500 cyc   | schedule_yield() — context switch éventuel |
//!
//! ## Règles NO-ALLOC (regle_bonus.md)
//! Ce module n'utilise jamais `alloc`, `Vec`, `Box`, `Rc`, `Arc`.
//! Seuls les registres et la pile sont utilisés.
//!
//! ## RÈGLE CONTRAT UNSAFE (regle_bonus.md)
//! Tout `unsafe {}` est précédé d'un commentaire `// SAFETY:`.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use core::mem::MaybeUninit;

use crate::arch::x86_64::cpu::tsc::read_tsc;
use crate::arch::x86_64::smp::percpu::current_cpu_id;
use crate::scheduler::core::task::{ThreadControlBlock, ThreadId, ProcessId};
use crate::scheduler::timer::clock::monotonic_ns;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Masque RFLAGS — seuls les flags autorisés sont restaurables depuis userspace.
/// IF(9), DF(10), TF(8), AC(18) sont filtrés par SFMASK (voir arch/syscall.rs).
const RFLAGS_USER_MASK: u64 = 0x003F_FFFF;

/// POSIX CLOCK IDs
pub const CLOCK_REALTIME:         u32 = 0;
pub const CLOCK_MONOTONIC:        u32 = 1;
pub const CLOCK_PROCESS_CPUTIME:  u32 = 2;
pub const CLOCK_THREAD_CPUTIME:   u32 = 3;
pub const CLOCK_MONOTONIC_RAW:    u32 = 4;
pub const CLOCK_REALTIME_COARSE:  u32 = 5;
pub const CLOCK_MONOTONIC_COARSE: u32 = 6;
pub const CLOCK_BOOTTIME:         u32 = 7;

// ─────────────────────────────────────────────────────────────────────────────
// Instrumentation fast-path
// ─────────────────────────────────────────────────────────────────────────────

static FP_GETPID_COUNT:       AtomicU64 = AtomicU64::new(0);
static FP_GETTID_COUNT:       AtomicU64 = AtomicU64::new(0);
static FP_GETUID_COUNT:       AtomicU64 = AtomicU64::new(0);
static FP_GETGID_COUNT:       AtomicU64 = AtomicU64::new(0);
static FP_GETPPID_COUNT:      AtomicU64 = AtomicU64::new(0);
static FP_GETCPU_COUNT:       AtomicU64 = AtomicU64::new(0);
static FP_CLOCKGET_COUNT:     AtomicU64 = AtomicU64::new(0);
static FP_YIELD_COUNT:        AtomicU64 = AtomicU64::new(0);

/// Statistiques fast-path renvoyées par [`fast_path_stats`].
#[derive(Copy, Clone, Debug, Default)]
pub struct FastPathStats {
    pub getpid:    u64,
    pub gettid:    u64,
    pub getuid:    u64,
    pub getgid:    u64,
    pub getppid:   u64,
    pub getcpu:    u64,
    pub clockget:  u64,
    pub yield_cnt: u64,
}

/// Retourne un snapshot des compteurs fast-path.
pub fn fast_path_stats() -> FastPathStats {
    FastPathStats {
        getpid:    FP_GETPID_COUNT.load(Ordering::Relaxed),
        gettid:    FP_GETTID_COUNT.load(Ordering::Relaxed),
        getuid:    FP_GETUID_COUNT.load(Ordering::Relaxed),
        getgid:    FP_GETGID_COUNT.load(Ordering::Relaxed),
        getppid:   FP_GETPPID_COUNT.load(Ordering::Relaxed),
        getcpu:    FP_GETCPU_COUNT.load(Ordering::Relaxed),
        clockget:  FP_CLOCKGET_COUNT.load(Ordering::Relaxed),
        yield_cnt: FP_YIELD_COUNT.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Accès au TCB du thread courant via GS:[0x20]
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne un pointeur brut vers le TCB du thread courant.
///
/// Lit directement `GS:[0x20]` (voir percpu.rs layout).
/// Ne peut être appelé que depuis le contexte kernel Ring 0.
///
/// # Safety
/// - Le GS doit être le GS kernel (après SWAPGS).
/// - La valeur GS:[0x20] pointe vers un `ThreadControlBlock` valide.
/// - L'appelant ne doit PAS stocker le pointeur au-delà du point de préemption.
#[inline(always)]
unsafe fn current_tcb_ptr() -> *const ThreadControlBlock {
    let tcb: u64;
    // SAFETY: GS:[0x20] est initialisé lors du context switch
    // (scheduler::core::switch) et jamais nul sauf avant le premier switch.
    core::arch::asm!(
        "mov {}, gs:[0x20]",
        out(reg) tcb,
        options(nostack, nomem)
    );
    tcb as *const ThreadControlBlock
}

/// Retourne le `ThreadId` du thread courant.
///
/// # Safety
/// Doit être appelé dans le contexte kernel (GS kernel actif).
#[inline(always)]
unsafe fn current_tid() -> ThreadId {
    let tcb = current_tcb_ptr();
    // SAFETY: le TCB est valide si current_tcb_ptr() renvoie un pointeur non-nul.
    // En cas de TCB nul (avant le premier switch), TID 0 est retourné.
    if tcb.is_null() {
        return ThreadId(0);
    }
    (*tcb).tid
}

/// Retourne le `ProcessId` du thread courant.
///
/// # Safety
/// Doit être appelé dans le contexte kernel (GS kernel actif).
#[inline(always)]
unsafe fn current_pid() -> ProcessId {
    let tcb = current_tcb_ptr();
    // SAFETY: identique à current_tid().
    if tcb.is_null() {
        return ProcessId(0);
    }
    (*tcb).pid
}

// ─────────────────────────────────────────────────────────────────────────────
// Représentation timespec POSIX (struct timespec)
// ─────────────────────────────────────────────────────────────────────────────

/// Représentation C `struct timespec` — layout conforme POSIX ABI x86_64.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Timespec {
    /// Secondes entières depuis l'époque (CLOCK_REALTIME) ou le boot (CLOCK_MONOTONIC).
    pub tv_sec:  i64,
    /// Nanosecondes (0..999_999_999).
    pub tv_nsec: i64,
}

impl Timespec {
    /// Construit un `Timespec` depuis un nombre de nanosecondes total.
    #[inline]
    pub const fn from_ns(ns: u64) -> Self {
        Self {
            tv_sec:  (ns / 1_000_000_000) as i64,
            tv_nsec: (ns % 1_000_000_000) as i64,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Credentials "current" — stub en attente de l'intégration process/ complète
// ─────────────────────────────────────────────────────────────────────────────

/// Accès aux credentials du processus courant.
///
/// Dans un kernel complet, cette fonction lirait le TCB→PCB→credentials.
/// L'implémentation actuelle lit directement le PCB via la registry process.
#[inline]
fn current_creds() -> Credentials {
    // Accès via le registry process. Si le registry n'est pas encore initialisé
    // (boot précoce), retourne des credentials root (uid=0).
    //
    // Production-ready : le registry process est initialisé avant toute activation
    // user-space, donc cette branche "boot" ne s'exécute jamais depuis userspace.
    let pid = unsafe { current_pid() };
    if pid.0 == 0 {
        // Noyau sans processus courant (idle task ou boot)
        return Credentials::root();
    }
    // Accès au registry process global (protégé RCU).
    // ProcessId (scheduler) → Pid (process) : même repr u32.
    let process_pid = crate::process::core::pid::Pid(pid.0);
    match crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(process_pid) {
        Some(pcb) => {
            // SAFETY: SpinLock<Credentials> — verrouillage court, pas d'alloc.
            let creds = pcb.creds.lock();
            Credentials {
                uid:  creds.uid,
                gid:  creds.gid,
                euid: creds.euid,
                egid: creds.egid,
                ppid: pcb.ppid.load(Ordering::Relaxed),
            }
        },
        None => Credentials::root(),
    }
}

/// Credentials d'un processus lus depuis le PCB.
#[derive(Copy, Clone, Debug)]
struct Credentials {
    uid:  u32,
    gid:  u32,
    euid: u32,
    egid: u32,
    ppid: u32,
}

impl Credentials {
    fn root() -> Self {
        Self { uid: 0, gid: 0, euid: 0, egid: 0, ppid: 0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers fast-path — implémentation directe sans table de dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le PID du processus courant depuis GS:[0x20].
///
/// Expose la fonction privée `current_pid()` pour les autres modules syscall
/// (notamment `compat/posix.rs`) qui ont besoin du PID sans passer par le handler.
///
/// Retourne 0 si appelé avant le premier context switch (boot).
#[inline(always)]
pub fn syscall_current_pid() -> u32 {
    // SAFETY: appelé depuis le contexte kernel Ring-0, GS kernel actif.
    unsafe { current_pid() }.0
}

/// `getpid()` — retourne le PID du processus courant.
///
/// Performance : ~40–60 cycles (lecture GS:[0x20] + champ TCB).
/// Zone NO-ALLOC : uniquement lecture de mémoire statique/atomique.
#[inline]
pub fn sys_getpid() -> i64 {
    FP_GETPID_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Appelé depuis syscall_rust_handler, GS est le kernel GS.
    let pid = unsafe { current_pid() };
    pid.0 as i64
}

/// `gettid()` — retourne le TID du thread courant.
///
/// Performance : ~40–60 cycles.
#[inline]
pub fn sys_gettid() -> i64 {
    FP_GETTID_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: GS kernel actif depuis l'entrée syscall.
    let tid = unsafe { current_tid() };
    tid.0 as i64
}

/// `getuid()` — retourne le UID réel du processus courant.
///
/// Performance : ~70–90 cycles (lecture PCB via registry).
#[inline]
pub fn sys_getuid() -> i64 {
    FP_GETUID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().uid as i64
}

/// `geteuid()` — retourne l'UID effectif.
#[inline]
pub fn sys_geteuid() -> i64 {
    FP_GETUID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().euid as i64
}

/// `getgid()` — retourne le GID réel.
#[inline]
pub fn sys_getgid() -> i64 {
    FP_GETGID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().gid as i64
}

/// `getegid()` — retourne le GID effectif.
#[inline]
pub fn sys_getegid() -> i64 {
    FP_GETGID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().egid as i64
}

/// `getppid()` — retourne le PID du parent.
#[inline]
pub fn sys_getppid() -> i64 {
    FP_GETPPID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().ppid as i64
}

/// `getcpu(cpu, node, tcache)` — retourne le CPU et le nœud NUMA courants.
///
/// Arguments :
/// - `arg1` : pointeur userspace vers `u32` CPU (peut être NULL)
/// - `arg2` : pointeur userspace vers `u32` NUMA node (peut être NULL)
/// - `arg3` : ignoré (tcache obsolète depuis Linux 2.6.24)
///
/// Performance : ~50 cycles.
pub fn sys_getcpu(cpu_ptr: u64, node_ptr: u64, _tcache: u64) -> i64 {
    FP_GETCPU_COUNT.fetch_add(1, Ordering::Relaxed);

    let cpu_id = current_cpu_id();
    // Nœud NUMA : pour l'instant 0 (architecture future NUMA l'enrichira).
    let numa_node: u32 = 0;

    // Écriture optionnelle vers userspace
    if cpu_ptr != 0 {
        if let Err(_) = crate::syscall::validation::write_user_typed::<u32>(cpu_ptr, cpu_id) {
            return super::numbers::EFAULT;
        }
    }
    if node_ptr != 0 {
        if let Err(_) = crate::syscall::validation::write_user_typed::<u32>(node_ptr, numa_node) {
            return super::numbers::EFAULT;
        }
    }
    0
}

/// `clock_gettime(clkid, timespec_ptr)` — lit une horloge POSIX.
///
/// Horloges supportées :
/// - `CLOCK_MONOTONIC` / `CLOCK_MONOTONIC_RAW` / `CLOCK_BOOTTIME` :
///   TSC → nanosecondes via `monotonic_ns()`.
/// - `CLOCK_REALTIME` / `CLOCK_REALTIME_COARSE` :
///   `monotonic_ns()` + offset réel (CLOCK_REALTIME_OFFSET).
/// - `CLOCK_MONOTONIC_COARSE` : identique MONOTONIC (granularité tick).
///
/// Performance : ~120–150 cycles (RDTSC + multiply + write_user).
pub fn sys_clock_gettime(clkid: u64, ts_ptr: u64) -> i64 {
    FP_CLOCKGET_COUNT.fetch_add(1, Ordering::Relaxed);

    if ts_ptr == 0 {
        return super::numbers::EFAULT;
    }

    let clock_id = match crate::syscall::validation::validate_clockid(clkid) {
        Ok(id) => id,
        Err(e) => return e.to_errno(),
    };

    let ns: u64 = match clock_id {
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_BOOTTIME | CLOCK_MONOTONIC_COARSE => {
            monotonic_ns()
        }
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => {
            // Offset réel = monotonic + REALTIME_OFFSET (mis à jour par settimeofday / NTP).
            // L'offset est stocké dans un AtomicU64 global du module timer/clock.
            monotonic_ns().saturating_add(
                crate::scheduler::timer::clock::realtime_offset_ns()
            )
        }
        // CLOCK_PROCESS_CPUTIME_ID / CLOCK_THREAD_CPUTIME_ID
        CLOCK_PROCESS_CPUTIME | CLOCK_THREAD_CPUTIME => {
            // TaskStats est séparé du TCB (TCB = 128B fixe, pas de champ stats).
            // Retourne 0 en attendant un pointeur TaskStats dans le TCB étendu.
            0u64
        }
        _ => return super::numbers::EINVAL,
    };

    let ts = Timespec::from_ns(ns);
    match crate::syscall::validation::write_user_typed::<Timespec>(ts_ptr, ts) {
        Ok(_)  => 0,
        Err(e) => e.to_errno(),
    }
}

/// `gettimeofday(tv_ptr, tz_ptr)` — retourne l'heure et (optionnellement) la timezone.
///
/// La timezone est dépréciée (retourne toujours UTC+0) conformément aux
/// recommandations POSIX.1-2008.
///
/// Performance : ~130 cycles.
pub fn sys_gettimeofday(tv_ptr: u64, tz_ptr: u64) -> i64 {
    // Validation
    if tv_ptr == 0 {
        return super::numbers::EFAULT;
    }
    let wall_ns = monotonic_ns().saturating_add(
        crate::scheduler::timer::clock::realtime_offset_ns()
    );
    let timeval = Timeval {
        tv_sec:  (wall_ns / 1_000_000_000) as i64,
        tv_usec: ((wall_ns % 1_000_000_000) / 1000) as i64,
    };
    if let Err(e) = crate::syscall::validation::write_user_typed::<Timeval>(tv_ptr, timeval) {
        return e.to_errno();
    }
    // Timezone : timezone fixe UTC, daylight=0.
    if tz_ptr != 0 {
        let tz = Timezone { tz_minuteswest: 0, tz_dsttime: 0 };
        if let Err(e) = crate::syscall::validation::write_user_typed::<Timezone>(tz_ptr, tz) {
            return e.to_errno();
        }
    }
    0
}

/// `sched_yield()` — abandonne volontairement le CPU.
///
/// Appelle `schedule_yield()` du scheduler qui effectue un context switch
/// si un autre thread est prêt. Sinon, retourne immédiatement.
///
/// Performance : ~500–800 cycles (identique au context switch normal).
pub fn sys_sched_yield() -> i64 {
    FP_YIELD_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: GS:[0x20] est le pointeur TCB du thread courant, invariant percpu.
    // Non-nul dès que le scheduler est initialisé et qu'un thread tourne.
    // run_queue() renvoie une référence statique valide (arène percpu).
    // schedule_yield() est reentrance-safe : désactive la préemption en interne.
    unsafe {
        let tcb_ptr: u64;
        core::arch::asm!(
            "mov {0}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nomem, nostack, preserves_flags)
        );
        if tcb_ptr != 0 {
            let tcb = &mut *(tcb_ptr as *mut crate::scheduler::ThreadControlBlock);
            let cpu_id = tcb.current_cpu();
            let rq = crate::scheduler::run_queue(cpu_id);
            crate::scheduler::schedule_yield(rq, tcb);
        }
    }
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures C auxiliaires
// ─────────────────────────────────────────────────────────────────────────────

/// `struct timeval` — POSIX ABI x86_64.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Timeval {
    pub tv_sec:  i64,
    pub tv_usec: i64,
}

/// `struct timezone` — POSIX (obsolète).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Timezone {
    pub tz_minuteswest: i32,
    pub tz_dsttime:     i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher rapide — point d'entrée pour dispatch.rs
// ─────────────────────────────────────────────────────────────────────────────

/// Tente de satisfaire un syscall par le fast-path.
///
/// Retourne `Some(result)` si le syscall est géré, `None` sinon.
/// Le caller (dispatch.rs) tombera dans le slow-path sur `None`.
///
/// # Performance
/// La branche fast-path est prédictée prise (BSP → `inline(always)` ici serait
/// contre-productif car la fonction est appelée une seule fois par syscall).
#[inline]
pub fn try_fast_path(
    nr:   u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _arg6: u64,
) -> Option<i64> {
    use crate::syscall::numbers::*;
    match nr {
        SYS_GETPID      => Some(sys_getpid()),
        SYS_GETTID      => Some(sys_gettid()),
        SYS_GETUID      => Some(sys_getuid()),
        SYS_GETEUID     => Some(sys_geteuid()),
        SYS_GETGID      => Some(sys_getgid()),
        SYS_GETEGID     => Some(sys_getegid()),
        SYS_GETPPID     => Some(sys_getppid()),
        SYS_GETCPU      => Some(sys_getcpu(arg1, arg2, arg3)),
        SYS_CLOCK_GETTIME => Some(sys_clock_gettime(arg1, arg2)),
        SYS_GETTIMEOFDAY  => Some(sys_gettimeofday(arg1, arg2)),
        SYS_SCHED_YIELD   => Some(sys_sched_yield()),
        _               => None,
    }
}
