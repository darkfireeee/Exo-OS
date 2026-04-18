#![no_std]
#![no_main]

//! # init_server — PID 1, Superviseur de services
//!
//! Rôle : processus racine de l'espace utilisateur.
//!   1. Démarre ipc_router (PID 2) en premier.
//!   2. Démarre ensuite la chaîne Ring1 canonique complète.
//!   3. Respecte l'ordre SRV-01/SRV-02/SRV-04 jusqu'à exo_shield en dernier.
//!   4. Supervise tous les services : relance automatique si crash (SIGCHLD).
//!   5. Gère l'arrêt propre du système (SIGTERM → arrêt ordonné).
//!
//! ## Invariants importants
//!   - Ne meurt jamais (boucle infinie de supervision).
//!   - SIGKILL/SIGSTOP sont non-masquables (SIG-07 respecté par le kernel).
//!   - Un service crashé est relancé avec un délai exponentiel (max 32s).

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

mod boot_sequence;
mod boot_info;
mod dependency;

mod syscall {
    #[inline(always)]
    pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
        let ret: i64;
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a1, in("rsi") a2, in("rdx") a3,
            in("r10") a4, in("r8")  a5, in("r9")  a6,
            lateout("rax") ret,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
        syscall6(nr, a1, a2, a3, a4, 0, 0)
    }
    #[inline(always)]
    pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
        syscall6(nr, a1, a2, a3, 0, 0, 0)
    }
    #[inline(always)]
    pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
        syscall6(nr, a1, a2, 0, 0, 0, 0)
    }
    #[inline(always)]
    pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
        syscall6(nr, a1, 0, 0, 0, 0, 0)
    }
    #[inline(always)]
    pub unsafe fn syscall0(nr: u64) -> i64 { syscall6(nr, 0, 0, 0, 0, 0, 0) }

    pub const SYS_FORK:       u64 =  57;
    pub const SYS_EXECVE:     u64 =  59;
    pub const SYS_EXIT:       u64 =  60;
    pub const SYS_WAIT4:      u64 =  61;
    pub const SYS_KILL:       u64 =  62;
    pub const SYS_SIGACTION:  u64 =  13;
    pub const SYS_NANOSLEEP:  u64 =  35;

    // Flags wait4
    pub const WNOHANG: u64 = 1;
    // Flags sigaction
    pub const SA_RESTART: u64 = 0x10000000;
}

/// Descripteur d'un service supervisé.
struct Service {
    #[allow(dead_code)]
    name:      &'static str,
    bin_path:  &'static [u8],  // chemin u8 null-terminated vers le binaire
    pid:       AtomicU32,      // PID courant (0 = non démarré)
    restart_delay_ticks: AtomicU32,  // délai avant relance (backoff exponentiel)
}

impl Service {
    const fn new(name: &'static str, bin: &'static [u8]) -> Self {
        Self {
            name,
            bin_path: bin,
            pid:      AtomicU32::new(0),
            restart_delay_ticks: AtomicU32::new(1),
        }
    }

    fn current_pid(&self) -> u32 { self.pid.load(Ordering::Acquire) }

    fn set_pid(&self, pid: u32) {
        self.pid.store(pid, Ordering::Release);
        self.restart_delay_ticks.store(1, Ordering::Relaxed);
    }

    fn mark_dead(&self) {
        self.pid.store(0, Ordering::Release);
        // Délai exponentiel cappé à 32 ticks
        let d = self.restart_delay_ticks.load(Ordering::Relaxed);
        self.restart_delay_ticks.store(d.saturating_mul(2).min(32), Ordering::Relaxed);
    }
}

// ── Table des services supervisés ────────────────────────────────────────────
// Note: null-terminated pour passation à execve via copy_userspace_argv
static IPC_ROUTER_BIN:      &[u8] = b"/sbin/exo-ipc-router\0";
static MEMORY_SERVER_BIN:   &[u8] = b"/sbin/exo-memory-server\0";
static VFS_SERVER_BIN:      &[u8] = b"/sbin/exo-vfs-server\0";
static CRYPTO_SERVER_BIN:   &[u8] = b"/sbin/exo-crypto-server\0";
static DEVICE_SERVER_BIN:   &[u8] = b"/sbin/exo-device-server\0";
static NETWORK_SERVER_BIN:  &[u8] = b"/sbin/exo-network-server\0";
static SCHEDULER_SERVER_BIN:&[u8] = b"/sbin/exo-scheduler-server\0";
static VIRTIO_DRIVERS_BIN:  &[u8] = b"/sbin/exo-virtio-drivers\0";
static EXO_SHIELD_BIN:      &[u8] = b"/sbin/exo-shield\0";

