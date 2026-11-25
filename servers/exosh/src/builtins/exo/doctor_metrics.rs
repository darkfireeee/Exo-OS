//! `exo doctor` and `exo metrics` (`SYS_EXO_PERF_READ = 342`).
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;

use super::phoenix_audit::{PhoenixTimingRecordWire, PHOENIX_METRICS_MAGIC};

pub(super) fn ipc_service_online(name: &[u8]) -> bool {
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_LOOKUP,
            name.as_ptr() as u64,
            name.len() as u64,
            0,
        )
    };
    rc > 0
}

pub(super) fn path_exists_probe(path_cstr: &[u8]) -> bool {
    let mut st = LinuxStat::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_STAT,
            path_cstr.as_ptr() as u64,
            &mut st as *mut LinuxStat as u64,
        )
    };
    rc == 0
}

pub(super) fn run_exo_doctor() {
    write_all(b"=== Exo-OS Doctor : Live System Diagnostics ===\n\n");
    let mut passed = 0u64;
    let total = 8u64;

    // 1. Kernel Syscall / Perf Telemetry
    let total_calls = unsafe {
        syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_TOTAL, 0)
    };
    if total_calls > 0 {
        passed += 1;
        write_all(b"[PASS] 1. Kernel Syscall/Perf     : live (dispatch_total=");
        write_u64(total_calls as u64);
        write_all(b")\n");
    } else {
        write_all(b"[FAIL] 1. Kernel Syscall/Perf     : SYS_EXO_PERF_READ failed (rc=");
        write_i64(total_calls);
        write_all(b")\n");
    }

    // 2. ExoPhoenix Engine
    let phoenix = unsafe { syscall::syscall0(syscall::SYS_EXO_PHOENIX_STATE_GET) };
    if phoenix >= 1 {
        passed += 1;
        write_all(b"[PASS] 2. ExoPhoenix Engine       : live (state=");
        write_i64(phoenix);
        let mut rec = PhoenixTimingRecordWire::zeroed();
        let mrc = unsafe {
            syscall::syscall3(
                syscall::SYS_EXOPHOENIX_TEST,
                0,
                &mut rec as *mut PhoenixTimingRecordWire as u64,
                core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
            )
        };
        if mrc > 0 && rec.magic == PHOENIX_METRICS_MAGIC {
            write_all(b", last_cycle=");
            write_u64(rec.cycle_id as u64);
            write_all(b", S0=");
            write_u64(rec.delta_s0_us);
            write_all(b"us, S0+S1_SLA=");
            write_u64(rec.delta_sla_us);
            write_all(b"us");
        }
        write_all(b")\n");
    } else {
        write_all(b"[WARN] 2. ExoPhoenix Engine       : unavailable (rc=");
        write_i64(phoenix);
        write_all(b")\n");
    }

    // 3. ExoShield Security Service
    if ipc_service_online(b"exo_shield") {
        passed += 1;
        write_all(b"[PASS] 3. ExoShield Service       : IPC endpoint online\n");
    } else {
        write_all(b"[WARN] 3. ExoShield Service       : IPC endpoint offline\n");
    }

    // 4. ExoFS Root & Dynamic Linker (honest check across pure & compat profiles)
    if path_exists_probe(b"/bin/exosh\0") && path_exists_probe(b"/sbin/exo-init-server\0") {
        passed += 1;
        if path_exists_probe(b"/lib/ld-exo.so\0") {
            write_all(b"[PASS] 4. ExoFS & Dynamic Linker  : /bin/exosh and /lib/ld-exo.so present (profile=compat)\n");
        } else {
            write_all(b"[PASS] 4. ExoFS Root Integrity    : /bin/exosh and /sbin/exo-init-server present (profile=pure, 0 C/libc)\n");
        }
    } else {
        write_all(b"[FAIL] 4. ExoFS & Dynamic Linker  : rootfs artifacts missing\n");
    }

    // 5. Wayland Compositor Socket (S5B: exo_compositor Strate 2 + fb_server Strate 1)
    if path_exists_probe(b"/run/wayland-0\0") && ipc_service_online(b"fb_server") {
        passed += 1;
        if ipc_service_online(b"exo_compositor") {
            write_all(b"[PASS] 5. Wayland / Compositor    : /run/wayland-0 ready (exo_compositor -> fb_server)\n");
        } else {
            write_all(b"[PASS] 5. Wayland / fb_server     : /run/wayland-0 ready\n");
        }
    } else {
        write_all(b"[WARN] 5. Wayland / Compositor    : compositor socket not ready\n");
    }

    // 6. ExoNet Network Stack
    let net_sock = unsafe { syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, 2, 0) };
    if net_sock >= 0 {
        let _ = close_fd(net_sock);
        passed += 1;
        write_all(b"[PASS] 6. Network Stack (ExoNet)  : network_server live (AF_INET OK)\n");
    } else {
        write_all(b"[WARN] 6. Network Stack (ExoNet)  : socket(AF_INET) returned errno ");
        write_i64(net_sock);
        write_all(b"\n");
    }

    // 7. Audio Service
    if ipc_service_online(b"audio_server") {
        passed += 1;
        write_all(b"[PASS] 7. Audio Service           : audio_server IPC endpoint online\n");
    } else {
        write_all(b"[WARN] 7. Audio Service           : audio_server offline\n");
    }

    // 8. USB Service
    if ipc_service_online(b"usb_server") {
        passed += 1;
        write_all(b"[PASS] 8. USB Service             : usb_server IPC endpoint online\n");
    } else {
        write_all(b"[WARN] 8. USB Service             : usb_server offline\n");
    }

    write_all(b"\nSystem Health: ");
    write_u64(passed);
    write_all(b"/");
    write_u64(total);
    write_all(b" checks passed\n");
}

