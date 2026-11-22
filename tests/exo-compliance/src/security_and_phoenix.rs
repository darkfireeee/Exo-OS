//! Adversarial Security Audit Suite (`S1`–`S6`) & ExoPhoenix 3-Strata O(1) Benchmark (`Cas A` / `Cas B`).
//!
//! Invoked via:
//! - `/bin/exo-compliance-runner --security-audit [--exit-qemu]`
//! - `/bin/exo-compliance-runner --phoenix-bench-a`
//! - `/bin/exo-compliance-runner --phoenix-bench-b`

use crate::raw::*;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const SELF_BIN: &str = "/bin/exo-compliance-runner";

pub fn run_security_audit(exit_qemu: bool) -> ! {
    let _ = fs::create_dir_all("/tmp");
    let _ = fs::create_dir_all("/mnt/usb");
    exo_log_line("[EXO_SEC] START SUITE=exo-security-hardening");

    let tests: &[(&str, fn() -> Result<(), String>)] = &[
        ("S1", test_s1_wx_exec_and_at_secure),
        ("S2", test_s2_scm_rights_zerotrust),
        ("S3", test_s3_pledge_abstract_and_ring1),
        ("S4", test_s4_quotas_contention_and_guard),
        ("S5", test_s5_shm_seal_and_wayland_fuzz),
        ("S6", test_s6_exophoenix_live_and_ledger),
    ];

    let mut pass = 0usize;
    let mut fail = 0usize;
    for (tier, func) in tests {
        let t0 = Instant::now();
        match func() {
            Ok(()) => {
                pass += 1;
                exo_log_line(&format!(
                    "[EXO_SEC] {} PASS DURATION_MS={}",
                    tier,
                    t0.elapsed().as_millis()
                ));
            }
            Err(detail) => {
                fail += 1;
                let clean = detail.trim().replace('\n', " | ");
                exo_log_line(&format!(
                    "[EXO_SEC] {} FAIL DURATION_MS={} DETAIL=\"{}\"",
                    tier,
                    t0.elapsed().as_millis(),
                    clean
                ));
            }
        }
    }

    exo_log_line(&format!(
        "[EXO_SEC] SUMMARY PASS={} FAIL={} TOTAL={}",
        pass,
        fail,
        tests.len()
    ));

    if exit_qemu {
        compliance_exit_qemu(fail == 0);
    }
    std::process::exit(if fail == 0 { 0 } else { 1 });
}

