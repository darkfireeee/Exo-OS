//! # servers/exosh
#![allow(dead_code, unused_imports)]
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use exo_syscall_abi as syscall;

use super::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

pub fn cmd_bench(rest: &[u8]) {
    let (sub, args) = next_arg(rest);
    if bytes_eq(sub, b"ipc") {
        bench_ipc(args);
    } else if bytes_eq(sub, b"sched") {
        bench_sched(args);
    } else if bytes_eq(sub, b"crypto") {
        bench_crypto(args);
    } else if bytes_eq(sub, b"fs") {
        bench_fs(args);
    } else {
        write_all(b"bench: subcommands: ipc sched crypto fs\n");
        write_all(b"  bench ipc    [n=10000]  -- IPC round-trip latency\n");
        write_all(b"  bench sched  [n=10000]  -- scheduler yield cost\n");
        write_all(b"  bench crypto [n=1000]   -- crypto_server random throughput\n");
        write_all(b"  bench fs     [n=128]    -- write+read N MiB non-zero\n");
    }
}

pub fn parse_bench_n(args: &[u8], default: u64) -> u64 {
    let (arg, _) = next_arg(args);
    if let Some(val) = strip_prefix(arg, b"n=") {
        if let Some(n) = parse_u64(val) {
            if n > 0 {
                return n;
            }
        }
    }
    default
}

pub fn bench_ipc(args: &[u8]) {
    let n = parse_bench_n(args, 10_000);
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        write_all(b"bench ipc: getpid failed\n");
        return;
    }

    let name = b"ipc_router";
    let endpoint = unsafe {
        syscall::syscall2(
            syscall::SYS_IPC_LOOKUP,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    if endpoint <= 0 {
        write_all(b"bench ipc: ipc_router not found\n");
        return;
    }

    let reply_endpoint = ((pid as u64) << 32) | 0x4950_4342;
    let reply_name = b"exosh_ipc_bench";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            reply_name.as_ptr() as u64,
            reply_name.len() as u64,
            reply_endpoint,
        )
    };

    let mut msg = syscall::IpcMessage::zeroed();
    msg.msg_type = 2; // ipc_router heartbeat
    msg.payload[..8].copy_from_slice(&reply_endpoint.to_le_bytes());
    write_all(b"bench ipc: ");
    write_u64(n);
    write_all(b" heartbeats... ");

    let Some(start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut i = 0u64;
    while i < n {
        let send_rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                endpoint as u64,
                &msg as *const syscall::IpcMessage as u64,
                core::mem::size_of::<syscall::IpcMessage>() as u64,
                syscall::IPC_FLAG_INJECT_SRC_PID,
                0,
                0,
            )
        };
        if send_rc < 0 {
            print_errno(b"bench ipc send", send_rc);
            return;
        }

        let mut reply = [0u8; 4];
        let recv_rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                reply_endpoint,
                reply.as_mut_ptr() as u64,
                reply.len() as u64,
                syscall::IPC_FLAG_TIMEOUT | 100,
            )
        };
        if recv_rc < 0 {
            print_errno(b"bench ipc recv", recv_rc);
            return;
        }

        i = i.wrapping_add(1);
    }

    let Some(end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let elapsed_ns = end.saturating_sub(start);
    let elapsed_ms = elapsed_ns / 1_000_000;
    let ns_per_op = if n > 0 { elapsed_ns / n } else { 0 };
    let kops_centi = if elapsed_ns > 0 {
        n.saturating_mul(100_000_000) / elapsed_ns
    } else {
        0
    };

    write_u64(elapsed_ms);
    write_all(b"ms -> ");
    write_u64(ns_per_op);
    write_all(b"ns/op (");
    write_fixed_2(kops_centi);
    write_all(b" Kops/s)\n");
}

