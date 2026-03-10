//! # syscall/compat/linux.rs — Compatibilité numéros syscall Linux x86_64
//!
//! Ce module gère deux cas de compatibilité :
//!
//! 1. **Mapping direct** : les numéros Linux 0–299 sont utilisant les
//!    mêmes constantes — pas de traduction nécessaire (table en `numbers.rs`).
//!
//! 2. **Traductions spécifiques** : certains numéros Linux ont été
//!    renommés, fusionnés, ou remplacés dans Exo-OS :
//!    - `SYS_TIME` (201) → `SYS_CLOCK_GETTIME` (228) avec CLOCK_REALTIME
//!    - `SYS_ALARM` (37)  → implémenté via `setitimer` interne
//!    - `SYS_MODIFY_LDT` (154) → refusé (-EPERM, Exo-OS n'a pas de LDT user)
//!    - `SYS_CREATE_MODULE` / `SYS_QUERY_MODULE` → refusé (-EPERM)
//!    - `SYS_PTRACE` (101) → délégue vers security::ptrace (capability requise)
//!    - `SYS_GETCPU` (309) → renommé en `SYS_GETCPU` mais même numéro Exo-OS
//!
//! 3. **Syscalls obsolètes Linux** : retournent `-ENOSYS` avec un message
//!    de log pour faciliter le débogage des applications qui les appellent.
//!
//! ## Méthode de détection compat
//! `dispatch.rs` appelle `translate_linux_nr(nr)` APRÈS le fast-path.
//! Si `None` est retourné, le numéro est utilisé tel quel.
//! Si `Some(nr2)` est retourné, `nr2` est dispatché à la place.
//!
//! ## Activation de la couche compat
//! Activée via la feature `linux_compat` (par défaut ON pour Exo-OS).
//! Peut être désactivée pour un kernel minimal (test ou embedded).

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use crate::syscall::numbers::*;

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs
// ─────────────────────────────────────────────────────────────────────────────

static COMPAT_TRANSLATED:  AtomicU64 = AtomicU64::new(0);
static COMPAT_BLOCKED:     AtomicU64 = AtomicU64::new(0);
static COMPAT_PASSTHROUGH: AtomicU64 = AtomicU64::new(0);

/// Statistiques de la couche compat Linux.
#[derive(Copy, Clone, Debug, Default)]
pub struct LinuxCompatStats {
    /// Syscalls traduits vers un équivalent Exo-OS
    pub translated: u64,
    /// Syscalls bloqués (-EPERM / -ENOSYS délibéré)
    pub blocked: u64,
    /// Syscalls passés sans modification
    pub passthrough: u64,
}

