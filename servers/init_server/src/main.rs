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

mod boot_info;
mod boot_sequence;
mod dependency;
mod isolation;
mod protocol;
mod service_manager;
mod service_table;
mod sigchld_handler;
mod supervisor;
mod watchdog;

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
    pub unsafe fn syscall0(nr: u64) -> i64 {
        syscall6(nr, 0, 0, 0, 0, 0, 0)
    }

    pub const SYS_FORK: u64 = 57;
    pub const SYS_EXECVE: u64 = 59;
    pub const SYS_EXIT: u64 = 60;
    pub const SYS_WAIT4: u64 = 61;
    pub const SYS_KILL: u64 = 62;
    pub const SYS_SIGACTION: u64 = 13;
    pub const SYS_NANOSLEEP: u64 = 35;
    pub const SYS_GETPID: u64 = 39;
    pub const SYS_IPC_SEND: u64 = 300;
    pub const SYS_IPC_RECV: u64 = 301;
    pub const SYS_IPC_REGISTER: u64 = 304;

    // Flags wait4
    pub const WNOHANG: u64 = 1;
    // Flags sigaction
    pub const SA_RESTART: u64 = 0x10000000;
    pub const IPC_FLAG_TIMEOUT: u64 = 0x0001;
    pub const EAGAIN: i64 = -11;
    pub const EIO: i64 = -5;
    pub const EINVAL: i64 = -22;
    pub const ENOENT: i64 = -2;
    pub const ETIMEDOUT: i64 = -110;
}

/// Descripteur d'un service supervisé.
struct Service {
    #[allow(dead_code)]
    name: &'static str,
    bin_path: &'static [u8], // chemin u8 null-terminated vers le binaire
    pid: AtomicU32,          // PID courant (0 = non démarré)
    restart_delay_ticks: AtomicU32, // délai avant relance (backoff exponentiel)
    disabled: AtomicBool,
}

impl Service {
    const fn new(name: &'static str, bin: &'static [u8]) -> Self {
        Self {
            name,
            bin_path: bin,
            pid: AtomicU32::new(0),
            restart_delay_ticks: AtomicU32::new(1),
            disabled: AtomicBool::new(false),
        }
    }

    fn current_pid(&self) -> u32 {
        self.pid.load(Ordering::Acquire)
    }

    fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    fn set_pid(&self, pid: u32) {
        self.pid.store(pid, Ordering::Release);
        self.disabled.store(false, Ordering::Release);
        self.restart_delay_ticks.store(1, Ordering::Relaxed);
    }

    fn enable(&self) {
        self.disabled.store(false, Ordering::Release);
    }

    fn disable(&self) {
        self.disabled.store(true, Ordering::Release);
        self.pid.store(0, Ordering::Release);
        self.restart_delay_ticks.store(1, Ordering::Relaxed);
    }

    fn mark_dead(&self) {
        self.pid.store(0, Ordering::Release);
        // Délai exponentiel cappé à 32 ticks
        let d = self.restart_delay_ticks.load(Ordering::Relaxed);
        self.restart_delay_ticks
            .store(d.saturating_mul(2).min(32), Ordering::Relaxed);
    }
}

type ServiceWatchdog = watchdog::InitWatchdog<{ service_table::SERVICE_COUNT }>;

// Séquence Ring1 canonique issue des docs de création/correction.
static SERVICES: [Service; service_table::SERVICE_COUNT] = [
    Service::new("ipc_router", service_table::IPC_ROUTER_BIN),
    Service::new("memory_server", service_table::MEMORY_SERVER_BIN),
    Service::new("vfs_server", service_table::VFS_SERVER_BIN),
    Service::new("crypto_server", service_table::CRYPTO_SERVER_BIN),
    Service::new("device_server", service_table::DEVICE_SERVER_BIN),
    Service::new("virtio_drivers", service_table::VIRTIO_DRIVERS_BIN),
    Service::new("network_server", service_table::NETWORK_SERVER_BIN),
    Service::new("scheduler_server", service_table::SCHEDULER_SERVER_BIN),
    Service::new("exo_shield", service_table::EXO_SHIELD_BIN),
];

#[inline(always)]
fn halt_forever() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