pub(super) fn print_exo_metrics() {
    let total_calls = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_TOTAL, 0) };
    let fast_path   = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_FAST_PATH, 0) };
    let slow_path   = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_SLOW_PATH, 0) };
    let enosys      = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_ENOSYS, 0) };

    let sent     = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_MESSAGES_SENT, 0) };
    let received = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_MESSAGES_RECEIVED, 0) };
    let dropped  = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_MESSAGES_DROPPED, 0) };
    let rpc      = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_RPC_CALLS, 0) };
    let timeouts = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_RPC_TIMEOUTS, 0) };
    let waits    = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_FUTEX_WAITS, 0) };
    let wakes    = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_FUTEX_WAKES, 0) };

    let mut out = [0u8; 1024];
    let mut len = 0usize;
    push_bytes(&mut out, &mut len, b"=== Exo-OS Micro-Kernel Performance Telemetry (SYS_EXO_PERF_READ) ===\n\n");
    push_bytes(&mut out, &mut len, b"Syscall Dispatch Telemetry:\n");
    push_bytes(&mut out, &mut len, b"  Total Invocations      : "); push_i64(&mut out, &mut len, total_calls); push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Fast-Path (Direct)     : "); push_i64(&mut out, &mut len, fast_path);   push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Slow-Path (Complex)    : "); push_i64(&mut out, &mut len, slow_path);   push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  ENOSYS (Unimplemented) : "); push_i64(&mut out, &mut len, enosys);      push_bytes(&mut out, &mut len, b"\n\n");

    push_bytes(&mut out, &mut len, b"Zero-Trust IPC Telemetry:\n");
    push_bytes(&mut out, &mut len, b"  Messages Sent          : "); push_i64(&mut out, &mut len, sent);        push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Messages Received      : "); push_i64(&mut out, &mut len, received);    push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Messages Dropped       : "); push_i64(&mut out, &mut len, dropped);     push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  RPC Calls Handled      : "); push_i64(&mut out, &mut len, rpc);         push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  RPC Timeouts           : "); push_i64(&mut out, &mut len, timeouts);    push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Futex Waits / Wakes    : "); push_i64(&mut out, &mut len, waits);       push_bytes(&mut out, &mut len, b" / "); push_i64(&mut out, &mut len, wakes); push_bytes(&mut out, &mut len, b"\n");

    write_bytes(&out[..len]);
}

