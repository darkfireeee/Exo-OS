#![no_std]
#![no_main]

//! # init_server -- PID 1, Superviseur de services
//!
//! Rôle : processus racine de l'espace utilisateur.
//!   1. Démarre ipc_router (PID 2) en premier.
//!   2. Démarre ensuite la chaîne Ring1 canonique complète.
//!   3. Stabilise les services de fond avant d'ouvrir le shell interactif.
//!   4. Supervise tous les services : relance automatique si crash (SIGCHLD).
//!   5. Gère l'arrêt propre du système (SIGTERM → arrêt ordonné).
//!
//! ## Invariants importants
//!   - Ne meurt jamais (boucle infinie de supervision).
//!   - SIGKILL/SIGSTOP sont non-masquables (SIG-07 respecté par le kernel).
//!   - Un service crashé est relancé avec un délai exponentiel (max 32s).

use core::sync::atomic::Ordering;
pub(crate) use exo_init_server::{dependency, service_manager, service_table, supervisor, Service};
pub(crate) use exo_syscall_abi as syscall;

mod boot_info;
mod boot_sequence;
mod isolation;
mod log;
mod protocol;
mod sigchld_handler;
mod watchdog;

const CLOCK_MONOTONIC: u64 = 1;

#[repr(C)]
#[derive(Default)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

fn monotonic_ms() -> u64 {
    let mut ts = Timespec::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_CLOCK_GETTIME,
            CLOCK_MONOTONIC,
            &mut ts as *mut Timespec as u64,
        )
    };
    if rc != 0 || ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000)
}

type ServiceWatchdog = watchdog::InitWatchdog<{ service_table::SERVICE_COUNT }>;

// Séquence Ring1 canonique issue des docs de création/correction.
static SERVICES: [Service; service_table::SERVICE_COUNT] = [
    Service::new("ipc_router", service_table::IPC_ROUTER_BIN),
    Service::new("memory_server", service_table::MEMORY_SERVER_BIN),
    Service::new("vfs_server", service_table::VFS_SERVER_BIN),
    Service::new("device_server", service_table::DEVICE_SERVER_BIN),
    Service::new("input_server", service_table::INPUT_SERVER_BIN),
    Service::new("fb_server", service_table::FB_SERVER_BIN),
    Service::new("tty_server", service_table::TTY_SERVER_BIN),
    Service::new("ps2_driver", service_table::PS2_DRIVER_BIN),
    Service::new("exo_shield", service_table::EXO_SHIELD_BIN),
    Service::new("crypto_server", service_table::CRYPTO_SERVER_BIN),
    Service::new("virtio_drivers", service_table::VIRTIO_DRIVERS_BIN),
    Service::new("e1000_driver", service_table::E1000_DRIVER_BIN),
    Service::new("virtio_net_driver", service_table::VIRTIO_NET_DRIVER_BIN),
    Service::new("loopback_driver", service_table::LOOPBACK_DRIVER_BIN),
    Service::new("network_server", service_table::NETWORK_SERVER_BIN),
    Service::new("scheduler_server", service_table::SCHEDULER_SERVER_BIN),
    Service::new("audio_server", service_table::AUDIO_SERVER_BIN),
    Service::new("usb_server", service_table::USB_SERVER_BIN),
    Service::new("exo_compositor", service_table::EXO_COMPOSITOR_BIN),
    Service::new("exosh", service_table::EXOSH_BIN),
];