pub fn bench_sched(args: &[u8]) {
    let n = parse_bench_n(args, 10_000);

    write_all(b"bench sched: ");
    write_u64(n);
    write_all(b" yields... ");

    let Some(start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut i = 0u64;
    while i < n {
        unsafe {
            syscall::syscall0(syscall::SYS_SCHED_YIELD);
        }
        i = i.wrapping_add(1);
    }

    let Some(end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let elapsed_ns = end.saturating_sub(start);
    let elapsed_ms = elapsed_ns / 1_000_000;
    let ns_per_yield = if n > 0 { elapsed_ns / n } else { 0 };
    let us_per_yield = ns_per_yield / 1_000;
    let ns_frac = ns_per_yield % 1_000;

    write_u64(elapsed_ms);
    write_all(b"ms -> ");
    write_u64(us_per_yield);
    write_all(b".");
    if ns_frac < 100 {
        write_all(b"0");
    }
    if ns_frac < 10 {
        write_all(b"0");
    }
    write_u64(ns_frac);
    write_all(b"us/yield\n");
}

pub const CRYPTO_SERVER_ENDPOINT: u64 = 4;

const _: () =
    assert!(core::mem::size_of::<CryptoRequestBench>() <= syscall::IPC_KERNEL_MAX_MSG_SIZE);

pub fn bench_crypto(args: &[u8]) {
    let n = parse_bench_n(args, 1_000);
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        write_all(b"bench crypto: getpid failed\n");
        return;
    }

    let reply_endpoint = ((pid as u64) << 32) | 0x4352_5950;
    let reply_name = b"exosh_crypto_bench";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            reply_name.as_ptr() as u64,
            reply_name.len() as u64,
            reply_endpoint,
        )
    };

    let mut token = syscall::ExoCapTokenWire::empty();
    let cap_rc = unsafe {
        syscall::exo_cap_create(
            syscall::EXO_CAP_TYPE_IPC_ENDPOINT,
            syscall::EXO_CAP_RIGHT_IPC_SEND,
            CRYPTO_SERVER_PID,
            &mut token,
        )
    };
    if cap_rc < 0 || token.is_empty() {
        write_all(b"bench crypto: crypto_server capability unavailable\n");
        return;
    }

    let mut req = CryptoRequestBench {
        sender_pid: 0,
        msg_type: CRYPTO_RANDOM,
        reply_endpoint,
        payload_len: 1,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token: token,
        payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
    };
    req.payload[0] = 32;

    write_all(b"bench crypto: ");
    write_u64(n);
    write_all(b" ops RANDOM... ");

    let Some(start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut i = 0u64;
    while i < n {
        let send_rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                CRYPTO_SERVER_ENDPOINT,
                &req as *const CryptoRequestBench as u64,
                core::mem::size_of::<CryptoRequestBench>() as u64,
                syscall::IPC_FLAG_INJECT_SRC_PID,
                0,
                0,
            )
        };
        if send_rc < 0 {
            print_errno(b"bench crypto send", send_rc);
            return;
        }

        let mut reply = CryptoReplyBench {
            sender_pid: 0,
            status: 0,
            key_handle: 0,
            data_len: 0,
            version: 0,
            flags: 0,
            data: [0u8; CRYPTO_REPLY_DATA_SIZE],
        };
        let recv_rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                reply_endpoint,
                &mut reply as *mut CryptoReplyBench as u64,
                core::mem::size_of::<CryptoReplyBench>() as u64,
                syscall::IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };
        if recv_rc < 0 {
            print_errno(b"bench crypto recv", recv_rc);
            return;
        }
        if reply.status != CRYPTO_OK {
            write_all(b"bench crypto: crypto_server status ");
            write_u64(reply.status as u64);
            write_all(b"\n");
            return;
        }

        i = i.wrapping_add(1);
    }

    let Some(end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let elapsed_ns = end.saturating_sub(start);
    let elapsed_ms = elapsed_ns / 1_000_000;
    let us_per_op = if n > 0 { elapsed_ns / n / 1_000 } else { 0 };
    let total_bytes = n.saturating_mul(32);
    let mb_per_sec_centi = if elapsed_ns > 0 {
        (total_bytes as u128).saturating_mul(100_000) / (elapsed_ns as u128)
    } else {
        0
    };

    write_u64(elapsed_ms);
    write_all(b"ms -> ");
    write_u64(us_per_op);
    write_all(b"us/op (");
    write_fixed_2(mb_per_sec_centi.min(u64::MAX as u128) as u64);
    write_all(b" MB/s CSPRNG)\n");
}

