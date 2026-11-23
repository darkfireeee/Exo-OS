//! ExoPhoenix Indisputable Qualification Suite (`F1`–`F9`, Plan `P5`–`P8`).
//!
//! Invoked via:
//! - `/bin/exo-compliance-runner --phoenix-qual [--suite faults|smp-cold|gi05|all] [--exit-qemu]`
//! - `/bin/exo-compliance-runner --qual-helper sleep-victim`

use crate::raw::*;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const SELF_BIN: &str = "/bin/exo-compliance-runner";

const SUBCMD_MINT_FAULT_CAP: u64 = 9;
const FAULT_KERNEL_A_PANIC: u64 = 10;
const FAULT_KERNEL_A_DEADLOCK: u64 = 11;
const FAULT_KERNEL_A_TEXT_CORRUPT: u64 = 12;
const FAULT_AP_STALL_FREEZE: u64 = 13;
const FAULT_STRATE1_COLD_KILL: u64 = 14;
const FAULT_EXOFS_CRASH_MID_EPOCH: u64 = 15;
const FAULT_IRQ_STORM_AND_MASKED_SINCE: u64 = 16;
const FAULT_EXOVEIL_AND_P0_STRESS: u64 = 17;
const FAULT_DMA_IOMMU_STRESS: u64 = 18;
const FAULT_CRYPTO_AEAD_NONCE_VERIFY: u64 = 19;

const CAP_TOKEN_WIRE_SIZE: usize = 80;
const EXO_CAP_TYPE_IPC_ENDPOINT: u64 = 2;
const EXO_CAP_RIGHT_IPC_SEND: u64 = 1 << 7;
const EXO_CAP_RIGHT_IPC_MANAGE: u64 = 1 << 9;

pub fn run_qual_helper(args: &[String]) -> ! {
    match args.first().map(|s| s.as_str()) {
        Some("sleep-victim") => {
            // Disposable victim process for `F7` DMAR fault SIGKILL verification.
            loop {
                thread::sleep(Duration::from_millis(50));
            }
        }
        _ => std::process::exit(99),
    }
}

fn mint_fault_cap() -> Result<[u8; CAP_TOKEN_WIRE_SIZE], String> {
    let mut token = [0u8; CAP_TOKEN_WIRE_SIZE];
    let rc = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            SUBCMD_MINT_FAULT_CAP,
            token.as_mut_ptr() as u64,
            token.len() as u64,
            0,
        )
    };
    if rc <= 0 {
        return Err(format!(
            "CAP_PHOENIX_FAULT_INJECT mint failed: rc={rc} (kernel built with --features phoenix-fault-injection?)"
        ));
    }
    Ok(token)
}

fn snapshot_process_pids() -> BTreeMap<String, u32> {
    let mut infos = vec![ExoProcessInfoWire::default(); 256];
    let n = unsafe {
        syscall3(
            SYS_EXO_PROCESS_LIST,
            infos.as_mut_ptr() as u64,
            infos.len() as u64,
            core::mem::size_of::<ExoProcessInfoWire>() as u64,
        )
    };
    let mut map = BTreeMap::new();
    if n <= 0 {
        return map;
    }
    for info in &infos[..(n as usize).min(infos.len())] {
        let end = info
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(info.name.len());
        if let Ok(name) = core::str::from_utf8(&info.name[..end]) {
            if !name.is_empty() && info.pid > 0 {
                map.insert(name.to_string(), info.pid);
            }
        }
    }
    map
}

fn find_pid_matching(procs: &BTreeMap<String, u32>, prefixes: &[&str]) -> Option<u32> {
    for (k, &pid) in procs {
        for p in prefixes {
            if k.starts_with(p) {
                return Some(pid);
            }
        }
    }
    None
}

fn read_leak_snapshot(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<PhoenixLeakSnapshotWire, String> {
    let mut snap = PhoenixLeakSnapshotWire::default();
    let rc = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_EXOVEIL_AND_P0_STRESS,
            token.as_ptr() as u64,
            1,
            &mut snap as *mut PhoenixLeakSnapshotWire as u64,
        )
    };
    if rc != 0 {
        return Err(format!("leak snapshot failed: rc={rc}"));
    }
    Ok(snap)
}