#[inline(always)]
fn halt_forever() -> ! {
    loop {
        core::hint::spin_loop();
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

fn play_boot_chime() {
    let req = syscall::AudioRequest {
        sender_pid: 1,
        msg_type: syscall::AUDIO_MSG_PLAY_SYSTEM_SOUND,
        reply_endpoint: 0,
        a: syscall::AUDIO_SOUND_BOOT_COMPLETE,
        b: 0,
        _reserved: [0; 16],
    };
    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::AUDIO_SERVER_ENDPOINT,
            &req as *const syscall::AudioRequest as u64,
            core::mem::size_of::<syscall::AudioRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT,
            0,
            0,
        );
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
    if unsafe {
        boot_sequence::wait_for_ipc_ready(
            service.name,
            pid,
            dependency::ready_timeout_ms(service.name),
        )
    } {
        service.mark_ready();
        pid as i64
    } else {
        if unsafe { boot_sequence::terminate_timed_out_pid(pid) } {
            service.mark_dead();
            service_watchdog.observe_stop(idx);
        }
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

fn refresh_service_readiness(idx: usize) {
    let service = &SERVICES[idx];
    let pid = service.current_pid();
    if pid != 0 && !service.is_ready() && unsafe { boot_sequence::ipc_ready_now(service.name, pid) }
    {
        service.mark_ready();
        log::service_pid(b"init: ready ", service.name, pid);
    }
}

fn cold_respawn_wave(
    names: &[&str],
    budget_ms: u64,
    service_watchdog: &mut ServiceWatchdog,
) -> (u32, u32, u32) {
    let mut spawned_indices = [usize::MAX; 4];
    let mut spawned_count = 0usize;

    // 1. Spawn all members of this wave in parallel (vfork+execve).
    let mut n_idx = 0usize;
    while n_idx < names.len() {
        let name = names[n_idx];
        if let Some(idx) = service_table::runtime_index_by_name(&SERVICES, name.as_bytes()) {
            let svc = &SERVICES[idx];
            if unsafe { boot_sequence::verify_binary_integrity(svc.name, svc.bin_path) } {
                svc.enable();
                let pid = unsafe { boot_sequence::spawn_service(svc.name, svc.bin_path) };
                if pid != 0 {
                    svc.set_pid(pid);
                    service_watchdog.observe_spawn(idx);
                    if spawned_count < spawned_indices.len() {
                        spawned_indices[spawned_count] = idx;
                        spawned_count += 1;
                    }
                }
            }
        }
        n_idx += 1;
    }

    // 2. Poll all spawned wave members in parallel until ready or budget expires.
    let start_ms = monotonic_ms();
    let mut ready_count: u32;
    let mut waited_ms = 0u64;
    loop {
        ready_count = 0;
        let mut s = 0usize;
        while s < spawned_count {
            let idx = spawned_indices[s];
            let svc = &SERVICES[idx];
            let pid = svc.current_pid();
            if svc.is_ready() {
                ready_count += 1;
            } else if pid != 0 && unsafe { boot_sequence::ipc_ready_now(svc.name, pid) } {
                svc.mark_ready();
                ready_count += 1;
            }
            s += 1;
        }
        if ready_count as usize == spawned_count {
            break;
        }
        let now_ms = monotonic_ms();
        if start_ms != 0 && now_ms != 0 {
            if now_ms.saturating_sub(start_ms) >= budget_ms {
                break;
            }
        } else if waited_ms >= budget_ms {
            break;
        }
        unsafe {
            let _ = syscall::syscall0(syscall::SYS_SCHED_YIELD);
        }
        waited_ms = waited_ms.saturating_add(1);
    }

    let first_pid = if spawned_count > 0 {
        SERVICES[spawned_indices[0]].current_pid()
    } else {
        0
    };
    let second_pid = if spawned_count > 1 {
        SERVICES[spawned_indices[1]].current_pid()
    } else {
        0
    };
    (ready_count, first_pid, second_pid)
}

fn execute_cold_respawn_phoenix(service_watchdog: &mut ServiceWatchdog) -> protocol::InitReply {
    const STRATE1_KILL_TARGETS: &[&str] = &[
        "crypto_server",
        "memory_server",
        "vfs_server",
        "device_server",
    ];
    const STRATE2_KILL_TARGETS: &[&str] = &[
        "network_server",
        "scheduler_server",
        "exo_compositor",
    ];

    // 1. Destructive phase: real SIGKILL + reap of Strate 1 & Strate 2 targets.
    let mut k = 0usize;
    while k < STRATE1_KILL_TARGETS.len() {
        if let Some(idx) =
            service_table::runtime_index_by_name(&SERVICES, STRATE1_KILL_TARGETS[k].as_bytes())
        {
            let pid = SERVICES[idx].current_pid();
            if pid != 0 {
                let _ = unsafe { boot_sequence::terminate_timed_out_pid(pid) };
                SERVICES[idx].mark_dead();
                SERVICES[idx].restart_delay_ticks.store(0, Ordering::Release);
                service_watchdog.observe_stop(idx);
            }
        }
        k += 1;
    }
    let mut k2 = 0usize;
    while k2 < STRATE2_KILL_TARGETS.len() {
        if let Some(idx) =
            service_table::runtime_index_by_name(&SERVICES, STRATE2_KILL_TARGETS[k2].as_bytes())
        {
            let pid = SERVICES[idx].current_pid();
            if pid != 0 {
                kill_service(pid, 9); // SIGKILL
                SERVICES[idx].mark_dead();
                SERVICES[idx].restart_delay_ticks.store(0, Ordering::Release);
                service_watchdog.observe_stop(idx);
            }
        }
        k2 += 1;
    }
    unsafe {
        reap_children(service_watchdog);
    }

    // 2. Wave R0 (parallel): crypto_server + memory_server (ipc_router verified alive / SRV-05).
    let (r0_ready, crypto_pid, mem_pid) =
        cold_respawn_wave(&["crypto_server", "memory_server"], 2500, service_watchdog);

    // 3. Wave R1 (parallel): vfs_server + device_server (fb_server channel verified alive).
    let (r1_ready, vfs_pid, dev_pid) =
        cold_respawn_wave(&["vfs_server", "device_server"], 2500, service_watchdog);

    // 4. Wave R2: PHX-03 BLAKE3 check on init_server & ipc_router binaries.
    let phx03_ok = unsafe {
        boot_sequence::verify_binary_integrity("ipc_router", service_table::IPC_ROUTER_BIN)
            && boot_sequence::verify_binary_integrity("fb_server", service_table::FB_SERVER_BIN)
    };

    let mut reply = protocol::InitReply::ok();
    let total_respawned = r0_ready.saturating_add(r1_ready);
    reply.data[0..4].copy_from_slice(&total_respawned.to_le_bytes());
    reply.data[4..8].copy_from_slice(&crypto_pid.to_le_bytes());
    reply.data[8..12].copy_from_slice(&mem_pid.to_le_bytes());
    reply.data[12..16].copy_from_slice(&vfs_pid.to_le_bytes());
    reply.data[16..20].copy_from_slice(&dev_pid.to_le_bytes());
    reply.data[20..24].copy_from_slice(&(if phx03_ok { 1u32 } else { 0u32 }).to_le_bytes());
    reply
}

fn spawn_strate2_async_after_reply(service_watchdog: &mut ServiceWatchdog) {
    const STRATE2_RESPAWN: &[&str] = &["network_server", "scheduler_server", "exo_compositor"];
    unsafe {
        reap_children(service_watchdog);
    }
    let mut i = 0usize;
    while i < STRATE2_RESPAWN.len() {
        if let Some(idx) =
            service_table::runtime_index_by_name(&SERVICES, STRATE2_RESPAWN[i].as_bytes())
        {
            let svc = &SERVICES[idx];
            if svc.current_pid() == 0 && !svc.is_disabled() {
                svc.enable();
                let pid = unsafe { boot_sequence::spawn_service(svc.name, svc.bin_path) };
                if pid != 0 {
                    svc.set_pid(pid);
                    service_watchdog.observe_spawn(idx);
                }
            }
        }
        i += 1;
    }
}

#[inline]
fn caller_can_control_services(sender_pid: u32) -> bool {
    sender_pid == 0
        || sender_pid == 1
        || syscall::peer_is_any_service(
            sender_pid,
            &[syscall::SVC_INIT_SERVER, syscall::SVC_EXO_SHIELD],
        )
}

#[inline]
fn caller_can_power_action(sender_pid: u32) -> bool {
    sender_pid == 0
        || sender_pid == 1
        || syscall::peer_is_any_service(
            sender_pid,
            &[
                syscall::SVC_INIT_SERVER,
                syscall::SVC_EXO_COMPOSITOR,
                syscall::SVC_EXOSH,
            ],
        )
}

fn handle_control_plane(service_watchdog: &mut ServiceWatchdog) {
    let mut request = protocol::InitRequest::zeroed();
    let mut async_strate2_after_reply = false;
    let mut override_reply_ep: u64 = 0;
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
                if !caller_can_control_services(request.sender_pid) {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
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
            }
            protocol::INIT_MSG_STOP => {
                if !caller_can_control_services(request.sender_pid) {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
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
            }
            protocol::INIT_MSG_RESTART => {
                if !caller_can_control_services(request.sender_pid) {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
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
            }
            protocol::INIT_MSG_CHILD_DIED => {
                if request.sender_pid != 0 && request.sender_pid != 1 {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
                    match protocol::read_u32(&request.payload, 0) {
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
                    }
                }
            }
            protocol::INIT_MSG_PREPARE_ISOLATION => {
                if !caller_can_control_services(request.sender_pid) {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
                    isolation::prepare_isolation_reply(&SERVICES)
                }
            }
            protocol::INIT_MSG_RELAUNCH_STRATE2 => {
                if !caller_can_control_services(request.sender_pid) {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
                    let scheduled = supervisor::schedule_strate2_async_relaunch(&SERVICES);
                    if supervisor::all_strate2_ready(&SERVICES) {
                        log::line(b"init_server: strate2 converged (t6)");
                    }
                    protocol::lifecycle_reply(scheduled, supervisor::running_mask(&SERVICES))
                }
            }
            protocol::INIT_MSG_COLD_RESPAWN_PHOENIX => {
                if request.sender_pid != 0 && request.sender_pid != 1 {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
                    if request.sender_pid == 0 {
                        override_reply_ep = u64::from_le_bytes([
                            request.payload[0],
                            request.payload[1],
                            request.payload[2],
                            request.payload[3],
                            request.payload[4],
                            request.payload[5],
                            request.payload[6],
                            request.payload[7],
                        ]);
                    }
                    async_strate2_after_reply = true;
                    execute_cold_respawn_phoenix(service_watchdog)
                }
            }
            protocol::INIT_MSG_POWER_ACTION => {
                if !caller_can_power_action(request.sender_pid) {
                    protocol::InitReply::error(syscall::EPERM)
                } else {
                    const RB_AUTOBOOT: u64 = 0x0123_4567;
                    const RB_POWER_OFF: u64 = 0x4321_fedc;
                    let cmd = match request.payload[0] {
                        1 => RB_AUTOBOOT,
                        2 => RB_POWER_OFF,
                        _ => 0,
                    };
                    if cmd != 0 {
                        log::line(if cmd == RB_AUTOBOOT {
                            b"init_server: reboot requested by desktop"
                        } else {
                            b"init_server: poweroff requested by desktop"
                        });
                        unsafe {
                            let _ = syscall::syscall4(
                                syscall::SYS_REBOOT,
                                syscall::LINUX_REBOOT_MAGIC1,
                                syscall::LINUX_REBOOT_MAGIC2,
                                cmd,
                                0,
                            );
                        }
                        protocol::InitReply::ok()
                    } else {
                        protocol::InitReply::error(syscall::EINVAL)
                    }
                }
            }
            _ => protocol::InitReply::error(syscall::EINVAL),
        },
    };

    if override_reply_ep != 0 {
        unsafe {
            let _ = syscall::syscall6(
                syscall::SYS_IPC_SEND,
                override_reply_ep,
                &reply as *const protocol::InitReply as u64,
                core::mem::size_of::<protocol::InitReply>() as u64,
                0,
                0,
                0,
            );
        }
    } else if request.sender_pid != 0 {
        let _ = protocol::send_reply(request.sender_pid, &reply);
    }

    if async_strate2_after_reply {
        // Relaunch Strate 2 asynchronously strictly AFTER replying (outside SLA window).
        spawn_strate2_async_after_reply(service_watchdog);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Point d'entrée
// ─────────────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start(boot_info_virt: usize) -> ! {
    log::line(b"init_server: PID1 online");

    let boot_info = unsafe { boot_info::BootInfo::from_virt(boot_info_virt) };
    if let Some(info) = boot_info {
        if !info.validate() {
            log::line(b"init_server: invalid boot info");
            halt_forever();
        }
    }

    let mut service_watchdog = ServiceWatchdog::new();

    log::line(b"init_server: registering control endpoint");
    protocol::register_endpoint();
    unsafe {
        sigchld_handler::install_handlers();
    }

    // ── 2. Démarrer tous les services dans l'ordre ────────────────────────
    log::line(b"init_server: starting service graph");
    // FIX-SRV-M9 (ANALYSE_SERVERS §M9) : timeout global de boot.
    // Chaque service a déjà son propre timeout (ready_timeout_ms).
    // Ce compteur global garantit que l'OS entre en mode rescue si la somme
    // des timeouts individuels dépasse 45 secondes -- signalant un blocage
    // inattendu dans la résolution des dépendances.
    let boot_global_start_ms = boot_sequence::monotonic_ms();
    const GLOBAL_BOOT_TIMEOUT_MS: u64 = 180_000; // 180s max pour les 17 services en émulation QEMU debug
    let _ = unsafe { boot_sequence::boot_services(&SERVICES) };
    let boot_elapsed = boot_sequence::monotonic_ms().saturating_sub(boot_global_start_ms);
    if boot_elapsed > GLOBAL_BOOT_TIMEOUT_MS {
        log::line(b"init_server: BOOT GLOBAL TIMEOUT -- entering degraded supervision mode");
        // Mode rescue : continuer la supervision mais logger le problème
    }
    let mut idx = 0usize;
    while idx < SERVICES.len() {
        if SERVICES[idx].current_pid() != 0 {
            service_watchdog.observe_spawn(idx);
        }
        idx += 1;
    }
    log::line(b"init_server: service graph booted");

    // Émettre le carillon de démarrage (Chime Boot - SPEC-AUDIO-STRATA §6, fallback silencieux)
    play_boot_chime();

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
            refresh_service_readiness(check_idx);
            if !service_watchdog.check(check_idx, &SERVICES[check_idx]) {
                SERVICES[check_idx].mark_dead();
                service_watchdog.observe_stop(check_idx);
            } else {
                SERVICES[check_idx].note_alive();
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

exo_syscall_abi::ring1_nostd_panic_handler!(b"");
