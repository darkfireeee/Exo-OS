use super::{dependency, service_manager, supervisor, syscall, Service};

const POLL_INTERVAL_MS: u64 = 5;
const IPC_READY_SETTLE_MS: u64 = 10;

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
fn pid_alive(pid: u32) -> bool {
    unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, 0) == 0 }
}

/// Démarre un serveur Ring1 via fork + execve.
///
/// Retourne le PID du fils, ou `0` si le lancement échoue.
pub unsafe fn spawn_service(service_name: &str, bin_path: &[u8]) -> u32 {
    let _ = service_name;

    let argv: [u64; 2] = [bin_path.as_ptr() as u64, 0];
    let envp: [u64; 1] = [0];

    let child_pid = syscall::syscall0(syscall::SYS_FORK);
    if child_pid < 0 {
        return 0;
    }

    if child_pid == 0 {
        let rc = syscall::syscall3(
            syscall::SYS_EXECVE,
            bin_path.as_ptr() as u64,
            argv.as_ptr() as u64,
            envp.as_ptr() as u64,
        );
        if rc < 0 {
            let _ = syscall::syscall1(syscall::SYS_EXIT, 127);
        }
        loop {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }

    child_pid as u32
}

/// Attend qu'un serveur soit prêt à participer à la chaîne IPC.
///
/// L'ABI courante n'expose pas encore d'ack explicite "IPC ready" depuis Ring1.
/// La barrière retenue est donc :
/// 1. le processus répond à `kill(pid, 0)` ;
/// 2. on laisse une courte fenêtre de stabilisation pour que `_start()`
///    termine l'enregistrement IPC initial.
pub unsafe fn wait_for_ipc_ready(pid: u32, timeout_ms: u64) -> bool {
    let mut waited_ms = 0u64;
    while waited_ms <= timeout_ms {
        if pid_alive(pid) {
            sleep_ms(IPC_READY_SETTLE_MS);
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
    let mut progress = true;
    while progress {
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
            if wait_for_ipc_ready(pid, dependency::ready_timeout_ms(service.name)) {
                progress = true;
            } else {
                let _ = syscall::syscall2(syscall::SYS_KILL, pid as u64, 15);
                service.mark_dead();
            }
            idx += 1;
        }
    }

    service_manager::running_count(services)
}
