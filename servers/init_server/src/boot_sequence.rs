use super::{dependency, log, service_manager, service_table, syscall, Service};

const POLL_INTERVAL_MS: u64 = 5;
const BOOT_PHASE_TIMEOUT_MS: u64 = 300_000;
const READY_TIMEOUT_GRACE_MS: u64 = 5_000;
const CLOCK_MONOTONIC: u64 = 1;
const POLL_YIELD_ROUNDS: usize = 4;
const POLL_SPIN_ROUNDS: usize = 256;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

/// DIAG-P* (bisect run 6) : trace fine du chemin post-nanosleep de spawn_service
/// jusqu'au premier poll service_ready — le SEGV rip=0 (call/jmp/ret NULL)
/// tombe quelque part dans cette fenêtre ; le dernier <P*> imprimé avant le
/// crash désigne la fonction fautive à une instruction près.
fn diag_step(tag: &[u8], n: u64) {
    let mut buf = [0u8; 24];
    let mut len = 0usize;
    let mut push = |b: &[u8]| {
        let room = buf.len().saturating_sub(len);
        let c = b.len().min(room);
        buf[len..len + c].copy_from_slice(&b[..c]);
        len += c;
    };
    push(tag);
    if n != u64::MAX {
        let mut d = [0u8; 20];
        let mut v = n;
        let mut i = d.len();
        if v == 0 {
            i -= 1;
            d[i] = b'0';
        }
        while v != 0 && i > 0 {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        push(&d[i..]);
    }
    push(b">\n");
    log::write_all(&buf[..len]);
}

/// DIAG-V9 (SEGV rip=0 run 10) : empreinte FNV-1a de la PREMIÈRE PAGE .text de
/// `boot_services` (entrée + 4096 octets — contient le site d'appel `set_pid`
/// à ~0xd5b : les marqueurs P2/P2b et l'appel fautif sont dans CETTE page).
/// L'empreinte est prise à <P1> (page prouvée saine : P1/P2 s'exécutent) puis
/// re-vérifiée à <P2b>, juste avant l'appel qui crashe :
///   - hash BAD à P2b  → la page .text a été corrompue ENTRE P1 et P2b
///     (fenêtre fork/exec/teardown/préemption) → écriture aliène CONFIRMÉE
///     côté user, les gardes kernel V9 doivent l'attraper au prochain run ;
///   - hash ok à P2b puis crash → la corruption atterrit dans les ~20
///     instructions restantes (fenêtre IRQ timer) ou le saut vers 0 vient
///     d'un registre/kstate kernel corrompu → le dump V9 (ret0/rdx8) tranche.
static V9_TXT_HASH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[inline]
fn v9_boot_text_hash() -> u64 {
    let base = boot_services as *const () as *const u8;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0usize;
    while i < 4096 {
        // SAFETY: lecture volatile .text de la page contenant le code en cours
        // d'exécution (mappée, présente — on vient d'y exécuter diag_step).
        let b = unsafe { core::ptr::read_volatile(base.add(i)) };
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

fn v9_check_boot_text() {
    let now = v9_boot_text_hash();
    let prev = V9_TXT_HASH.load(core::sync::atomic::Ordering::Relaxed);
    if prev == 0 {
        V9_TXT_HASH.store(now, core::sync::atomic::Ordering::Relaxed);
        diag_step(b"<P1t txt=", now);
    } else if prev == now {
        diag_step(b"<P2t ok=1 txt=", now);
    } else {
        diag_step(b"<P2t BAD txt=", now);
    }
}

#[inline]
fn service_poll_delay() {
    let mut yields = 0usize;
    while yields < POLL_YIELD_ROUNDS {
        unsafe {
            let _ = syscall::syscall0(syscall::SYS_SCHED_YIELD);
        }
        let mut spins = 0usize;
        while spins < POLL_SPIN_ROUNDS {
            core::hint::spin_loop();
            spins += 1;
        }
        yields += 1;
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
fn service_ready(service_name: &str, pid: u32) -> bool {
    // DIAG-SR2 (bisect run 5) : marquer l'ENTRÉE de service_ready — le SEGV
    // rip=0 survient quelque part entre le retour du nanosleep et ce point ;
    // si <SR2> n'apparaît pas, le crash est AVANT (wrappers/logique de la
    // boucle de vagues) ; si <SR2> apparaît, il est DANS les syscalls du poll.
    if service_name == "ipc_router" {
        use core::sync::atomic::{AtomicU32, Ordering};
        static S: AtomicU32 = AtomicU32::new(0);
        if S.fetch_add(1, Ordering::Relaxed) < 2 {
            log::write_all(b"<SR2 enter>\n");
        }
    }
    let alive = pid_alive(pid);
    let ep = endpoint_registered(service_name);
    // DIAG-25 : tracer pourquoi ipc_router n'est pas vu "ready" (a=pid_alive,
    // e=endpoint_registered). Throttlé pour ne pas saturer la console E9.
    if service_name == "ipc_router" {
        use core::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        if N.fetch_add(1, Ordering::Relaxed) % 64 == 0 {
            let mut buf = *b"<SR a=0 e=0>\n";
            if alive {
                buf[6] = b'1';
            }
            if ep {
                buf[10] = b'1';
            }
            log::write_all(&buf);
        }
    }
    alive && ep
}

#[inline]
pub unsafe fn ipc_ready_now(service_name: &str, pid: u32) -> bool {
    service_ready(service_name, pid)
}

fn poll_until(pid: u32, timeout_ms: u64, mut predicate: impl FnMut() -> bool) -> bool {
    let start_ms = monotonic_ms();
    let mut waited_ms = 0u64;
    loop {
        if predicate() {
            return true;
        }

        if !pid_alive(pid) {
            return false;
        }

        let now_ms = monotonic_ms();
        if start_ms != 0 && now_ms != 0 {
            if now_ms.saturating_sub(start_ms) >= timeout_ms {
                return false;
            }
        } else if waited_ms >= timeout_ms {
            return false;
        }

        service_poll_delay();
        waited_ms = waited_ms.saturating_add(POLL_INTERVAL_MS);
    }
}

#[inline]
fn wait_for_late_ipc_ready(service_name: &str, pid: u32) -> bool {
    poll_until(pid, READY_TIMEOUT_GRACE_MS, || {
        service_ready(service_name, pid)
    })
}

/// Demande l'arrêt d'un service dont la readiness a expiré.
///
/// Le PID reste supervisé tant que l'arrêt n'est pas réellement observable.
/// Sans cette barrière, un endpoint enregistré tardivement peut coexister avec
/// un respawn immédiat du même service.
pub unsafe fn terminate_timed_out_pid(pid: u32) -> bool {
    let _ = syscall::syscall2(syscall::SYS_KILL, pid as u64, 15);
    poll_until(pid, READY_TIMEOUT_GRACE_MS, || !pid_alive(pid))
}

#[inline]
fn owns_interactive_console(service_name: &str) -> bool {
    service_name == "exosh"
}

#[inline]
fn should_quiet_console_after_ready(service_name: &str) -> bool {
    service_name == "tty_server" || owns_interactive_console(service_name)
}

/// Démarre un serveur Ring1 via fork + execve.
///
/// Le noyau implémente `vfork` comme un clone CoW séparé, mais son attente parent
/// a produit un retour utilisateur à RIP nul juste après le premier exec. `fork`
/// offre le même isolement d'espace d'adressage sans traverser ce retour bloquant.
/// La barrière de stabilisation post-exec ci-dessous laisse l'enfant terminer son
/// chargement et son enregistrement IPC avant le lancement du service suivant.
/// Contrat respecté : l'enfant ne fait que setpgid + execve (ou _exit) et ne
/// retourne jamais de cette fonction.
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

    if !owns_interactive_console(service_name) {
        log::service_pid(b"init: spawned ", service_name, child_pid as u32);
    }

    // FIX #25 (Audit 4.1) — Barrière post-execve : bloquer init pendant que
    // l'enfant termine son démarrage utilisateur (demand-paging .text +
    // IPC_REGISTER). Sans cette barrière, init reprend immédiatement après
    // l'execve de l'enfant et la course de co-planification peut corrompre
    // une frame vivante d'init (validé expérimentalement session 2026-06-21 :
    // 500 ms fait progresser init de 1→2 services au lieu d'un seul).
    // Le shell `exosh` est exempté car c'est le service terminal — aucun spawn
    // supplémentaire n'a lieu après lui, donc la barrière n'apporterait rien
    // et ralentirait l'accès au shell.
    if !owns_interactive_console(service_name) {
        let stabilise = Timespec {
            tv_sec: 0,
            tv_nsec: 500_000_000, // 500 ms — validé session 2026-06-21
        };
        let _ = unsafe {
            syscall::syscall2(
                syscall::SYS_NANOSLEEP,
                &stabilise as *const Timespec as u64,
                0,
            )
        };
    }

    // DIAG-P1 (bisect run 6) : le nanosleep est terminé, spawn_service va
    // retourner child_pid à boot_services.
    diag_step(b"<P1 post-sleep>", u64::MAX);
    // DIAG-V9 : baseline de l'empreinte .text de boot_services (page du site
    // d'appel set_pid) — comparée à <P2b> juste avant l'appel qui crashe.
    v9_check_boot_text();
    child_pid as u32
}

/// Attend qu'un serveur soit prêt à participer à la chaîne IPC.
///
/// La barrière de readiness est :
/// 1. le processus existe toujours (`kill(pid, 0)`);
/// 2. son endpoint IPC est bien visible dans le registre kernel.
pub unsafe fn wait_for_ipc_ready(service_name: &str, pid: u32, timeout_ms: u64) -> bool {
    let start_ms = monotonic_ms();
    let mut waited_ms = 0u64;
    loop {
        if service_ready(service_name, pid) {
            return true;
        }

        let now_ms = monotonic_ms();
        if start_ms != 0 && now_ms != 0 {
            if now_ms.saturating_sub(start_ms) >= timeout_ms {
                return wait_for_late_ipc_ready(service_name, pid);
            }
        } else if waited_ms >= timeout_ms {
            return wait_for_late_ipc_ready(service_name, pid);
        }

        service_poll_delay();
        waited_ms = waited_ms.saturating_add(POLL_INTERVAL_MS);
    }
}

#[inline]
fn service_index_by_name(services: &[Service], name: &str) -> Option<usize> {
    let mut idx = 0usize;
    while idx < services.len() {
        if services[idx].name == name {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

#[inline]
fn dependency_ready_in_wave(services: &[Service], ready_mask: u64, dep: &str) -> bool {
    if dep == "init_server" {
        return true;
    }
    let Some(idx) = service_index_by_name(services, dep) else {
        return false;
    };
    if (ready_mask & (1u64 << idx)) != 0 {
        return true;
    }
    dependency::metadata(dep)
        .map(|meta| !meta.critical && services[idx].is_dead())
        .unwrap_or(false)
}

fn can_start_in_wave(services: &[Service], name: &str, ready_mask: u64) -> bool {
    let optional = dependency::optional_dependencies(name);
    dependency::dependencies_satisfied(name, |dep| {
        dependency_ready_in_wave(services, ready_mask, dep) || optional.contains(&dep)
    })
}

fn log_service_graph_timeout(services: &[Service]) {
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
            log::service_status(b"init: service pending ", services[report_idx].name, b"\n");
        }
        report_idx += 1;
    }
}

#[inline]
fn graph_timeout_expired(boot_start: u64) -> bool {
    let now = monotonic_ms();
    boot_start != 0 && now.saturating_sub(boot_start) >= BOOT_PHASE_TIMEOUT_MS
}

#[inline]
fn note_graph_progress(last_progress_ms: &mut u64) {
    let now = monotonic_ms();
    if now != 0 {
        *last_progress_ms = now;
    }
}

/// Démarre la chaîne Ring1 canonique V4 par vagues de dépendances.
///
/// Tous les services dont les dépendances déjà prêtes sont satisfaites sont
/// lancés ensemble. La readiness de la vague est ensuite pollée en groupe afin
/// d'éviter de payer un timeout séquentiel par service.
pub unsafe fn boot_services(services: &[Service]) -> usize {
    let mut last_progress_ms = monotonic_ms();
    let mut ready_mask = 0u64;
    let mut fallback_wait_ms = [0u64; service_table::SERVICE_COUNT];

    loop {
        if graph_timeout_expired(last_progress_ms) {
            log_service_graph_timeout(services);
            break;
        }

        let mut launched = false;
        let mut idx = 0usize;
        while idx < services.len() {
            let service = &services[idx];
            if service.current_pid() != 0 {
                idx += 1;
                continue;
            }
            if service.is_dead() {
                idx += 1;
                continue;
            }
            if !can_start_in_wave(services, service.name, ready_mask) {
                // DIAG-P5s (bisect run 6) : service non démarrable dans cette
                // vague (dépendances manquantes) — la boucle continue.
                diag_step(b"<P5skip i=", idx as u64);
                idx += 1;
                continue;
            }

            // DIAG-P5 (bisect run 6) : can_start_in_wave OK pour ce service —
            // le spawn du service suivant va avoir lieu.
            diag_step(b"<P5 wave-ok i=", idx as u64);
            let pid = spawn_service(service.name, service.bin_path);
            // DIAG-P2 (bisect run 6) : retour de spawn_service.
            diag_step(b"<P2 ret-spawn>", u64::MAX);
            if pid == 0 {
                idx += 1;
                continue;
            }

            // DIAG-V9 (SEGV run 10 : crash ENTRE P2 et P3, registres = args de
            // set_pid (rdi=&SERVICES[0], rsi=pid) + garbage rax=1/rcx=1/rbx=rbp=0)
            // : P2b se place à ~5 instructions du `call *%rax` fautif et re-vérifie
            // l'intégrité de la page .text du site d'appel (cf. v9_check_boot_text).
            diag_step(b"<P2b pre-setpid>", u64::MAX);
            v9_check_boot_text();
            service.set_pid(pid);
            // DIAG-P3 (bisect run 6) : set_pid terminé (inclut monotonic_ms).
            diag_step(b"<P3 setpid>", u64::MAX);
            if idx < fallback_wait_ms.len() {
                fallback_wait_ms[idx] = 0;
            }
            note_graph_progress(&mut last_progress_ms);
            // DIAG-P4 (bisect run 6) : note_graph_progress terminé (inclut
            // monotonic_ms → SYS_CLOCK_GETTIME).
            diag_step(b"<P4 progress>", u64::MAX);
            launched = true;
            idx += 1;
        }

        let mut pending;
        let mut readiness_changed = false;
        // DIAG-P6 (bisect run 6) : entrée dans la boucle de readiness — le
        // prochain appel est service_ready (marqueur <SR2 enter>).
        diag_step(b"<P6 rdloop>", u64::MAX);
        loop {
            if graph_timeout_expired(last_progress_ms) {
                log_service_graph_timeout(services);
                return service_manager::running_count(services);
            }

            pending = false;
            let mut settled = false;
            idx = 0;
            while idx < services.len() {
                let service = &services[idx];
                if (ready_mask & (1u64 << idx)) != 0 {
                    idx += 1;
                    continue;
                }

                let pid = service.current_pid();
                if pid == 0 {
                    idx += 1;
                    continue;
                }

                if service_ready(service.name, pid) {
                    ready_mask |= 1u64 << idx;
                    service.mark_ready();
                    if should_quiet_console_after_ready(service.name) {
                        log::set_console_quiet(true);
                    }
                    log::service_pid(b"init: ready ", service.name, pid);
                    settled = true;
                    readiness_changed = true;
                    note_graph_progress(&mut last_progress_ms);
                    idx += 1;
                    continue;
                }

                let spawn_ms = service
                    .spawn_time_ms
                    .load(core::sync::atomic::Ordering::Acquire);
                let now_ms = monotonic_ms();
                let timeout_ms = dependency::ready_timeout_ms(service.name);
                let timed_out = if spawn_ms != 0 && now_ms != 0 {
                    now_ms.saturating_sub(spawn_ms) >= timeout_ms
                } else {
                    idx < fallback_wait_ms.len() && fallback_wait_ms[idx] >= timeout_ms
                };
                if timed_out {
                    if wait_for_late_ipc_ready(service.name, pid) {
                        ready_mask |= 1u64 << idx;
                        service.mark_ready();
                        if should_quiet_console_after_ready(service.name) {
                            log::set_console_quiet(true);
                        }
                        log::service_pid(b"init: ready ", service.name, pid);
                        settled = true;
                        readiness_changed = true;
                        note_graph_progress(&mut last_progress_ms);
                        idx += 1;
                        continue;
                    }

                    log::service_status(b"init: timeout ", service.name, b"\n");
                    if terminate_timed_out_pid(pid) {
                        service.mark_dead();
                        settled = true;
                        readiness_changed = true;
                        note_graph_progress(&mut last_progress_ms);
                    } else {
                        pending = true;
                    }
                    idx += 1;
                    continue;
                }

                pending = true;
                idx += 1;
            }

            if !pending {
                break;
            }
            if settled {
                break;
            }
            let mut wait_idx = 0usize;
            while wait_idx < services.len() && wait_idx < fallback_wait_ms.len() {
                if (ready_mask & (1u64 << wait_idx)) == 0 && services[wait_idx].current_pid() != 0 {
                    fallback_wait_ms[wait_idx] =
                        fallback_wait_ms[wait_idx].saturating_add(POLL_INTERVAL_MS);
                }
                wait_idx += 1;
            }
            service_poll_delay();
        }

        if !launched && !readiness_changed {
            // DIAG-P7 (bisect run 6) : la vague se termine sans nouveau lancement
            // ni changement de readiness — boot_services va retourner.
            diag_step(b"<P7 wave-end>", u64::MAX);
            break;
        }
    }

    service_manager::running_count(services)
}