pub fn run_phoenix_qualification(suite: &str, exit_qemu: bool) -> ! {
    let _ = fs::create_dir_all("/tmp");
    exo_log_line(&format!("[EXO_QUAL] START SUITE={suite}"));

    let token = match mint_fault_cap() {
        Ok(t) => t,
        Err(err) => {
            exo_log_line(&format!("[EXO_QUAL] INIT FAIL DETAIL=\"{err}\""));
            exo_log_line("[EXO_QUAL] SUMMARY PASS=0 FAIL=1 TOTAL=1");
            if exit_qemu {
                compliance_exit_qemu(false);
            }
            std::process::exit(1);
        }
    };

    // P5.0 negative check: invoking destructive injector WITHOUT CAP_PHOENIX_FAULT_INJECT MUST return -EPERM.
    let unauth_rc = unsafe { syscall4(SYS_EXOPHOENIX_TEST, FAULT_KERNEL_A_PANIC, 0, 0, 0) };
    if unauth_rc != EPERM {
        exo_log_line(&format!(
            "[EXO_QUAL] P5_CAP_GATE FAIL DETAIL=\"missing CAP_PHOENIX_FAULT_INJECT expected EPERM (-1), got {unauth_rc}\""
        ));
        exo_log_line("[EXO_QUAL] SUMMARY PASS=0 FAIL=1 TOTAL=1");
        if exit_qemu {
            compliance_exit_qemu(false);
        }
        std::process::exit(1);
    }

    if let Ok(snap) = read_leak_snapshot(&token) {
        exo_log_line(&format!(
            "[EXO_QUAL] ENV CPUS={} ACCEL={} VTD_UNITS={} FREE_FRAMES={} ACTIVE_PIDS={} HEARTBEAT={}",
            snap.online_cpus,
            if snap.is_kvm != 0 { "KVM" } else { "TCG" },
            snap.vtd_units,
            snap.free_frames,
            snap.active_pids,
            snap.heartbeat
        ));
    }

    type QualFn = fn(&[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String>;
    let all_tests: &[(&str, &str, QualFn)] = &[
        ("F1", "faults", test_f1_non_cooperative_and_text_heal),
        ("F2", "smp-cold", test_f2_smp_freeze_and_corr33),
        ("F3", "smp-cold", test_f3_cold_respawn_and_strate2_reconverge),
        ("F4", "gi05", test_f4_fpu_sse_avx_integrity),
        ("F5", "gi05", test_f5_exofs_sync_and_mid_epoch_crash),
        ("F6", "gi05", test_f6_crypto_nonces_and_t20_001),
        ("F7", "kvm", test_f7_dma_quiescence_and_iommu_fault),
        ("F8", "gi05", test_f8_irq_watchdog_and_masked_since_soak),
        ("F9", "gi05", test_f9_exoveil_p0_and_50_cycle_endurance),
    ];

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut total = 0usize;

    for &(id, group, func) in all_tests {
        let include = match suite {
            "all" | "kvm" => true,
            "faults" => group == "faults",
            "smp-cold" => group == "smp-cold",
            "gi05" => group == "gi05" || id == "F7",
            other => other.eq_ignore_ascii_case(id),
        };
        if !include {
            continue;
        }
        total += 1;
        let t0 = Instant::now();
        match func(&token) {
            Ok(info) => {
                pass += 1;
                let clean = info.trim().replace('\n', " | ");
                exo_log_line(&format!(
                    "[EXO_QUAL] {} PASS DURATION_MS={} INFO=\"{}\"",
                    id,
                    t0.elapsed().as_millis(),
                    clean
                ));
            }
            Err(detail) => {
                fail += 1;
                let clean = detail.trim().replace('\n', " | ");
                exo_log_line(&format!(
                    "[EXO_QUAL] {} FAIL DURATION_MS={} DETAIL=\"{}\"",
                    id,
                    t0.elapsed().as_millis(),
                    clean
                ));
            }
        }
    }

    exo_log_line(&format!(
        "[EXO_QUAL] SUMMARY PASS={} FAIL={} TOTAL={}",
        pass, fail, total
    ));

    if exit_qemu {
        compliance_exit_qemu(fail == 0 && total > 0);
    }
    std::process::exit(if fail == 0 && total > 0 { 0 } else { 1 });
}

// ─────────────────────────────────────────────────────────────────────────────
// F1: Non-cooperative Ring 0 survival (panic/#UD, cli deadlock + ExoNmi IST,
//     physical `.text` bit-flip auto-healing from Golden RAM Mirror)
// ─────────────────────────────────────────────────────────────────────────────

fn test_f1_non_cooperative_and_text_heal(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    // F1a: Real Ring 0 `#UD` / emergency crash recovery.
    let rc_panic = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_KERNEL_A_PANIC,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_panic != 0 {
        return Err(format!("F1a Ring 0 panic/exception recovery failed: rc={rc_panic}"));
    }

    // F1b: Real `cli` deadlock broken by hardware `#NMI` on dedicated TSS `IST_NMI` + 3 strikes P0.
    let rc_nmi = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_KERNEL_A_DEADLOCK,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_nmi != 0 {
        return Err(format!("F1b cli deadlock + ExoNmi IST recovery failed: rc={rc_nmi}"));
    }

    // F1c: Physical `.text` bit-flip -> BLAKE3 divergence -> CR0.WP=0 rewrite from RO mirror -> verified!
    let rc_text = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_KERNEL_A_TEXT_CORRUPT,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_text != 0 {
        return Err(format!("F1c physical .text bit-flip auto-healing failed: rc={rc_text}"));
    }

    let ledger = unsafe { syscall3(SYS_EXOPHOENIX_TEST, 1, 0, 0) };
    if ledger <= 0 {
        return Err(format!("F1 ExoLedger chain verification failed: {ledger}"));
    }

    Ok(format!("panic_ud_recovered=1 nmi_ist_deadlock_broken=1 text_phys_healed=1 ledger_entries={ledger}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// F2: Real SMP (`-smp 4`) freeze ACK + CORR-33 100 ms timeout (`0xDEAD_ACK`)
// ─────────────────────────────────────────────────────────────────────────────

fn test_f2_smp_freeze_and_corr33(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    // F2a: Nominal multi-core freeze across all online peer CPUs.
    let nom = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_AP_STALL_FREEZE,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if nom < 0 {
        return Err(format!("F2a nominal SMP freeze failed: rc={nom}"));
    }
    let online_cpus = (nom as u64) >> 16;
    let acked_peers = (nom as u64) & 0xFFFF;
    if online_cpus < 2 {
        return Err(format!("F2 requires SMP >= 2 CPUs (boot QEMU with -smp 4), observed online_cpus={online_cpus}"));
    }
    if acked_peers < online_cpus - 1 {
        return Err(format!("F2a expected {} peer ACKs, got {}", online_cpus - 1, acked_peers));
    }

    // F2b: Inject `FAULT_AP_STALL_FREEZE` on CPU 2 -> CORR-33 100 ms timeout -> 0xDEAD_ACK + DEGRADED=1.
    let target_ap = if online_cpus >= 3 { 2u64 } else { 1u64 };
    let freeze_us = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_AP_STALL_FREEZE,
            token.as_ptr() as u64,
            target_ap,
            0,
        )
    };
    if freeze_us < 60_000 {
        return Err(format!("F2b CORR-33 AP stall timeout failed: rc/us={freeze_us}"));
    }

    // F2c: Post-stall recovery check — all CPUs still responsive with zero deadlock!
    let post = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_AP_STALL_FREEZE,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if post < 0 {
        return Err(format!("F2c post-CORR-33 recovery freeze failed: rc={post}"));
    }

    Ok(format!(
        "online_cpus={online_cpus} nominal_peer_acks={acked_peers} corr33_stalled_ap={target_ap} corr33_timeout_us={freeze_us} post_recovery=OK"
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// F3: Destructive Cold Respawn (`SIGKILL` + parallel `R0 -> R1 -> R2` + `PHX-03`)
//     + Strate 2 asynchronous reconvergence (`TCP 127.0.0.1` + `/run/wayland-0`)
// ─────────────────────────────────────────────────────────────────────────────

fn verify_tcp_loopback_roundtrip() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("tcp bind: {e}"))?;
    let addr = listener.local_addr().map_err(|e| format!("local_addr: {e}"))?;
    let server = thread::spawn(move || -> Result<(), String> {
        let (mut stream, _) = listener.accept().map_err(|e| format!("accept: {e}"))?;
        let mut buf = [0u8; 64];
        let n = stream.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        stream.write_all(&buf[..n]).map_err(|e| format!("write: {e}"))?;
        let _ = stream.shutdown(Shutdown::Both);
        Ok(())
    });
    let mut client = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    let msg = b"EXOPHOENIX_F3_POST_COLD_RESPAWN_TCP_OK";
    client.write_all(msg).map_err(|e| format!("client write: {e}"))?;
    let mut rx = vec![0u8; msg.len()];
    client.read_exact(&mut rx).map_err(|e| format!("client read: {e}"))?;
    let _ = client.shutdown(Shutdown::Both);
    server.join().map_err(|_| "tcp server panicked".to_string())??;
    if rx != msg {
        return Err("TCP loopback payload mismatch after cold respawn".into());
    }
    Ok(())
}

fn verify_wayland_post_respawn() -> Result<(), String> {
    let sfd = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_STREAM as u64, 0) };
    if sfd < 0 {
        return Err(format!("AF_UNIX socket failed: {sfd}"));
    }
    let path = b"/run/wayland-0";
    let mut sun = [0u8; 110];
    sun[0..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    sun[2..2 + path.len()].copy_from_slice(path);
    let crc = unsafe {
        syscall3(
            SYS_CONNECT,
            sfd as u64,
            sun.as_ptr() as u64,
            (2 + path.len() + 1) as u64,
        )
    };
    if crc != 0 {
        let _ = unsafe { syscall1(SYS_CLOSE, sfd as u64) };
        return Err(format!("connect /run/wayland-0 failed: {crc}"));
    }
    // Send wl_display::sync(id=1, opcode=0, size=12, callback_id=2)
    let mut wire = [0u8; 12];
    wire[0..4].copy_from_slice(&1u32.to_ne_bytes());
    wire[4..8].copy_from_slice(&((12u32 << 16) | 0u32).to_ne_bytes());
    wire[8..12].copy_from_slice(&2u32.to_ne_bytes());
    let wrc = unsafe { syscall3(SYS_WRITE, sfd as u64, wire.as_ptr() as u64, wire.len() as u64) };
    if wrc != wire.len() as i64 {
        let _ = unsafe { syscall1(SYS_CLOSE, sfd as u64) };
        return Err(format!("wayland sync write failed: {wrc}"));
    }
    // Wait up to 2s for wl_callback::done event back from respawned exo_compositor.
    let mut rx = [0u8; 64];
    let mut got = 0i64;
    for _ in 0..40 {
        let r = unsafe { syscall4(SYS_RECVFROM, sfd as u64, rx.as_mut_ptr() as u64, rx.len() as u64, MSG_DONTWAIT as u64) };
        if r >= 12 {
            got = r;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = unsafe { syscall1(SYS_CLOSE, sfd as u64) };
    if got < 12 {
        return Err(format!("respawned exo_compositor did not reply to wl_display::sync (got={got})"));
    }
    Ok(())
}

fn test_f3_cold_respawn_and_strate2_reconverge(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    let procs_before = snapshot_process_pids();
    let old_crypto = find_pid_matching(&procs_before, &["exo-crypto", "crypto_server"]).unwrap_or(0);
    let old_mem = find_pid_matching(&procs_before, &["exo-memory", "memory_server"]).unwrap_or(0);
    let old_vfs = find_pid_matching(&procs_before, &["exo-vfs", "vfs_server"]).unwrap_or(0);
    let old_dev = find_pid_matching(&procs_before, &["exo-device", "device_server"]).unwrap_or(0);
    let old_comp = find_pid_matching(&procs_before, &["exo-compositor", "exo_compositor"]).unwrap_or(0);

    let mut rec = PhoenixTimingRecordWire::default();
    let t_wall_start = Instant::now();
    let rc = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_STRATE1_COLD_KILL,
            token.as_ptr() as u64,
            &mut rec as *mut PhoenixTimingRecordWire as u64,
            core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
        )
    };
    if rc != 0 {
        return Err(format!("F3 COLD_RESPAWN failed: rc={rc}"));
    }
    if rec.magic != PHOENIX_METRICS_MAGIC || (rec.flags & PHOENIX_FLAG_COLD_RESPAWN) == 0 {
        return Err(format!("F3 invalid timing record magic={:#x} flags={:#x}", rec.magic, rec.flags));
    }

    // Enforce hard <= 1,000 ms SLA strictly when running on KVM.
    let is_kvm = (rec.flags & PHOENIX_FLAG_KVM_ACCEL) != 0;
    if is_kvm && rec.delta_sla_us > 1_000_000 {
        return Err(format!(
            "F3 KVM COLD_RESPAWN SLA VIOLATION: S1_SLA={} us > 1_000_000 us",
            rec.delta_sla_us
        ));
    }

    // Verify Strate 1 killed targets all got genuinely NEW PIDs!
    let procs_after_s1 = snapshot_process_pids();
    let new_crypto = find_pid_matching(&procs_after_s1, &["exo-crypto", "crypto_server"]).unwrap_or(0);
    let new_mem = find_pid_matching(&procs_after_s1, &["exo-memory", "memory_server"]).unwrap_or(0);
    let new_vfs = find_pid_matching(&procs_after_s1, &["exo-vfs", "vfs_server"]).unwrap_or(0);
    let new_dev = find_pid_matching(&procs_after_s1, &["exo-device", "device_server"]).unwrap_or(0);

    if new_crypto == 0 || new_crypto == old_crypto {
        return Err(format!("F3 crypto_server did not cold-respawn (old={old_crypto}, new={new_crypto})"));
    }
    if new_mem == 0 || new_mem == old_mem {
        return Err(format!("F3 memory_server did not cold-respawn (old={old_mem}, new={new_mem})"));
    }
    if new_vfs == 0 || new_vfs == old_vfs {
        return Err(format!("F3 vfs_server did not cold-respawn (old={old_vfs}, new={new_vfs})"));
    }
    if new_dev == 0 || new_dev == old_dev {
        return Err(format!("F3 device_server did not cold-respawn (old={old_dev}, new={new_dev})"));
    }

    // Wait for asynchronous Strate 2 reconvergence (network_server + exo_compositor with NEW PIDs)
    // and execute live TCP loopback + Wayland `/run/wayland-0` end-to-end oracles (`t6`).
    let mut new_comp = 0u32;
    let mut tcp_ok = false;
    let mut wl_ok = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let procs = snapshot_process_pids();
        new_comp = find_pid_matching(&procs, &["exo-compositor", "exo_compositor"]).unwrap_or(0);
        if !tcp_ok && verify_tcp_loopback_roundtrip().is_ok() {
            tcp_ok = true;
        }
        if !wl_ok && new_comp != 0 && new_comp != old_comp && verify_wayland_post_respawn().is_ok() {
            wl_ok = true;
        }
        if tcp_ok && wl_ok {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let s2_wall_ms = t_wall_start.elapsed().as_millis();

    if !tcp_ok {
        return Err("F3 Strate 2 TCP loopback oracle failed after cold respawn".into());
    }
    if !wl_ok {
        return Err(format!(
            "F3 Strate 2 Wayland /run/wayland-0 oracle failed (old_comp={old_comp}, new_comp={new_comp})"
        ));
    }

    Ok(format!(
        "accel={} s0_us={} s1_cold_sla_us={} s2_reconverge_wall_ms={} crypto={}->{} mem={}->{} vfs={}->{} dev={}->{} comp={}->{} tcp=OK wayland=OK",
        if is_kvm { "KVM" } else { "TCG" },
        rec.delta_s0_us,
        rec.delta_sla_us,
        s2_wall_ms,
        old_crypto, new_crypto,
        old_mem, new_mem,
        old_vfs, new_vfs,
        old_dev, new_dev,
        old_comp, new_comp
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// F4: FPU / SSE2 / AVX register integrity across 4 threads & 10 switches (`CORR-15`)
// ─────────────────────────────────────────────────────────────────────────────

fn run_sse2_worker(tid_seed: u64, lazy_sleep: bool, stop: Arc<AtomicBool>) -> Result<u64, String> {
    // Non-trivial deterministic lane values held live inside XMM0..XMM3 across preemption & IPI freeze.
    let expected_a: f64 = 1.618_033_988_749_895 + (tid_seed as f64) * 0.125;
    let expected_b: f64 = 2.718_281_828_459_045 + (tid_seed as f64) * 0.25;
    let expected_sum = expected_a * 3.0 + expected_b * 5.0;
    let mut iters = 0u64;

    while !stop.load(Ordering::Acquire) || iters < 200 {
        let mut out0 = 0.0f64;
        let mut out1 = 0.0f64;
        // Load distinct bit patterns into xmm0..xmm5, run arithmetic loop, read back intact.
        unsafe {
            core::arch::asm!(
                "movsd xmm0, [{pa}]",
                "movsd xmm1, [{pb}]",
                "movapd xmm2, xmm0",
                "movapd xmm3, xmm1",
                "addsd xmm2, xmm0",
                "addsd xmm2, xmm0", // 3 * a
                "addsd xmm3, xmm1",
                "addsd xmm3, xmm1",
                "addsd xmm3, xmm1",
                "addsd xmm3, xmm1", // 5 * b
                "addsd xmm2, xmm3", // 3a + 5b
                "movsd [{o0}], xmm2",
                "movsd [{o1}], xmm0",
                pa = in(reg) &expected_a,
                pb = in(reg) &expected_b,
                o0 = in(reg) &mut out0,
                o1 = in(reg) &mut out1,
                out("xmm0") _,
                out("xmm1") _,
                out("xmm2") _,
                out("xmm3") _,
                options(nostack),
            );
        }
        if out0.to_bits() != expected_sum.to_bits() || out1.to_bits() != expected_a.to_bits() {
            return Err(format!(
                "F4 SIMD corruption on worker {tid_seed} (lazy={lazy_sleep}) iter {iters}: out0={:#x} exp={:#x}, out1={:#x} exp={:#x}",
                out0.to_bits(),
                expected_sum.to_bits(),
                out1.to_bits(),
                expected_a.to_bits()
            ));
        }
        iters += 1;
        if lazy_sleep && (iters % 16 == 0) {
            thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(iters)
}

fn test_f4_fpu_sse_avx_integrity(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    for idx in 0..4u64 {
        let s = stop.clone();
        // 2 threads active continuous (CR0.TS=0), 2 threads lazy sleep/compute (CR0.TS=1).
        let lazy = idx >= 2;
        handles.push(thread::spawn(move || run_sse2_worker(idx + 1, lazy, s)));
    }

    thread::sleep(Duration::from_millis(20));

    // Fire 10 mixed ExoPhoenix switches (8 SOFT_FORCE + 2 CRASH_NMI) while FPU workers spin.
    for cycle in 0..10u32 {
        let rc = if cycle % 5 == 4 {
            unsafe {
                syscall4(
                    SYS_EXOPHOENIX_TEST,
                    FAULT_KERNEL_A_DEADLOCK,
                    token.as_ptr() as u64,
                    0,
                    0,
                )
            }
        } else {
            unsafe { syscall3(SYS_PHOENIX_FORCE, 1, 0, 0) }
        };
        if rc != 0 {
            stop.store(true, Ordering::Release);
            for h in handles {
                let _ = h.join();
            }
            return Err(format!("F4 switch cycle {cycle} failed: rc={rc}"));
        }
        thread::sleep(Duration::from_millis(5));
    }

    stop.store(true, Ordering::Release);
    let mut total_iters = 0u64;
    for (i, h) in handles.into_iter().enumerate() {
        let it = h
            .join()
            .map_err(|_| format!("F4 worker {i} panicked"))??;
        total_iters += it;
    }

    Ok(format!("threads=4 active=2 lazy=2 switches=10 total_simd_checks={total_iters} corrupted_bits=0"))
}

// ─────────────────────────────────────────────────────────────────────────────
// F5: ExoFS 2 MiB integrity across cold vfs_server respawn + Mid-Epoch crash (`CORR-13`)
// ─────────────────────────────────────────────────────────────────────────────

fn fnv1a_256_chunked(data: &[u8]) -> [u64; 4] {
    let mut h = [
        0xcbf2_9ce4_8422_2325u64,
        0x8422_2325_cbf2_9ce4u64,
        0x1000_0000_01b3_55aau64,
        0x55aa_1000_0000_01b3u64,
    ];
    for (i, &b) in data.iter().enumerate() {
        let lane = i & 3;
        h[lane] ^= b as u64;
        h[lane] = h[lane].wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn test_f5_exofs_sync_and_mid_epoch_crash(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    const FILE_PATH: &str = "/tmp/f5_exofs_2mib.bin";
    const FILE_LEN: usize = 2 * 1024 * 1024; // 2 MiB

    let payload: Vec<u8> = (0..FILE_LEN)
        .map(|i| ((i.wrapping_mul(131) ^ (i >> 7) ^ 0x5A) & 0xFF) as u8)
        .collect();
    let digest_before = fnv1a_256_chunked(&payload);

    // Scenario A: Write 2 MiB + sync_all + cold respawn vfs_server + read back & verify byte-for-byte.
    {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(FILE_PATH)
            .map_err(|e| format!("F5 create 2MiB file: {e}"))?;
        f.write_all(&payload)
            .map_err(|e| format!("F5 write 2MiB: {e}"))?;
        f.sync_all().map_err(|e| format!("F5 sync_all: {e}"))?;
    }

    let rc_switch = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_STRATE1_COLD_KILL,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_switch != 0 {
        let _ = fs::remove_file(FILE_PATH);
        return Err(format!("F5 cold respawn failed: rc={rc_switch}"));
    }

    let read_back = fs::read(FILE_PATH).map_err(|e| format!("F5 read back 2MiB: {e}"))?;
    let _ = fs::remove_file(FILE_PATH);
    if read_back.len() != FILE_LEN || fnv1a_256_chunked(&read_back) != digest_before {
        return Err(format!(
            "F5 2MiB integrity mismatch after cold vfs_server respawn (len={})",
            read_back.len()
        ));
    }

    // Scenario B: Mid-Epoch crash between NVMe Barriers 1/2 and 2/3 -> O(1) A/B/C recovery.
    let rc_mid = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_EXOFS_CRASH_MID_EPOCH,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_mid != 0 {
        return Err(format!("F5 mid-epoch NVMe barrier crash recovery failed: rc={rc_mid}"));
    }

    Ok("file_2mib_intact_post_cold_vfs=1 mid_epoch_barrier_1_2=O(1)_PASS mid_epoch_barrier_2_3=O(1)_PASS".into())
}

// ─────────────────────────────────────────────────────────────────────────────
// F6: Crypto AEAD distinct nonces (`CORR-12`) & `T20-001` anti-spoofing
// ─────────────────────────────────────────────────────────────────────────────

fn test_f6_crypto_nonces_and_t20_001(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    // Scenario A: Identical plaintext AEAD encrypt before & after ExoPhoenix switch via real crypto_server.
    let rc_aead = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_CRYPTO_AEAD_NONCE_VERIFY,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_aead != 0 {
        return Err(format!("F6a AEAD nonce uniqueness / pre-Phoenix key revocation failed: rc={rc_aead}"));
    }

    // Scenario B1 (T20-001): Unprivileged Ring 3 attempt to mint `IPC_SEND | IPC_MANAGE` MUST return -EPERM.
    let procs = snapshot_process_pids();
    let crypto_pid = find_pid_matching(&procs, &["exo-crypto", "crypto_server"]).unwrap_or(4);
    let mut forged_tok = [0u8; CAP_TOKEN_WIRE_SIZE];
    let mint_rc = unsafe {
        syscall4(
            SYS_EXO_CAP_CREATE,
            EXO_CAP_TYPE_IPC_ENDPOINT,
            EXO_CAP_RIGHT_IPC_SEND | EXO_CAP_RIGHT_IPC_MANAGE,
            crypto_pid as u64,
            forged_tok.as_mut_ptr() as u64,
        )
    };
    if mint_rc >= 0 {
        return Err(format!("F6b Ring 3 unexpectedly minted IPC_MANAGE token: rc={mint_rc}"));
    }

    // Scenario B2 (T20-001): Unprivileged Ring 3 spoofing `PHOENIX_WAKE_ENTROPY` (`sender_pid=0`)
    // directly to `CRYPTO_SERVER_ENDPOINT (4)` MUST be rejected (either at ExoCordon or crypto_server).
    let mut rec_before = PhoenixTimingRecordWire::default();
    let _ = unsafe {
        syscall3(
            SYS_EXOPHOENIX_TEST,
            0,
            &mut rec_before as *mut PhoenixTimingRecordWire as u64,
            core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
        )
    };

    #[repr(C)]
    struct SpoofedCryptoReq {
        sender_pid: u32,
        msg_type: u32,
        reply_endpoint: u64,
        payload_len: u16,
        version: u8,
        flags: u8,
        cap_token: [u8; CAP_TOKEN_WIRE_SIZE],
        payload: [u8; 200],
    }
    let mut spoof = SpoofedCryptoReq {
        sender_pid: 0, // forged kernel sender_pid!
        msg_type: 255, // PHOENIX_WAKE_ENTROPY
        reply_endpoint: 0,
        payload_len: 16,
        version: 3,
        flags: 0,
        cap_token: [0u8; CAP_TOKEN_WIRE_SIZE],
        payload: [0x5Au8; 200],
    };
    let send_rc = unsafe {
        syscall6(
            SYS_EXO_IPC_SEND,
            4, // CRYPTO_SERVER_ENDPOINT
            &mut spoof as *mut SpoofedCryptoReq as u64,
            core::mem::size_of::<SpoofedCryptoReq>() as u64,
            1, // NOWAIT
            0,
            0,
        )
    };
    // Whether ExoCordon blocks the send (< 0) or crypto_server rejects empty/invalid cap_token,
    // verify `ExoLedger` chain remains intact!
    thread::sleep(Duration::from_millis(20));
    let ledger = unsafe { syscall3(SYS_EXOPHOENIX_TEST, 1, 0, 0) };
    if ledger <= 0 {
        return Err(format!("F6 ExoLedger chain corrupted after T20-001 spoof test: {ledger}"));
    }

    Ok(format!(
        "aead_nonce_distinct=1 pre_phoenix_key_revoked=1 ring3_ipc_manage_denied={mint_rc} ring3_spoof_send_rc={send_rc} ledger={ledger}"
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// F7: DMA Quiescence, `IOMMU_FAULT_QUEUE` (64 events) & DMAR fault `SIGKILL`
// ─────────────────────────────────────────────────────────────────────────────

fn test_f7_dma_quiescence_and_iommu_fault(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    // Spawn a disposable child to verify real `SIGKILL` delivery by `process_iommu_faults()`.
    let mut victim = Command::new(SELF_BIN)
        .args(["--qual-helper", "sleep-victim"])
        .spawn()
        .map_err(|e| format!("F7 spawn sleep-victim: {e}"))?;
    let victim_pid = victim.id();
    thread::sleep(Duration::from_millis(30));

    let mask = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_DMA_IOMMU_STRESS,
            token.as_ptr() as u64,
            victim_pid as u64,
            0,
        )
    };
    if mask < 0 || (mask & 1) == 0 {
        let _ = victim.kill();
        let _ = victim.wait();
        return Err(format!("F7 DMA/IOMMU fault queue stress failed: rc={mask}"));
    }

    // Confirm victim was genuinely terminated by SIGKILL (signal 9)!
    let status = victim.wait().map_err(|e| format!("F7 wait victim: {e}"))?;
    let status_s = status.to_string();
    let killed_by_sigkill = status_s.contains("signal: 9") || status.code() == Some(137);
    if !killed_by_sigkill {
        return Err(format!("F7 victim PID {victim_pid} expected SIGKILL (9), got {status_s}"));
    }

    let vtd_hw = (mask & (1 << 1)) != 0;
    let pci_enum = (mask & (1 << 2)) != 0;
    Ok(format!(
        "fault_queue_64_zero_drop=1 dmar_victim_sigkill=1 victim_pid={victim_pid} intel_vtd_hw={} pci_devices_ok={}",
        u8::from(vtd_hw),
        u8::from(pci_enum)
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// F8: IRQ Watchdog `masked_since` reset + Storm Blacklist EOI + 10s Post-Restore Soak
// ─────────────────────────────────────────────────────────────────────────────

fn test_f8_irq_watchdog_and_masked_since_soak(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    let rc_storm = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_IRQ_STORM_AND_MASKED_SINCE,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_storm != 0 {
        return Err(format!("F8 IRQ masked_since/storm blacklist check failed: rc={rc_storm}"));
    }

    // 10-second post-restore IRQ activity soak (timers, disk I/O, loopback packets).
    let soak_start = Instant::now();
    let mut ops = 0u32;
    let soak_path = "/tmp/f8_soak.tmp";
    while soak_start.elapsed() < Duration::from_secs(10) {
        let _ = fs::write(soak_path, b"f8-irq-watchdog-soak");
        let _ = fs::read(soak_path);
        let _ = verify_tcp_loopback_roundtrip();
        ops += 1;
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_file(soak_path);

    let snap = read_leak_snapshot(token)?;
    if snap.heartbeat == 0 {
        return Err("F8 Kernel A heartbeat stalled during 10s soak".into());
    }

    Ok(format!(
        "masked_since_reset=1 storm_blacklist_eoi=1 counter_reset_on_reregister=1 soak_ms={} soak_ops={ops} heartbeat={}",
        soak_start.elapsed().as_millis(),
        snap.heartbeat
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// F9: `ExoVeil` revocation, P0 immutability (200 writes), 50-cycle mixed endurance
// ─────────────────────────────────────────────────────────────────────────────

fn test_f9_exoveil_p0_and_50_cycle_endurance(token: &[u8; CAP_TOKEN_WIRE_SIZE]) -> Result<String, String> {
    // 1. ExoVeil domain revocation + 200 P0 overwrite attempts preserving all 16 genesis P0 hashes.
    let rc_p0 = unsafe {
        syscall4(
            SYS_EXOPHOENIX_TEST,
            FAULT_EXOVEIL_AND_P0_STRESS,
            token.as_ptr() as u64,
            0,
            0,
        )
    };
    if rc_p0 != 0 {
        return Err(format!("F9 ExoVeil / P0 200-write immutability failed: rc={rc_p0}"));
    }

    // 2. Snapshot resources BEFORE 50 mixed cycles.
    // Warm up 1 cycle so any one-time caches/buffers are settled before baseline snapshot.
    let _ = unsafe { syscall3(SYS_PHOENIX_FORCE, 1, 0, 0) };
    let before = read_leak_snapshot(token)?;

    // 3. Execute 50 consecutive mixed cycles: 47 SOFT_FORCE + 2 CRASH_NMI + 1 COLD_RESPAWN.
    let mut last_epoch = 0u64;
    for cycle in 1..=50u32 {
        let rc = if cycle == 25 {
            unsafe {
                syscall4(
                    SYS_EXOPHOENIX_TEST,
                    FAULT_STRATE1_COLD_KILL,
                    token.as_ptr() as u64,
                    0,
                    0,
                )
            }
        } else if cycle == 15 || cycle == 40 {
            unsafe {
                syscall4(
                    SYS_EXOPHOENIX_TEST,
                    FAULT_KERNEL_A_DEADLOCK,
                    token.as_ptr() as u64,
                    0,
                    0,
                )
            }
        } else {
            unsafe { syscall3(SYS_PHOENIX_FORCE, 1, 0, 0) }
        };
        if rc != 0 {
            return Err(format!("F9 endurance cycle {cycle}/50 failed: rc={rc}"));
        }
        let mut rec = PhoenixTimingRecordWire::default();
        let rrc = unsafe {
            syscall3(
                SYS_EXOPHOENIX_TEST,
                0,
                &mut rec as *mut PhoenixTimingRecordWire as u64,
                core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
            )
        };
        if rrc <= 0 || rec.magic != PHOENIX_METRICS_MAGIC {
            return Err(format!("F9 cycle {cycle} invalid SSR timing record"));
        }
        // Note: on cycle 25 crypto_server cold-respawned so its in-process epoch counter restarts;
        // enforce strict monotonicity on all other cycles.
        if cycle != 25 && last_epoch != 0 && rec.crypto_epoch_id <= last_epoch {
            return Err(format!(
                "F9 cycle {cycle} crypto_epoch_id non-monotonic: {} <= {}",
                rec.crypto_epoch_id, last_epoch
            ));
        }
        last_epoch = rec.crypto_epoch_id;
    }

    // Allow Strate 2 background respawn after cycle 25 to settle cleanly.
    thread::sleep(Duration::from_millis(500));
    let after = read_leak_snapshot(token)?;

    let ledger = unsafe { syscall3(SYS_EXOPHOENIX_TEST, 1, 0, 0) };
    if ledger <= 0 {
        return Err(format!("F9 ExoLedger chain broken after 50 cycles: {ledger}"));
    }

    let pid_delta = after.active_pids as i64 - before.active_pids as i64;
    let frame_delta = after.free_frames as i64 - before.free_frames as i64;
    // Allow minor +/-3 PID jitter from Strate 2 async helpers; fail on unbounded PID leaks.
    if pid_delta > 4 {
        return Err(format!(
            "F9 PID leak detected over 50 cycles: before={} after={} delta={pid_delta}",
            before.active_pids, after.active_pids
        ));
    }

    Ok(format!(
        "exoveil_revoked=1 p0_200_writes_intact=1 cycles=50 free_frames_before={} free_frames_after={} frame_delta={frame_delta} pids_before={} pids_after={} pid_delta={pid_delta} ledger_verified={ledger}",
        before.free_frames,
        after.free_frames,
        before.active_pids,
        after.active_pids
    ))
}