// Séquence Ring1 V4 canonique — SRV-01, SRV-02, SRV-04
static SERVICES: [Service; 9] = [
    Service::new("ipc_router",       IPC_ROUTER_BIN),      // SRV-01 : PREMIER
    Service::new("memory_server",    MEMORY_SERVER_BIN),   // SRV-02 : avant tout alloc
    Service::new("vfs_server",       VFS_SERVER_BIN),
    Service::new("crypto_server",    CRYPTO_SERVER_BIN),
    Service::new("device_server",    DEVICE_SERVER_BIN),
    Service::new("network_server",   NETWORK_SERVER_BIN),
    Service::new("scheduler_server", SCHEDULER_SERVER_BIN),
    Service::new("virtio_drivers",   VIRTIO_DRIVERS_BIN),  // virtio-block/net/console
    Service::new("exo_shield",       EXO_SHIELD_BIN),      // SRV-04 : DERNIER
];
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn halt_forever() -> ! {
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trampoline SIGCHLD
// ─────────────────────────────────────────────────────────────────────────────

/// Handler SIGCHLD : marque qu'au moins un fils est mort.
/// La détection précise se fait dans la boucle via wait4(WNOHANG).
static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigchld_handler(_sig: i32) {
    SIGCHLD_RECEIVED.store(true, Ordering::Release);
}

extern "C" fn sigterm_handler(_sig: i32) {
    SHUTDOWN.store(true, Ordering::Release);
}

/// Attends tous les fils zombies (WNOHANG) et met à jour la table de services.
unsafe fn reap_children() {
    loop {
        let mut wstatus: u32 = 0;
        let pid = syscall::syscall4(
            syscall::SYS_WAIT4,
            u64::MAX, // -1 = tout enfant
            &mut wstatus as *mut u32 as u64,
            syscall::WNOHANG,
            0,
        ) as i32;

        if pid <= 0 { break; } // plus de zombie

        // Trouver quel service a crashé et le marquer mort
        let dead_pid = pid as u32;
        let mut i = 0usize;
        while i < SERVICES.len() {
            if SERVICES[i].current_pid() == dead_pid {
                SERVICES[i].mark_dead();
                break;
            }
            i += 1;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Point d'entrée
// ─────────────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start(boot_info_virt: usize) -> ! {
    let boot_info = unsafe { boot_info::BootInfo::from_virt(boot_info_virt) };
    if let Some(info) = boot_info {
        if !info.validate() {
            halt_forever();
        }
    }

    // ── 1. Installer les handlers de signaux ──────────────────────────────
    // Structure sigaction : handler(u64) + flags(u64) + mask(u64)[2]
    // (layout simplifié — doit correspondre au kernel SigactionEntry)
    #[repr(C)]
    struct Sigaction {
        handler: u64,
        flags:   u64,
        mask:    [u64; 2],
    }
    let chld_sa = Sigaction {
        handler: sigchld_handler as *const () as u64,
        flags:   syscall::SA_RESTART,
        mask:    [0; 2],
    };
    let term_sa = Sigaction {
        handler: sigterm_handler as *const () as u64,
        flags:   syscall::SA_RESTART,
        mask:    [0; 2],
    };
    unsafe {
        // SIGCHLD = 17, SIGTERM = 15
        syscall::syscall3(
            syscall::SYS_SIGACTION,
            17,
            &chld_sa as *const Sigaction as u64,
            0,
        );
        syscall::syscall3(
            syscall::SYS_SIGACTION,
            15,
            &term_sa as *const Sigaction as u64,
            0,
        );
    }

    // ── 2. Démarrer tous les services dans l'ordre ────────────────────────
    let _ = unsafe { boot_sequence::boot_services(&SERVICES) };

    // ── 3. Boucle de supervision ──────────────────────────────────────────
    loop {
        // Vérifier l'arrêt demandé
        if SHUTDOWN.load(Ordering::Acquire) {
            // Envoyer SIGTERM à tous les enfants
            let mut i = 0usize;
            while i < SERVICES.len() {
                let pid = SERVICES[i].current_pid();
                if pid != 0 {
                    unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, 15); }
                }
                i += 1;
            }
            break;
        }

        // Si SIGCHLD reçu, recueillir les tombés
        if SIGCHLD_RECEIVED.swap(false, Ordering::AcqRel) {
            unsafe { reap_children(); }
        }

        // Relancer les services morts
        let mut i = 0usize;
        while i < SERVICES.len() {
            if SERVICES[i].current_pid() == 0 {
                // Service mort : attendre le délai de backoff avant relance
                let delay = SERVICES[i].restart_delay_ticks.load(Ordering::Relaxed);
                if delay == 0 || delay == 1 {
                    let pid = unsafe {
                        boot_sequence::spawn_service(SERVICES[i].name, SERVICES[i].bin_path)
                    };
                    if pid != 0 {
                        SERVICES[i].set_pid(pid);
                        let _ = unsafe {
                            boot_sequence::wait_for_ipc_ready(
                                pid,
                                dependency::ready_timeout_ms(SERVICES[i].name),
                            )
                        };
                    }
                } else {
                    // Décrémenter le compteur de délai
                    SERVICES[i].restart_delay_ticks.fetch_sub(1, Ordering::Relaxed);
                }
            }
            i += 1;
        }

        // Attendre un signal ou un timeout (~10ms avec nanosleep)
        #[repr(C)]
        struct Timespec { tv_sec: i64, tv_nsec: i64 }
        let ts = Timespec { tv_sec: 0, tv_nsec: 10_000_000 }; // 10ms
        unsafe {
            syscall::syscall2(
                syscall::SYS_NANOSLEEP,
                &ts as *const Timespec as u64,
                0,
            );
        }
    }

    // Arrêt propre terminé : attendre les derniers zombies et halt
    unsafe { reap_children(); }
    halt_forever();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Init ne peut pas mourir — boucle. Le kernel le relancera via SIGCHLD du parent (inexistant).
    halt_forever();
}