/// Attends tous les fils zombies (WNOHANG) et met à jour la table de services.
unsafe fn reap_children(service_watchdog: &mut ServiceWatchdog) {
    loop {
        let mut wstatus: u32 = 0;
        let pid = syscall::syscall4(
            syscall::SYS_WAIT4,
            u64::MAX, // -1 = tout enfant
            &mut wstatus as *mut u32 as u64,
            syscall::WNOHANG,
            0,
        ) as i32;

        if pid <= 0 {
            break;
        } // plus de zombie

        // Trouver quel service a crashé et le marquer mort
        let dead_pid = pid as u32;
        if let Some(idx) = supervisor::note_child_exit(&SERVICES, dead_pid) {
            service_watchdog.observe_stop(idx);
        }
    }
}

#[inline]
fn kill_service(pid: u32, signal: u64) {
    unsafe {
        let _ = syscall::syscall2(syscall::SYS_KILL, pid as u64, signal);
    }
}

fn start_service(idx: usize, service_watchdog: &mut ServiceWatchdog) -> i64 {
    let service = &SERVICES[idx];
    if service.current_pid() != 0 {
        return service.current_pid() as i64;
    }
    service.enable();
    if !supervisor::can_start(&SERVICES, service.name) {
        return syscall::EAGAIN;
    }

    let pid = unsafe { boot_sequence::spawn_service(service.name, service.bin_path) };
    if pid == 0 {
        return syscall::EIO;
    }

    service.set_pid(pid);
    service_watchdog.observe_spawn(idx);
    if unsafe { boot_sequence::wait_for_ipc_ready(pid, dependency::ready_timeout_ms(service.name)) }
    {
        pid as i64
    } else {
        kill_service(pid, 15);
        service.mark_dead();
        service_watchdog.observe_stop(idx);
        syscall::ETIMEDOUT
    }
}

fn stop_service(idx: usize, service_watchdog: &mut ServiceWatchdog) -> i64 {
    let pid = SERVICES[idx].current_pid();
    SERVICES[idx].disable();
    if pid != 0 {
        kill_service(pid, 15);
    }
    service_watchdog.observe_stop(idx);
    0
}

fn restart_service(idx: usize, service_watchdog: &mut ServiceWatchdog) -> i64 {
    let _ = stop_service(idx, service_watchdog);
    start_service(idx, service_watchdog)
}