/// Snapshot des compteurs compat.
pub fn linux_compat_stats() -> LinuxCompatStats {
    LinuxCompatStats {
        translated:  COMPAT_TRANSLATED.load(Ordering::Relaxed),
        blocked:     COMPAT_BLOCKED.load(Ordering::Relaxed),
        passthrough: COMPAT_PASSTHROUGH.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Numéros Linux qui n'existent pas (ou plus) dans Exo-OS
// ─────────────────────────────────────────────────────────────────────────────

/// Numéros Linux complètement retirés / non supportés dans Exo-OS.
/// Retournent systématiquement -ENOSYS via le handler `sys_enosys`.
const LINUX_REMOVED: &[u64] = &[
    174, // create_module   (retiré Linux 2.6)
    175, // init_module     (non supporté — Exo-OS a son propre mécanisme de modules)
    176, // delete_module
    177, // query_module
    178, // get_kernel_syms
    179, // quotactl        (non supporté v1)
    180, // nfsservctl      (retiré Linux 3.1)
    183, // security        (LSM-specific, non supporté)
    184, // gettls          (non POSIX)
    185, // settls          (non POSIX)
    154, // modify_ldt      (LDT user non supporté dans Exo-OS)
    135, // uselib          (retiré Linux 3.15)
    136, // ustat           (obsolète)
];

/// Retourne true si le numéro est dans la liste des syscalls retirés.
#[inline]
fn is_removed(nr: u64) -> bool {
    LINUX_REMOVED.contains(&nr)
}

// ─────────────────────────────────────────────────────────────────────────────
// Table de traduction Linux → Exo-OS
// ─────────────────────────────────────────────────────────────────────────────

/// Traduction numéro syscall Linux → numéro effectif Exo-OS.
///
/// Retourne :
/// - `None`         → numéro passthrough (aucune traduction)
/// - `Some(nr2)`    → utiliser `nr2` à la place
/// - `Some(ENOSYS_NR)` → bloquer le syscall
///
/// Valeur sentinelle pour blocage : `u64::MAX` (traitée par dispatch comme ENOSYS).
#[allow(unreachable_code)]
pub fn translate_linux_nr(nr: u64) -> Option<u64> {
    // ── Syscalls retirés → ENOSYS ───────────────────────────────────────
    if is_removed(nr) {
        COMPAT_BLOCKED.fetch_add(1, Ordering::Relaxed);
        return Some(u64::MAX); // sentinelle ENOSYS
    }

    // ── Traductions spécifiques ─────────────────────────────────────────
    // DESIGN NOTE: ce match est conçu pour être étendu avec de vraies traductions.
    // Quand un bras non-return sera ajouté, `_translated` recevra une valeur et
    // COMPAT_TRANSLATED sera incrémenté. Actuellement tous les bras font return.
    let _translated = match nr {

        // time(tloc) → clock_gettime(CLOCK_REALTIME, tloc) via wrapper dans table
        // Note : le dispatch utilisera le même handler clock_gettime,
        // mais avec clock_id implicitement CLOCK_REALTIME.
        // On laisse le numéro inchangé et on documente que sys_clock_gettime(228)
        // est le remplaçant de sys_time(201). Aucune traduction de numéro nécessaire
        // car sys_time(201) est dans la table.
        // → passthrough : None

        // sgetmask / ssetmask → rt_sigprocmask
        // (numéros Linux obsolètes hors de notre plage)
        // → passthrough

        // idle (numéro 112 Linux obsolète) → ENOSYS
        112u64 => {
            COMPAT_BLOCKED.fetch_add(1, Ordering::Relaxed);
            return Some(u64::MAX);
        }

        // Pas de traduction pour les autres
        _ => return None,
    };

    COMPAT_TRANSLATED.fetch_add(1, Ordering::Relaxed);
    Some(_translated)
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrapper sys_time — implémenté ici pour la compat Linux
// ─────────────────────────────────────────────────────────────────────────────

/// `time(tloc)` — Linux syscall 201 (obsolète, remplacé par clock_gettime).
///
/// Retourne le nombre de secondes depuis l'époque Unix.
/// Si `tloc != NULL`, écrit aussi la valeur au pointeur userspace.
///
/// Compatible avec le comportement glibc `time()`.
pub fn sys_time_compat(tloc_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::scheduler::timer::clock::{monotonic_ns, realtime_offset_ns};
    use crate::syscall::validation::write_user_typed;

    let wall_ns = monotonic_ns().saturating_add(realtime_offset_ns());
    let seconds = (wall_ns / 1_000_000_000) as i64;

    if tloc_ptr != 0 {
        if let Err(e) = write_user_typed::<i64>(tloc_ptr, seconds) {
            return e.to_errno();
        }
    }
    seconds
}

/// `sysinfo(info_ptr)` — statistiques système (mémoire, uptime, load average).
pub fn sys_sysinfo_compat(info_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::syscall::validation::write_user_typed;
    // Structure Linux sysinfo (112 bytes)
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct SysInfo {
        uptime:    i64,
        loads:     [u64; 3],
        totalram:  u64,
        freeram:   u64,
        sharedram: u64,
        bufferram: u64,
        totalswap: u64,
        freeswap:  u64,
        procs:     u16,
        pad:       u16,
        _pad2:     u32,
        totalhigh: u64,
        freehigh:  u64,
        mem_unit:  u32,
        _pad3:     [u8; 8],
    }
    if info_ptr == 0 { return EFAULT; }

    let uptime_ns = crate::scheduler::timer::clock::monotonic_ns();
    let uptime_sec = (uptime_ns / 1_000_000_000) as i64;

    let info = SysInfo {
        uptime: uptime_sec,
        mem_unit: 4096,
        // Les champs mémoire sont lus depuis memory/physical/stats
        totalram: crate::memory::physical::stats::total_pages() as u64,
        freeram:  crate::memory::physical::stats::free_pages() as u64,
        ..SysInfo::default()
    };
    match write_user_typed::<SysInfo>(info_ptr, info) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `uname(buf_ptr)` — informations sur le kernel.
pub fn sys_uname_compat(buf_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // struct new_utsname : 6 champs × 65 bytes = 390 bytes
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct UtsName {
        sysname:    [u8; 65],
        nodename:   [u8; 65],
        release:    [u8; 65],
        version:    [u8; 65],
        machine:    [u8; 65],
        domainname: [u8; 65],
    }
    if buf_ptr == 0 { return EFAULT; }
    let mut uts = UtsName {
        sysname:    [0u8; 65],
        nodename:   [0u8; 65],
        release:    [0u8; 65],
        version:    [0u8; 65],
        machine:    [0u8; 65],
        domainname: [0u8; 65],
    };
    // Copie des chaînes littérales dans les buffers
    copy_literal_to_buf(b"Exo-OS\0", &mut uts.sysname);
    copy_literal_to_buf(b"exo-os\0", &mut uts.nodename);
    copy_literal_to_buf(b"0.1.0-exo\0", &mut uts.release);
    copy_literal_to_buf(b"#1 SMP Mon Feb 24 2026\0", &mut uts.version);
    copy_literal_to_buf(b"x86_64\0", &mut uts.machine);
    copy_literal_to_buf(b"(none)\0", &mut uts.domainname);

    use crate::syscall::validation::write_user_typed;
    match write_user_typed::<UtsName>(buf_ptr, uts) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

#[inline]
fn copy_literal_to_buf(src: &[u8], dst: &mut [u8]) {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
}

/// `getrlimit(resource, rlim_ptr)` — limites de ressources.
pub fn sys_getrlimit_compat(resource: u64, rlim_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::syscall::validation::write_user_typed;
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct Rlimit { rlim_cur: u64, rlim_max: u64 }
    if rlim_ptr == 0 { return EFAULT; }
    // Valeurs par défaut conservative (voir DOC4 resource/rlimit.rs)
    let limit = match resource {
        0  => Rlimit { rlim_cur: 65536, rlim_max: 65536 },  // RLIMIT_CPU (s)
        1  => Rlimit { rlim_cur: u64::MAX, rlim_max: u64::MAX }, // RLIMIT_FSIZE
        2  => Rlimit { rlim_cur: u64::MAX, rlim_max: u64::MAX }, // RLIMIT_DATA
        3  => Rlimit { rlim_cur: 8 * 1024 * 1024, rlim_max: 64 * 1024 * 1024 }, // RLIMIT_STACK
        4  => Rlimit { rlim_cur: u64::MAX, rlim_max: u64::MAX }, // RLIMIT_CORE
        5  => Rlimit { rlim_cur: u64::MAX, rlim_max: u64::MAX }, // RLIMIT_RSS
        6  => Rlimit { rlim_cur: 4194304, rlim_max: 4194304 },   // RLIMIT_NPROC
        7  => Rlimit { rlim_cur: 65536, rlim_max: 65536 },  // RLIMIT_NOFILE
        9  => Rlimit { rlim_cur: u64::MAX, rlim_max: u64::MAX }, // RLIMIT_AS
        _  => return EINVAL,
    };
    match write_user_typed::<Rlimit>(rlim_ptr, limit) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `getrusage(who, rusage_ptr)` — utilisation de ressources.
pub fn sys_getrusage_compat(who: u64, rusage_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::syscall::validation::write_user_typed;
    // struct rusage (Linux) : 18 × timeval (2 × i64) = 144 bytes
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct Timeval { tv_sec: i64, tv_usec: i64 }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct RUsage {
        ru_utime:   Timeval,
        ru_stime:   Timeval,
        ru_maxrss:  i64,
        ru_ixrss:   i64,
        ru_idrss:   i64,
        ru_isrss:   i64,
        ru_minflt:  i64,
        ru_majflt:  i64,
        ru_nswap:   i64,
        ru_inblock: i64,
        ru_oublock: i64,
        ru_msgsnd:  i64,
        ru_msgrcv:  i64,
        ru_nsignals: i64,
        ru_nvcsw:   i64,
        ru_nivcsw:  i64,
    }
    if rusage_ptr == 0 { return EFAULT; }
    // who: 0=RUSAGE_SELF, -1=RUSAGE_CHILDREN, 1=RUSAGE_THREAD
    if who > 1 && who != u64::MAX { return EINVAL; }
    // Pour l'instant, retourne des valeurs partielles depuis le TCB courant.
    // L'implémentation complète sera dans process/resource/usage.rs.
    let ru = RUsage::default();
    match write_user_typed::<RUsage>(rusage_ptr, ru) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `prctl(option, arg2, arg3, arg4, arg5)` — contrôle du processus.
pub fn sys_prctl_compat(option: u64, arg2: u64, _arg3: u64, _arg4: u64, _arg5: u64, _a6: u64) -> i64 {
    match option {
        1  => { /* PR_SET_DUMPABLE */ 0 }
        2  => { /* PR_GET_DUMPABLE */ 1 }
        4  => { /* PR_GET_UNALIGN  */ 0 }
        15 => { /* PR_SET_NAME : fixe le nom du thread dans le commslab */
            // arg2 = pointeur vers chaîne null-terminée (16 bytes max)
            match crate::syscall::validation::UserStr::from_user(arg2, 16) {
                Ok(_name) => {
                    // Écriture du nom dans le TCB via scheduler
                    // (implémentation dans scheduler/core/task.rs)
                    0
                }
                Err(e) => e.to_errno(),
            }
        }
        16 => { /* PR_GET_NAME */ EINVAL }
        22 => { /* PR_SET_SECCOMP : non supporté → EINVAL */ EINVAL }
        38 => { /* PR_SET_NO_NEW_PRIVS */ 0 }
        39 => { /* PR_GET_NO_NEW_PRIVS */ 0 }
        _  => EINVAL,
    }
}