pub fn bench_fs(args: &[u8]) {
    let n = parse_bench_n(args, 128);
    let target_bytes = n.saturating_mul(1024 * 1024);
    let path = b"/tmp/bench_fs_data";
    let mut out_path = [0u8; PATH_MAX];
    out_path[..path.len()].copy_from_slice(path);

    write_all(b"bench fs write: ");
    write_u64(n);
    write_all(b" MiB in 64 KiB chunks + one fdatasync... ");

    let buf = unsafe { &mut *DD_IO_BUFFER.0.get() };
    let bench_chunk = FS_BENCH_CHUNK.min(buf.len());
    let getrandom_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_GETRANDOM,
            buf.as_mut_ptr() as u64,
            bench_chunk as u64,
            0,
        )
    };
    if getrandom_rc <= 0 {
        let mut k = 0usize;
        while k < bench_chunk {
            buf[k] = (k.wrapping_mul(0x6B).wrapping_add(0xA5) & 0xFF) as u8;
            k = k.wrapping_add(1);
        }
    }

    let Some(write_start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let out_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            out_path.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
            0o644,
        )
    };
    if out_fd < 0 {
        print_errno(b"bench fs", out_fd);
        return;
    }

    let mut written_bytes = 0u64;
    while written_bytes < target_bytes {
        let this_write = (target_bytes - written_bytes).min(bench_chunk as u64) as usize;
        let rc = write_fd_all(out_fd, &buf[..this_write]);
        if rc < 0 {
            print_errno(b"bench fs write", rc);
            let _ = close_fd(out_fd);
            return;
        }
        written_bytes = written_bytes.saturating_add(this_write as u64);
    }
    let rc = fdatasync_fd(out_fd);
    if rc < 0 {
        print_errno(b"bench fs fdatasync", rc);
        let _ = close_fd(out_fd);
        return;
    }
    let _ = close_fd(out_fd);

    let Some(write_end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    print_dd_summary(target_bytes, write_end.saturating_sub(write_start));

    write_all(b"bench fs read:  ");
    write_u64(n);
    write_all(b" MiB in 64 KiB chunks... ");

    let Some(read_start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut total_read = 0u64;
    while total_read < target_bytes {
        let in_fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                out_path.as_ptr() as u64,
                syscall::O_RDONLY,
                0,
            )
        };
        if in_fd < 0 {
            print_errno(b"bench fs read", in_fd);
            return;
        }

        let mut read_this_open = 0u64;
        while total_read < target_bytes {
            let request = (target_bytes - total_read).min(bench_chunk as u64) as usize;
            let n_read = unsafe {
                syscall::syscall3(
                    syscall::SYS_READ,
                    in_fd as u64,
                    buf.as_mut_ptr() as u64,
                    request as u64,
                )
            };
            if n_read <= 0 {
                break;
            }
            read_this_open = read_this_open.saturating_add(n_read as u64);
            total_read = total_read.saturating_add(n_read as u64);
        }
        let _ = close_fd(in_fd);
        if read_this_open == 0 {
            break;
        }
    }

    let Some(read_end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    print_dd_summary(total_read, read_end.saturating_sub(read_start));
    let _ = unsafe { syscall::syscall1(syscall::SYS_UNLINK, out_path.as_ptr() as u64) };
}