fn handle_control_plane(service_watchdog: &mut ServiceWatchdog) {
    let mut request = protocol::InitRequest::zeroed();
    let reply = match protocol::recv_request(&mut request) {
        Ok(false) => return,
        Err(_) => return,
        Ok(true) => match request.msg_type {
            protocol::INIT_MSG_HEARTBEAT => {
                let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
                if pid < 0 {
                    protocol::InitReply::error(pid)
                } else {
                    protocol::heartbeat_reply(
                        pid as u32,
                        service_manager::running_count(&SERVICES) as u32,
                        supervisor::running_mask(&SERVICES),
                    )
                }
            }
            protocol::INIT_MSG_STATUS => {
                match protocol::read_service_name(&request.payload).and_then(|service_name| {
                    supervisor::runtime_index_by_name(&SERVICES, service_name)
                }) {
                    Some(idx) => protocol::status_reply(
                        idx as u32,
                        SERVICES[idx].current_pid(),
                        supervisor::restart_delay_ticks(&SERVICES[idx]),
                        SERVICES[idx].current_pid() != 0,
                        dependency::is_critical(SERVICES[idx].name),
                    ),
                    None => protocol::InitReply::error(syscall::ENOENT),
                }
            }
            protocol::INIT_MSG_START => {
                match protocol::read_service_name(&request.payload).and_then(|service_name| {
                    supervisor::runtime_index_by_name(&SERVICES, service_name)
                }) {
                    Some(idx) => {
                        let rc = start_service(idx, service_watchdog);
                        if rc < 0 {
                            protocol::InitReply::error(rc)
                        } else {
                            protocol::lifecycle_reply(
                                rc as u32,
                                supervisor::running_mask(&SERVICES),
                            )
                        }
                    }
                    None => protocol::InitReply::error(syscall::ENOENT),
                }
            }
            protocol::INIT_MSG_STOP => {
                match protocol::read_service_name(&request.payload).and_then(|service_name| {
                    supervisor::runtime_index_by_name(&SERVICES, service_name)
                }) {
                    Some(idx) => {
                        let rc = stop_service(idx, service_watchdog);
                        if rc < 0 {
                            protocol::InitReply::error(rc)
                        } else {
                            protocol::lifecycle_reply(0, supervisor::running_mask(&SERVICES))
                        }
                    }
                    None => protocol::InitReply::error(syscall::ENOENT),
                }
            }
            protocol::INIT_MSG_RESTART => {
                match protocol::read_service_name(&request.payload).and_then(|service_name| {
                    supervisor::runtime_index_by_name(&SERVICES, service_name)
                }) {
                    Some(idx) => {
                        let rc = restart_service(idx, service_watchdog);
                        if rc < 0 {
                            protocol::InitReply::error(rc)
                        } else {
                            protocol::lifecycle_reply(
                                rc as u32,
                                supervisor::running_mask(&SERVICES),
                            )
                        }
                    }
                    None => protocol::InitReply::error(syscall::ENOENT),
                }
            }
            protocol::INIT_MSG_CHILD_DIED => match protocol::read_u32(&request.payload, 0) {
                Ok(pid) => {
                    let _ = protocol::read_i32(&request.payload, 4);
                    if let Some(idx) = supervisor::note_child_exit(&SERVICES, pid) {
                        if SERVICES[idx].is_disabled() {
                            SERVICES[idx].disable();
                        } else {
                            service_watchdog.observe_stop(idx);
                        }
                    }
                    protocol::lifecycle_reply(pid, supervisor::running_mask(&SERVICES))
                }
                Err(err) => protocol::InitReply::error(err),
            },
            protocol::INIT_MSG_PREPARE_ISOLATION => isolation::prepare_isolation_reply(&SERVICES),
            _ => protocol::InitReply::error(syscall::EINVAL),
        },
    };

    let _ = protocol::send_reply(request.sender_pid, &reply);
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

    let mut service_watchdog = ServiceWatchdog::new();

    protocol::register_endpoint();
    unsafe {
        sigchld_handler::install_handlers();
    }

    // ── 2. Démarrer tous les services dans l'ordre ────────────────────────
    let _ = unsafe { boot_sequence::boot_services(&SERVICES) };
    let mut idx = 0usize;
    while idx < SERVICES.len() {
        if SERVICES[idx].current_pid() != 0 {
            service_watchdog.observe_spawn(idx);
        }
        idx += 1;
    }

    // ── 3. Boucle de supervision ──────────────────────────────────────────
    loop {
        // Vérifier l'arrêt demandé
        if sigchld_handler::shutdown_requested() {
            // Envoyer SIGTERM à tous les enfants
            let mut i = 0usize;
            while i < SERVICES.len() {
                let pid = SERVICES[i].current_pid();
                if pid != 0 {
                    kill_service(pid, 15);
                }
                i += 1;
            }
            break;
        }

        // Si SIGCHLD reçu, recueillir les tombés
        if sigchld_handler::take_sigchld() {
            unsafe {
                reap_children(&mut service_watchdog);
            }
        }

        let mut check_idx = 0usize;
        while check_idx < SERVICES.len() {
            if !service_watchdog.check(check_idx, &SERVICES[check_idx]) {
                SERVICES[check_idx].mark_dead();
                service_watchdog.observe_stop(check_idx);
            }
            check_idx += 1;
        }

        // Relancer les services morts
        let mut i = 0usize;
        while i < SERVICES.len() {
            if SERVICES[i].current_pid() == 0 {
                if SERVICES[i].is_disabled() {
                    i += 1;
                    continue;
                }
                if !supervisor::can_start(&SERVICES, SERVICES[i].name) {
                    i += 1;
                    continue;
                }
                // Service mort : attendre le délai de backoff avant relance
                let delay = SERVICES[i].restart_delay_ticks.load(Ordering::Relaxed);
                if delay == 0 || delay == 1 {
                    let _ = start_service(i, &mut service_watchdog);
                } else {
                    // Décrémenter le compteur de délai
                    SERVICES[i]
                        .restart_delay_ticks
                        .fetch_sub(1, Ordering::Relaxed);
                }
            }
            i += 1;
        }

        handle_control_plane(&mut service_watchdog);
    }

    // Arrêt propre terminé : attendre les derniers zombies et halt
    unsafe {
        reap_children(&mut service_watchdog);
    }
    halt_forever();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Init ne peut pas mourir — boucle. Le kernel le relancera via SIGCHLD du parent (inexistant).
    halt_forever();
}
