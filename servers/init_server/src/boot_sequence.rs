use super::{dependency, log, service_manager, supervisor, syscall, Service};

const POLL_INTERVAL_MS: u64 = 5;
const BOOT_PHASE_TIMEOUT_MS: u64 = 30_000;
const CLOCK_MONOTONIC: u64 = 1;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[inline]
fn sleep_ms(ms: u64) {
    let ts = Timespec {
        tv_sec: (ms / 1_000) as i64,
        tv_nsec: ((ms % 1_000) * 1_000_000) as i64,
    };

    unsafe {
        let _ = syscall::syscall2(syscall::SYS_NANOSLEEP, &ts as *const Timespec as u64, 0);
    }
}

#[inline]
fn monotonic_ms() -> u64 {
    let mut ts = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
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

#[inline]
fn pid_alive(pid: u32) -> bool {
    unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, 0) == 0 }
}

#[inline]
fn endpoint_registered(service_name: &str) -> bool {
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_LOOKUP,
            service_name.as_ptr() as u64,
            service_name.len() as u64,
            0,
        )
    };
    rc > 0
}

#[inline]
unsafe fn isolate_process_group(pid: u32) {
    if pid != 0 {
        let _ = syscall::syscall2(syscall::SYS_SETPGID, pid as u64, pid as u64);
    }
}

/// Démarre un serveur Ring1 via fork + execve.
///
/// Retourne le PID du fils, ou `0` si le lancement échoue.
pub unsafe fn spawn_service(service_name: &str, bin_path: &[u8]) -> u32 {
    let argv: [u64; 2] = [bin_path.as_ptr() as u64, 0];
    let envp: [u64; 1] = [0];

    log::service_status(b"init: start ", service_name, b"\n");
    let child_pid = syscall::syscall0(syscall::SYS_FORK);
    if child_pid < 0 {
        log::service_error(b"init: fork failed ", service_name, child_pid);
        return 0;
    }

    if child_pid == 0 {
        let _ = syscall::syscall2(syscall::SYS_SETPGID, 0, 0);
        let rc = syscall::syscall3(
            syscall::SYS_EXECVE,
            bin_path.as_ptr() as u64,
            argv.as_ptr() as u64,
            envp.as_ptr() as u64,
        );
        if rc < 0 {
            log::service_error(b"init-child: exec failed ", service_name, rc);
            let _ = syscall::syscall1(syscall::SYS_EXIT, 127);
            let _ = syscall::syscall1(syscall::SYS_EXIT_GROUP, 127);
        }
        loop {
            core::hint::spin_loop();
        }
    }

    isolate_process_group(child_pid as u32);
    log::service_pid(b"init: spawned ", service_name, child_pid as u32);
    child_pid as u32
}

/// Attend qu'un serveur soit prêt à participer à la chaîne IPC.
///
/// La barrière de readiness est :
/// 1. le processus existe toujours (`kill(pid, 0)`);
/// 2. son endpoint IPC est bien visible dans le registre kernel.
pub unsafe fn wait_for_ipc_ready(service_name: &str, pid: u32, timeout_ms: u64) -> bool {
    let mut waited_ms = 0u64;
    while waited_ms <= timeout_ms {
        if pid_alive(pid) && endpoint_registered(service_name) {
            return true;
        }

        sleep_ms(POLL_INTERVAL_MS);
        waited_ms = waited_ms.saturating_add(POLL_INTERVAL_MS);
    }

    false
}

/// Démarre séquentiellement la chaîne Ring1 canonique V4.
///
/// La dépendance est volontairement stricte : chaque service doit être vivant
/// et stabilisé avant le lancement du suivant.
pub unsafe fn boot_services(services: &[Service]) -> usize {
    let boot_start = monotonic_ms();
    let mut progress = true;
    while progress {
        let now = monotonic_ms();
        if boot_start != 0 && now.saturating_sub(boot_start) >= BOOT_PHASE_TIMEOUT_MS {
            log::line(b"init: service graph timeout");
            let mut report_idx = 0usize;
            while report_idx < services.len() {
                if services[report_idx].current_pid() != 0 {
                    log::service_pid(
                        b"init: service alive ",
                        services[report_idx].name,
                        services[report_idx].current_pid(),
                    );
                } else {
                    log::service_status(
                        b"init: service pending ",
                        services[report_idx].name,
                        b"\n",
                    );
                }
                report_idx += 1;
            }
            break;
        }

        progress = false;

        let mut idx = 0usize;
        while idx < services.len() {
            let service = &services[idx];
            if service.current_pid() != 0 {
                idx += 1;
                continue;
            }
            if !supervisor::can_start(services, service.name) {
                idx += 1;
                continue;
            }

            let pid = spawn_service(service.name, service.bin_path);
            if pid == 0 {
                idx += 1;
                continue;
            }

            service.set_pid(pid);
            if wait_for_ipc_ready(
                service.name,
                pid,
                dependency::ready_timeout_ms(service.name),
            ) {
                log::service_pid(b"init: ready ", service.name, pid);
                progress = true;
            } else {
                log::service_status(b"init: timeout ", service.name, b"\n");
                let _ = syscall::syscall2(syscall::SYS_KILL, pid as u64, 15);
                service.mark_dead();
            }
            idx += 1;
        }
    }

    service_manager::running_count(services)
}