pub fn run_security_helper(args: &[String]) -> ! {
    match args.first().map(|s| s.as_str()) {
        Some("sec-s1-atsecure") => {
            // Apply NO_RING1_CRITICAL_IPC (flags=0x2) with EXEC(10) + DLOPEN(18) (without INET(5)/PROC(9)),
            // then execve("/bin/p3dyn") in-place with poisoned LD_LIBRARY_PATH=/tmp so the kernel
            // sets AT_SECURE=1 and ld-exo.so MUST ignore /tmp.
            let promises: u64 = 0x1_e45f | (1 << 18);
            let rc = unsafe { syscall2(SYS_EXO_PLEDGE, promises, 0x2) };
            if rc != 0 {
                eprintln!("sec-s1-atsecure pledge failed: {rc}");
                std::process::exit(2);
            }
            let path = b"/bin/p3dyn\0";
            let env_ld = b"LD_LIBRARY_PATH=/tmp\0";
            let argv: [*const u8; 2] = [path.as_ptr(), core::ptr::null()];
            let envp: [*const u8; 2] = [env_ld.as_ptr(), core::ptr::null()];
            let erc = unsafe {
                syscall3(
                    SYS_EXECVE,
                    path.as_ptr() as u64,
                    argv.as_ptr() as u64,
                    envp.as_ptr() as u64,
                )
            };
            eprintln!("execve /bin/p3dyn failed: {erc}");
            std::process::exit(4);
        }
        Some("sec-s3-child") => {
            // Minimal compat promises (0x1e45f: omits INET(5), PROC(9), UNIX_ABSTRACT(17), DLOPEN(18))
            // + NO_RING1_CRITICAL_IPC (flags=0x2).
            let promises: u64 = 0x1_e45f;
            let prc = unsafe { syscall2(SYS_EXO_PLEDGE, promises, 0x2) };
            if prc != 0 {
                eprintln!("pledge failed: {prc}");
                std::process::exit(10);
            }
            // Query active restriction mask via SYS_EXO_PLEDGE(0, 1)
            let mask = unsafe { syscall2(SYS_EXO_PLEDGE, 0, 1) };
            let expected_bits = (1i64 << 8) | (1i64 << 9) | (1i64 << 10) | (1i64 << 11); // NO_UNIX_ABSTRACT | NO_DLOPEN | NO_RING1_CRITICAL_IPC | NO_INET
            if mask < 0 || (mask & expected_bits) != expected_bits {
                eprintln!("unexpected pledge restriction mask: {mask:#x}");
                std::process::exit(11);
            }
            // 1. Abstract AF_UNIX bind must be denied with -EACCES
            let sfd = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_STREAM as u64, 0) };
            if sfd < 0 {
                eprintln!("AF_UNIX socket creation failed: {sfd}");
                std::process::exit(12);
            }
            let mut sun = [0u8; 110];
            sun[0..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
            sun[2] = 0; // abstract namespace
            let tag = b"sec_s3_abs_denied";
            sun[3..3 + tag.len()].copy_from_slice(tag);
            let brc = unsafe {
                syscall3(
                    SYS_BIND,
                    sfd as u64,
                    sun.as_ptr() as u64,
                    (3 + tag.len()) as u64,
                )
            };
            let _ = unsafe { syscall1(SYS_CLOSE, sfd as u64) };
            if brc != EACCES {
                eprintln!("abstract AF_UNIX bind expected EACCES (-13), got {brc}");
                std::process::exit(13);
            }
            // 2. Direct Ring 1 critical endpoint lookups must return -EACCES
            for ep_name in [
                b"device_server".as_slice(),
                b"crypto_server".as_slice(),
                b"memory_server".as_slice(),
                b"init_server".as_slice(),
            ] {
                let lrc = unsafe {
                    syscall2(
                        SYS_EXO_IPC_LOOKUP,
                        ep_name.as_ptr() as u64,
                        ep_name.len() as u64,
                    )
                };
                if lrc != EACCES {
                    eprintln!(
                        "critical endpoint lookup {:?} expected EACCES (-13), got {lrc}",
                        core::str::from_utf8(ep_name)
                    );
                    std::process::exit(14);
                }
            }
            // 3. Forbidden AF_INET socket (NO_NETWORK) must be rejected
            let net_rc = unsafe { syscall3(SYS_SOCKET, AF_INET as u64, SOCK_STREAM as u64, 0) };
            if net_rc >= 0 {
                let _ = unsafe { syscall1(SYS_CLOSE, net_rc as u64) };
                eprintln!("forbidden AF_INET socket unexpectedly succeeded: {net_rc}");
                std::process::exit(15);
            }
            std::process::exit(0);
        }
        _ => std::process::exit(99),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// S1: W^X, anonymous PROT_EXEC denial, runtime/noexec file mmap denial, AT_SECURE
// ─────────────────────────────────────────────────────────────────────────────

fn test_s1_wx_exec_and_at_secure() -> Result<(), String> {
    // 1. Anonymous mmap(PROT_READ | PROT_EXEC) MUST return -EACCES
    let anon_rx = unsafe {
        syscall6(
            SYS_MMAP,
            0,
            4096,
            PROT_READ | PROT_EXEC,
            MAP_PRIVATE | MAP_ANONYMOUS,
            u64::MAX,
            0,
        )
    };
    if anon_rx != EACCES {
        if anon_rx > 0 {
            let _ = unsafe { syscall2(SYS_MUNMAP, anon_rx as u64, 4096) };
        }
        return Err(format!("anon PROT_EXEC expected EACCES (-13), got {anon_rx}"));
    }

    // 2. Anonymous RW -> RX mprotect MUST return -EACCES
    let anon_rw = unsafe {
        syscall6(
            SYS_MMAP,
            0,
            4096,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            u64::MAX,
            0,
        )
    };
    if anon_rw <= 0 {
        return Err(format!("anon RW mmap failed: {anon_rw}"));
    }
    let mp_rc = unsafe { syscall3(SYS_MPROTECT, anon_rw as u64, 4096, PROT_READ | PROT_EXEC) };
    let _ = unsafe { syscall2(SYS_MUNMAP, anon_rw as u64, 4096) };
    if mp_rc != EACCES {
        return Err(format!("anon RW->RX mprotect expected EACCES (-13), got {mp_rc}"));
    }

    // 3. Unsigned runtime-written file in /tmp + /mnt/usb MUST be denied PROT_EXEC
    for probe_path in ["/tmp/s1_unsigned.so", "/mnt/usb/s1_unsigned.so"] {
        fs::write(probe_path, vec![0x90u8; 4096])
            .map_err(|e| format!("write {probe_path}: {e}"))?;
        let file = OpenOptions::new()
            .read(true)
            .open(probe_path)
            .map_err(|e| format!("open {probe_path}: {e}"))?;
        let fm = unsafe {
            syscall6(
                SYS_MMAP,
                0,
                4096,
                PROT_READ | PROT_EXEC,
                MAP_PRIVATE,
                file.as_raw_fd() as u64,
                0,
            )
        };
        drop(file);
        let _ = fs::remove_file(probe_path);
        if fm != EACCES {
            if fm > 0 {
                let _ = unsafe { syscall2(SYS_MUNMAP, fm as u64, 4096) };
            }
            return Err(format!("file PROT_EXEC on {probe_path} expected EACCES (-13), got {fm}"));
        }
    }

    // 4. AT_SECURE=1 ignores poisoned LD_LIBRARY_PATH=/tmp/libp3a.so
    let poison_path = "/tmp/libp3a.so";
    fs::write(poison_path, b"CORRUPTED_POISON_SO_MUST_BE_IGNORED")
        .map_err(|e| format!("write poison {poison_path}: {e}"))?;
    let atsec_out = Command::new(SELF_BIN)
        .args(["--sec-helper", "sec-s1-atsecure"])
        .output();
    let _ = fs::remove_file(poison_path);
    let atsec_out = atsec_out.map_err(|e| format!("spawn sec-s1-atsecure: {e}"))?;
    let atsec_stdout = String::from_utf8_lossy(&atsec_out.stdout);
    // p3dyn explicitly asserts `AT_SECURE == 0` when unconfined; under AT_SECURE=1 we MUST observe:
    //  1) `[P3_PROBE] FAIL auxv_at_secure_zero` (proves kernel stamped AT_SECURE=1), AND
    //  2) `needed_lib_plt_call` + `needed_lib_constructor_ran` PASS (proves ld-exo.so ignored
    //     poisoned `LD_LIBRARY_PATH=/tmp/libp3a.so` and loaded genuine `/lib/libp3a.so`).
    let at_secure_proven = atsec_stdout.contains("[P3_PROBE] FAIL auxv_at_secure_zero")
        && atsec_stdout.contains("[P3_PROBE] PASS needed_lib_plt_call")
        && atsec_stdout.contains("[P3_PROBE] PASS needed_lib_constructor_ran");
    if !at_secure_proven {
        return Err(format!(
            "AT_SECURE helper failed ({}): out={} err={}",
            atsec_out.status,
            atsec_stdout.trim(),
            String::from_utf8_lossy(&atsec_out.stderr).trim()
        ));
    }

    // 5. ExoLedger BLAKE3 chain intact after recording S1 alerts
    let ledger = unsafe { syscall3(SYS_EXOPHOENIX_TEST, 1, 0, 0) };
    if ledger < 0 {
        return Err(format!("ExoLedger chain verification failed: {ledger}"));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// S2: SCM_RIGHTS Zero-Trust type filtering, anti-cycle, and in-flight quotas
// ─────────────────────────────────────────────────────────────────────────────

fn send_scm_fds(sock: i32, fds: &[i32]) -> i64 {
    let payload = [b'S'];
    let iov = Iovec {
        iov_base: payload.as_ptr() as u64,
        iov_len: 1,
    };
    let cmsg_len = 16 + fds.len() * 4;
    let mut ctrl = vec![0u8; cmsg_len.max(32)];
    ctrl[0..8].copy_from_slice(&(cmsg_len as u64).to_ne_bytes());
    ctrl[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
    ctrl[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
    for (i, fd) in fds.iter().enumerate() {
        let off = 16 + i * 4;
        ctrl[off..off + 4].copy_from_slice(&fd.to_ne_bytes());
    }
    let hdr = Msghdr {
        msg_iov: &iov as *const _ as u64,
        msg_iovlen: 1,
        msg_control: ctrl.as_ptr() as u64,
        msg_controllen: cmsg_len as u64,
        ..Default::default()
    };
    unsafe { syscall3(SYS_SENDMSG, sock as u64, &hdr as *const _ as u64, MSG_DONTWAIT as u64) }
}

fn test_s2_scm_rights_zerotrust() -> Result<(), String> {
    let mut sv = [0i32; 2];
    if unsafe { syscall4(SYS_SOCKETPAIR, AF_UNIX as u64, SOCK_STREAM as u64, 0, sv.as_mut_ptr() as u64) } != 0 {
        return Err("socketpair failed".into());
    }

    // 1. Sending AF_UNIX socket fd over SCM_RIGHTS (cycle/GC attack) MUST return -EPERM
    let sock_rc = send_scm_fds(sv[0], &[sv[0]]);
    if sock_rc != EPERM {
        let _ = unsafe { syscall1(SYS_CLOSE, sv[0] as u64) };
        let _ = unsafe { syscall1(SYS_CLOSE, sv[1] as u64) };
        return Err(format!("SCM_RIGHTS socket fd expected EPERM (-1), got {sock_rc}"));
    }

    // 2. Sending epoll fd over SCM_RIGHTS MUST return -EPERM
    let epfd = unsafe { syscall1(SYS_EPOLL_CREATE1, 0) } as i32;
    let ep_rc = send_scm_fds(sv[0], &[epfd]);
    let _ = unsafe { syscall1(SYS_CLOSE, epfd as u64) };
    if ep_rc != EPERM {
        let _ = unsafe { syscall1(SYS_CLOSE, sv[0] as u64) };
        let _ = unsafe { syscall1(SYS_CLOSE, sv[1] as u64) };
        return Err(format!("SCM_RIGHTS epoll fd expected EPERM (-1), got {ep_rc}"));
    }

    // 3. Sending TTY fd (stdin = 0) over SCM_RIGHTS MUST return -EPERM
    let tty_rc = send_scm_fds(sv[0], &[0]);
    if tty_rc != EPERM {
        let _ = unsafe { syscall1(SYS_CLOSE, sv[0] as u64) };
        let _ = unsafe { syscall1(SYS_CLOSE, sv[1] as u64) };
        return Err(format!("SCM_RIGHTS TTY fd 0 expected EPERM (-1), got {tty_rc}"));
    }

    // 4. In-flight quota flood (> 16 eventfds on one socket) MUST return -EMFILE
    let mut efds = Vec::new();
    let mut hit_emfile = false;
    for _ in 0..20 {
        let efd = unsafe { syscall2(SYS_EVENTFD2, 1, 0) } as i32;
        if efd < 0 {
            break;
        }
        efds.push(efd);
        let rc = send_scm_fds(sv[0], &[efd]);
        if rc == EMFILE {
            hit_emfile = true;
            break;
        }
        if rc < 0 {
            for f in efds {
                let _ = unsafe { syscall1(SYS_CLOSE, f as u64) };
            }
            let _ = unsafe { syscall1(SYS_CLOSE, sv[0] as u64) };
            let _ = unsafe { syscall1(SYS_CLOSE, sv[1] as u64) };
            return Err(format!("SCM_RIGHTS flood unexpected errno {rc}"));
        }
    }
    for f in efds {
        let _ = unsafe { syscall1(SYS_CLOSE, f as u64) };
    }
    let _ = unsafe { syscall1(SYS_CLOSE, sv[0] as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, sv[1] as u64) };
    if !hit_emfile {
        return Err("SCM_RIGHTS flood did not trigger EMFILE (-24) at quota=16".into());
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// S3: Pledge confinement, abstract Unix denial, Ring 1 endpoint isolation
// ─────────────────────────────────────────────────────────────────────────────

fn test_s3_pledge_abstract_and_ring1() -> Result<(), String> {
    let out = Command::new(SELF_BIN)
        .args(["--sec-helper", "sec-s3-child"])
        .output()
        .map_err(|e| format!("spawn sec-s3-child: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "sec-s3-child failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// S4: Kernel quotas (`EPOLL_WATCHES` cap), concurrent `MAP_FIXED`, guard page
// ─────────────────────────────────────────────────────────────────────────────

fn test_s4_quotas_contention_and_guard() -> Result<(), String> {
    // 1. Hit per-PID epoll watch quota (512) using 8 epoll fds x 70 eventfds = 560 watches
    let mut epfds = Vec::new();
    let mut efds = Vec::new();
    for _ in 0..8 {
        let ep = unsafe { syscall1(SYS_EPOLL_CREATE1, 0) } as i32;
        if ep < 0 {
            break;
        }
        epfds.push(ep);
    }
    for _ in 0..70 {
        let efd = unsafe { syscall2(SYS_EVENTFD2, 0, 0) } as i32;
        if efd < 0 {
            break;
        }
        efds.push(efd);
    }
    let mut hit_cap = false;
    'outer: for &ep in &epfds {
        for &efd in &efds {
            let ev = EpollEvent {
                events: EPOLLIN,
                data: efd as u64,
            };
            let rc = unsafe {
                syscall4(
                    SYS_EPOLL_CTL,
                    ep as u64,
                    EPOLL_CTL_ADD as u64,
                    efd as u64,
                    &ev as *const _ as u64,
                )
            };
            if rc == ENOSPC || rc == -12 {
                hit_cap = true;
                break 'outer;
            }
        }
    }
    for f in efds {
        let _ = unsafe { syscall1(SYS_CLOSE, f as u64) };
    }
    for f in epfds {
        let _ = unsafe { syscall1(SYS_CLOSE, f as u64) };
    }
    if !hit_cap {
        return Err("EPOLL_WATCHES per-PID quota (512) did not reject with ENOSPC/ENOMEM".into());
    }

    // 2. Concurrent thread + mmap/MAP_FIXED/munmap contention (8 threads x 8 iterations)
    let mut handles = Vec::new();
    for _ in 0..8 {
        handles.push(thread::spawn(|| -> Result<(), String> {
            for _ in 0..8 {
                let p = unsafe {
                    syscall6(
                        SYS_MMAP,
                        0,
                        8192,
                        PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS,
                        u64::MAX,
                        0,
                    )
                };
                if p <= 0 {
                    return Err(format!("mmap failed: {p}"));
                }
                let p2 = unsafe {
                    syscall6(
                        SYS_MMAP,
                        p as u64,
                        4096,
                        PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                        u64::MAX,
                        0,
                    )
                };
                if p2 != p {
                    let _ = unsafe { syscall2(SYS_MUNMAP, p as u64, 8192) };
                    return Err(format!("MAP_FIXED remap mismatch: {p2} != {p}"));
                }
                unsafe {
                    core::ptr::write_volatile(p2 as *mut u8, 0xA5);
                }
                let _ = unsafe { syscall2(SYS_MUNMAP, p as u64, 8192) };
            }
            Ok(())
        }));
    }
    for h in handles {
        h.join().map_err(|_| "mmap stress thread panicked".to_string())??;
    }

    // 3. Hardware PROT_NONE guard page -> SIGSEGV
    let g_out = Command::new(SELF_BIN)
        .args(["--case", "t2_guard_page_sigsegv"])
        .output()
        .map_err(|e| format!("spawn guard test: {e}"))?;
    let st_s = g_out.status.to_string();
    if !st_s.contains("signal: 11") && g_out.status.code() != Some(139) {
        return Err(format!("guard page did not deliver SIGSEGV: {st_s}"));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// S5: `memfd` `F_SEAL_SHRINK`, malicious Wayland fuzzing, compositor liveness
// ─────────────────────────────────────────────────────────────────────────────

fn connect_wayland_unix() -> Result<i32, String> {
    let fd = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_STREAM as u64, 0) };
    if fd < 0 {
        return Err(format!("wayland socket failed: {fd}"));
    }
    let mut sun = [0u8; 110];
    sun[0..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    let path = b"/run/wayland-0";
    sun[2..2 + path.len()].copy_from_slice(path);
    let alen = (2 + path.len() + 1) as u64;
    let crc = unsafe { syscall3(SYS_CONNECT, fd as u64, sun.as_ptr() as u64, alen) };
    if crc != 0 {
        let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
        return Err(format!("connect /run/wayland-0 failed: {crc}"));
    }
    Ok(fd as i32)
}

fn push_wl_msg(buf: &mut Vec<u8>, obj: u32, opcode: u16, words: &[u32]) {
    let total = 8 + words.len() * 4;
    let hdr = ((total as u32) << 16) | (opcode as u32);
    buf.extend_from_slice(&obj.to_ne_bytes());
    buf.extend_from_slice(&hdr.to_ne_bytes());
    for &w in words {
        buf.extend_from_slice(&w.to_ne_bytes());
    }
}

fn test_s5_shm_seal_and_wayland_fuzz() -> Result<(), String> {
    // 1. memfd F_SEAL_SHRINK blocks shrinking via ftruncate with -EPERM
    let name = b"s5-seal-test\0";
    let mfd = unsafe { syscall2(SYS_MEMFD_CREATE, name.as_ptr() as u64, MFD_ALLOW_SEALING) } as i32;
    if mfd < 0 {
        return Err(format!("memfd_create failed: {mfd}"));
    }
    if unsafe { syscall2(SYS_FTRUNCATE, mfd as u64, 4096) } != 0 {
        let _ = unsafe { syscall1(SYS_CLOSE, mfd as u64) };
        return Err("initial ftruncate(4096) failed".into());
    }
    let add_rc = unsafe { syscall3(SYS_FCNTL, mfd as u64, F_ADD_SEALS, F_SEAL_SHRINK) };
    let seals = unsafe { syscall3(SYS_FCNTL, mfd as u64, F_GET_SEALS, 0) };
    let shrink_rc = unsafe { syscall2(SYS_FTRUNCATE, mfd as u64, 0) };
    if add_rc != 0 || seals < 0 || ((seals as u64) & F_SEAL_SHRINK) == 0 || shrink_rc != EPERM {
        let _ = unsafe { syscall1(SYS_CLOSE, mfd as u64) };
        return Err(format!(
            "memfd F_SEAL_SHRINK check failed: add={add_rc} seals={seals:#x} shrink={shrink_rc}"
        ));
    }

    // 2. Malicious Wayland client: sends valid sealed pool then create_buffer with stride=0xFFFF_FFFF
    let wfd = connect_wayland_unix()?;
    let mut init_wire = Vec::new();
    // wl_display(1).get_registry(new_id=2)
    push_wl_msg(&mut init_wire, 1, 1, &[2]);
    // wl_registry(2).bind(name=2 [wl_shm], iface="wl_shm", version=1, new_id=4)
    {
        let mut bind = Vec::new();
        bind.extend_from_slice(&2u32.to_ne_bytes()); // name = 2 (wl_shm)
        let iface = b"wl_shm\0\0";
        bind.extend_from_slice(&7u32.to_ne_bytes()); // str len including NUL
        bind.extend_from_slice(iface);
        bind.extend_from_slice(&1u32.to_ne_bytes()); // version
        bind.extend_from_slice(&4u32.to_ne_bytes()); // new_id = 4
        let total = 8 + bind.len();
        init_wire.extend_from_slice(&2u32.to_ne_bytes());
        init_wire.extend_from_slice(&(((total as u32) << 16) | 0u32).to_ne_bytes());
        init_wire.extend_from_slice(&bind);
    }
    let _ = unsafe {
        syscall3(
            SYS_WRITE,
            wfd as u64,
            init_wire.as_ptr() as u64,
            init_wire.len() as u64,
        )
    };

    // wl_shm(4).create_pool(new_id=5, size=4096) with SCM_RIGHTS(mfd)
    let mut pool_wire = Vec::new();
    push_wl_msg(&mut pool_wire, 4, 0, &[5, 4096]);
    // Malicious wl_shm_pool(5).create_buffer(id=6, off=0, w=64, h=64, stride=0xFFFF_FFFF, fmt=0)
    push_wl_msg(&mut pool_wire, 5, 0, &[6, 0, 64, 64, u32::MAX, 0]);
    let iov = Iovec {
        iov_base: pool_wire.as_ptr() as u64,
        iov_len: pool_wire.len() as u64,
    };
    let mut ctrl = [0u8; 24];
    ctrl[0..8].copy_from_slice(&20u64.to_ne_bytes());
    ctrl[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
    ctrl[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
    ctrl[16..20].copy_from_slice(&mfd.to_ne_bytes());
    let hdr = Msghdr {
        msg_iov: &iov as *const _ as u64,
        msg_iovlen: 1,
        msg_control: ctrl.as_ptr() as u64,
        msg_controllen: 20,
        ..Default::default()
    };
    let _ = unsafe { syscall3(SYS_SENDMSG, wfd as u64, &hdr as *const _ as u64, 0) };
    let _ = unsafe { syscall1(SYS_CLOSE, mfd as u64) };

    // Wait briefly for compositor to reject & close the malicious connection
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 256];
    let mut peer_closed = false;
    while Instant::now() < deadline {
        let rc = unsafe {
            syscall6(
                SYS_RECVFROM,
                wfd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                MSG_DONTWAIT as u64,
                0,
                0,
            )
        };
        if rc == 0 || (rc < 0 && rc != EAGAIN) {
            peer_closed = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = unsafe { syscall1(SYS_CLOSE, wfd as u64) };
    if !peer_closed {
        return Err("exo_compositor did not disconnect malicious stride-overflow client".into());
    }

    // 3. Verify compositor & fb_server remain 100% healthy for a benign client
    crate::t4_wayland_live_ipc()
        .map_err(|e| format!("healthy wayland follow-up failed after fuzz: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// S6: Live ExoPhoenix 3-Strata switch under active Strate 2 load + ExoLedger
// ─────────────────────────────────────────────────────────────────────────────

fn invoke_phoenix_force() -> Result<PhoenixTimingRecordWire, String> {
    let mut rec = PhoenixTimingRecordWire::default();
    let rc = unsafe {
        syscall3(
            SYS_PHOENIX_FORCE,
            1, // PHOENIX_REASON_FORCE_SYSCALL
            &mut rec as *mut PhoenixTimingRecordWire as u64,
            core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
        )
    };
    if rc != 0 {
        return Err(format!("SYS_PHOENIX_FORCE (523) failed: rc={rc}"));
    }
    if rec.magic != PHOENIX_METRICS_MAGIC {
        return Err(format!("invalid PhoenixTimingRecord magic: {:#x}", rec.magic));
    }
    if (rec.flags & 1) != 0 {
        return Err(format!("ExoPhoenix cycle {} returned DEGRADED=1", rec.cycle_id));
    }
    Ok(rec)
}

fn spawn_strate2_load(stop: Arc<AtomicBool>) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();

    // Load 1: Loopback TCP echo activity (deadlock-free shutdown: client stops first, server drains)
    let s1 = Arc::clone(&stop);
    handles.push(thread::spawn(move || {
        if let Ok(listener) = TcpListener::bind("127.0.0.1:0") {
            let _ = listener.set_nonblocking(true);
            if let Ok(addr) = listener.local_addr() {
                let s_cli = Arc::clone(&s1);
                let cli_done = Arc::new(AtomicBool::new(false));
                let cli_done_w = Arc::clone(&cli_done);
                let cli = thread::spawn(move || {
                    while !s_cli.load(Ordering::Relaxed) {
                        if let Ok(mut stream) =
                            TcpStream::connect_timeout(&addr, Duration::from_millis(200))
                        {
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                            let _ = stream.write_all(b"STRATE2_LOAD_PING");
                            let _ = stream.shutdown(Shutdown::Write);
                            let mut rb = [0u8; 32];
                            let _ = stream.read(&mut rb);
                        }
                        thread::sleep(Duration::from_millis(40));
                    }
                    cli_done_w.store(true, Ordering::Release);
                });
                let mut drain_deadline: Option<Instant> = None;
                while !cli_done.load(Ordering::Acquire) {
                    if s1.load(Ordering::Relaxed) {
                        let dl = *drain_deadline.get_or_insert_with(|| Instant::now() + Duration::from_secs(2));
                        if Instant::now() >= dl {
                            break;
                        }
                    }
                    if let Ok((mut conn, _)) = listener.accept() {
                        let _ = conn.set_read_timeout(Some(Duration::from_millis(200)));
                        let _ = conn.set_write_timeout(Some(Duration::from_millis(200)));
                        let mut rb = [0u8; 64];
                        if let Ok(n) = conn.read(&mut rb) {
                            let _ = conn.write_all(&rb[..n]);
                        }
                    } else {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
                let _ = cli.join();
            }
        }
    }));

    // Load 2: Active Wayland client roundtrips on /run/wayland-0
    let s2 = Arc::clone(&stop);
    handles.push(thread::spawn(move || {
        while !s2.load(Ordering::Relaxed) {
            let _ = crate::t4_wayland_live_ipc();
            thread::sleep(Duration::from_millis(50));
        }
    }));

    // Load 3 & 4: Concurrent userspace compute/allocation workers
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..2 {
        let sw = Arc::clone(&stop);
        let cw = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            let mut v = vec![0u8; 4096];
            while !sw.load(Ordering::Relaxed) {
                for (i, b) in v.iter_mut().enumerate() {
                    *b = (*b).wrapping_add((i & 0xFF) as u8);
                }
                cw.fetch_add(1, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(40));
            }
        }));
    }

    handles
}

fn test_s6_exophoenix_live_and_ledger() -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let workers = spawn_strate2_load(Arc::clone(&stop));
    thread::sleep(Duration::from_millis(40));

    let r1 = invoke_phoenix_force();
    let r2 = invoke_phoenix_force();

    stop.store(true, Ordering::Relaxed);
    for h in workers {
        let _ = h.join();
    }

    let r1 = r1?;
    let r2 = r2?;
    if r2.cycle_id <= r1.cycle_id {
        return Err(format!("non-monotonic cycle_id: {} -> {}", r1.cycle_id, r2.cycle_id));
    }
    if r2.crypto_epoch_id <= r1.crypto_epoch_id {
        return Err(format!(
            "non-monotonic crypto epoch_id: {} -> {}",
            r1.crypto_epoch_id, r2.crypto_epoch_id
        ));
    }

    // Verify SSR readback via SYS_EXOPHOENIX_TEST(0)
    let mut stored = PhoenixTimingRecordWire::default();
    let ssr_rc = unsafe {
        syscall3(
            SYS_EXOPHOENIX_TEST,
            0,
            &mut stored as *mut PhoenixTimingRecordWire as u64,
            core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
        )
    };
    if ssr_rc <= 0 || stored.cycle_id != r2.cycle_id || stored.crypto_epoch_id != r2.crypto_epoch_id {
        return Err(format!("SSR metrics readback mismatch: rc={ssr_rc}"));
    }

    // Verify ExoLedger BLAKE3 chain integrity post-switch
    let ledger = unsafe { syscall3(SYS_EXOPHOENIX_TEST, 1, 0, 0) };
    if ledger <= 0 {
        return Err(format!("ExoLedger chain invalid post-switch: {ledger}"));
    }

    // Verify Wayland still alive after switches
    crate::t4_wayland_live_ipc()
        .map_err(|e| format!("post-switch Wayland check failed: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoPhoenix 3-Strata O(1) Benchmark Hooks (`--phoenix-bench-a` / `--phoenix-bench-b`)
// ─────────────────────────────────────────────────────────────────────────────

pub fn run_phoenix_bench(with_strate2_load: bool) -> ! {
    let label = if with_strate2_load { "CAS_B" } else { "CAS_A" };
    let stop = Arc::new(AtomicBool::new(false));
    let workers = if with_strate2_load {
        let w = spawn_strate2_load(Arc::clone(&stop));
        thread::sleep(Duration::from_millis(60));
        w
    } else {
        Vec::new()
    };

    let mut ok = true;
    for i in 0..5 {
        match invoke_phoenix_force() {
            Ok(rec) => {
                println!(
                    "[PHOENIX_BENCH] {} ITER={} CYCLE={} S0_US={} SLA_US={} EPOCH={}",
                    label,
                    i + 1,
                    rec.cycle_id,
                    rec.delta_s0_us,
                    rec.delta_sla_us,
                    rec.crypto_epoch_id
                );
            }
            Err(e) => {
                eprintln!("[PHOENIX_BENCH] {} ITER={} FAIL: {}", label, i + 1, e);
                ok = false;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }

    stop.store(true, Ordering::Relaxed);
    for h in workers {
        let _ = h.join();
    }

    std::process::exit(if ok { 0 } else { 1 });
}
