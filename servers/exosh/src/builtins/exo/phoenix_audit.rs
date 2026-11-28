//! `exo phoenix`, `exo caps`, `exo net`, `exo audit`, `exo logs`, `exo mount/umount/cp`.
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;

use super::doctor_metrics::ipc_service_online;

pub(super) const PHOENIX_METRICS_MAGIC: u32 = 0x504D_4554;

#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub(super) struct PhoenixTimingRecordWire {
    pub(super) magic: u32,
    pub(super) cycle_id: u32,
    pub(super) trigger_reason: u32,
    pub(super) flags: u32,
    pub(super) tsc_khz: u64,
    pub(super) t0_freeze_req: u64,
    pub(super) t1_freeze_ack_all: u64,
    pub(super) t2_iommu_and_forge: u64,
    pub(super) t3_kernel_b_active: u64,
    pub(super) t4_crypto_reseeded: u64,
    pub(super) t5_strate1_ready: u64,
    pub(super) t6_strate2_ready: u64,
    pub(super) delta_s0_freeze_us: u64,
    pub(super) delta_s0_forge_us: u64,
    pub(super) delta_s0_us: u64,
    pub(super) delta_crypto_us: u64,
    pub(super) delta_sla_us: u64,
    pub(super) delta_full_us: u64,
    pub(super) crypto_epoch_id: u64,
}

impl PhoenixTimingRecordWire {
    pub(super) const fn zeroed() -> Self {
        Self {
            magic: 0,
            cycle_id: 0,
            trigger_reason: 0,
            flags: 0,
            tsc_khz: 0,
            t0_freeze_req: 0,
            t1_freeze_ack_all: 0,
            t2_iommu_and_forge: 0,
            t3_kernel_b_active: 0,
            t4_crypto_reseeded: 0,
            t5_strate1_ready: 0,
            t6_strate2_ready: 0,
            delta_s0_freeze_us: 0,
            delta_s0_forge_us: 0,
            delta_s0_us: 0,
            delta_crypto_us: 0,
            delta_sla_us: 0,
            delta_full_us: 0,
            crypto_epoch_id: 0,
        }
    }
}

fn print_phoenix_timing_record(rec: &PhoenixTimingRecordWire) {
    if rec.magic != PHOENIX_METRICS_MAGIC {
        write_all(b"  Last Switch SLA : (no cycle recorded yet)\n");
        return;
    }
    write_all(b"  Last Cycle ID   : ");
    write_u64(rec.cycle_id as u64);
    write_all(b" (epoch=");
    write_u64(rec.crypto_epoch_id);
    write_all(b", degraded=");
    write_u64(u64::from((rec.flags & 1) != 0));
    write_all(b")\n");
    write_all(b"  Strate 0 (Ring0): ");
    write_u64(rec.delta_s0_us);
    write_all(b" us (freeze=");
    write_u64(rec.delta_s0_freeze_us);
    write_all(b" us, forge=");
    write_u64(rec.delta_s0_forge_us);
    write_all(b" us)\n");
    write_all(b"  Strate 0+1 SLA  : ");
    write_u64(rec.delta_sla_us);
    write_all(b" us (crypto=");
    write_u64(rec.delta_crypto_us);
    write_all(b" us)\n");
    write_all(b"  Full S2 Signal  : ");
    write_u64(rec.delta_full_us);
    write_all(b" us\n");
}

pub(super) fn handle_exo_phoenix(args: &[u8]) {
    let (action, _) = first_token(args);
    if bytes_eq(action, b"switch") {
        write_all(b"Initiating manual ExoPhoenix 3-Strata A/B switch (SYS_PHOENIX_FORCE=523)...\n");
        let mut rec = PhoenixTimingRecordWire::zeroed();
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_EXO_PHOENIX_FORCE,
                1, // PHOENIX_REASON_FORCE_SYSCALL
                &mut rec as *mut PhoenixTimingRecordWire as u64,
                core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
            )
        };
        if rc == 0 {
            write_all(b"[OK] ExoPhoenix 3-Strata switch completed.\n");
            print_phoenix_timing_record(&rec);
        } else {
            write_all(b"[FAIL] Failed to trigger handoff: error code ");
            write_i64(rc);
            write_all(b"\n");
        }
        return;
    }

    let state = unsafe { syscall::syscall0(syscall::SYS_EXO_PHOENIX_STATE_GET) };
    write_all(b"ExoPhoenix Status: ");
    match state {
        1 => write_all(b"Normal (Kernel B active, Kernel A standby)\n"),
        9 => write_all(b"NetworkDraining (Draining rings before A/B switch)\n"),
        10 => write_all(b"NetworkSerialized (Ready for atomic handoff)\n"),
        rc => {
            write_all(b"State code ");
            write_u32(rc as u32);
            write_all(b"\n");
        }
    }
    let mut rec = PhoenixTimingRecordWire::zeroed();
    let mrc = unsafe {
        syscall::syscall3(
            syscall::SYS_EXOPHOENIX_TEST,
            0,
            &mut rec as *mut PhoenixTimingRecordWire as u64,
            core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
        )
    };
    if mrc > 0 {
        print_phoenix_timing_record(&rec);
    }
}

pub(super) fn print_exo_caps() {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    let uid = unsafe { syscall::syscall0(syscall::SYS_GETUID) };
    let gid = unsafe { syscall::syscall0(syscall::SYS_GETGID) };
    // FIX-25: Query live Zero-Trust restriction mask via SYS_EXO_PLEDGE(0, 1) and prctl state.
    let restrictions = unsafe { syscall::syscall2(syscall::SYS_EXO_PLEDGE, 0, 1) };
    let nnp = unsafe { syscall::syscall5(syscall::SYS_PRCTL, 39, 0, 0, 0, 0) }; // PR_GET_NO_NEW_PRIVS
    let dumpable = unsafe { syscall::syscall5(syscall::SYS_PRCTL, 3, 0, 0, 0, 0) }; // PR_GET_DUMPABLE
    write_all(b"Process Identity & Security State (exosh):\n");
    write_all(b"  PID / UID / GID : ");
    write_i64(pid);
    write_all(b" / ");
    write_i64(uid);
    write_all(b" / ");
    write_i64(gid);
    write_all(b"\n");
    write_all(b"  ExoShield IPC   : ");
    if ipc_service_online(b"exo_shield") {
        write_all(b"online\n");
    } else {
        write_all(b"offline\n");
    }
    write_all(b"  Zero-Trust mask : ");
    write_u64(if restrictions >= 0 { restrictions as u64 } else { 0 });
    write_all(b"  (no_new_privs=");
    write_i64(if nnp >= 0 { nnp } else { 0 });
    write_all(b", dumpable=");
    write_i64(if dumpable >= 0 { dumpable } else { 0 });
    write_all(b")\n");
}

pub(super) fn print_exo_net(_args: &[u8]) {
    write_all(b"ExoNet Stack Status:\n");
    write_all(b"  network_server  : ");
    if ipc_service_online(b"network_server") {
        write_all(b"online\n");
    } else {
        write_all(b"offline\n");
    }
    let sock = unsafe { syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, 2, 0) };
    if sock >= 0 {
        let _ = close_fd(sock);
        write_all(b"  AF_INET probe   : OK\n");
    } else {
        write_all(b"  AF_INET probe   : errno ");
        write_i64(sock);
        write_all(b"\n");
    }
    let conf_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            b"/etc/network.conf\0".as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if conf_fd >= 0 {
        let mut buf = [0u8; 256];
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                conf_fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        let _ = close_fd(conf_fd);
        if n > 0 {
            write_all(b"  /etc/network.conf:\n");
            write_bytes(&buf[..n as usize]);
            if buf[n as usize - 1] != b'\n' {
                write_all(b"\n");
            }
        }
    } else {
        write_all(b"  /etc/network.conf: not present (using server default)\n");
    }
}


pub(super) fn handle_exo_audit(_args: &[u8]) {
    write_all(b"=== ExoLedger Audit Trail ===\n");
    let chain_rc = unsafe { syscall::syscall3(syscall::SYS_EXOPHOENIX_TEST, 1, 0, 0) };
    if chain_rc >= 0 {
        write_all(b"ExoLedger BLAKE3 chain : INTACT (PASS, ");
        write_i64(chain_rc);
        write_all(b" verified entries)\n");
    } else {
        write_all(b"ExoLedger BLAKE3 chain : CORRUPTED (FAIL, errno=");
        write_i64(chain_rc);
        write_all(b")\n");
    }
    let mut st = LinuxStat::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_STAT,
            b"/var/log/exoledger.sealed\0".as_ptr() as u64,
            &mut st as *mut LinuxStat as u64,
        )
    };
    if rc == 0 {
        write_all(b"Log file : /var/log/exoledger.sealed (");
        write_i64(st.st_size);
        write_all(b" bytes)\n");
    } else {
        write_all(b"Log file : /var/log/exoledger.sealed not present (in-memory SSR P0+Ring active)\n");
    }
}

pub(super) fn print_exo_logs(_args: &[u8]) {
    write_all(b"exo log: ring-buffer log reader not wired to kernel/monitor_server yet (ENOSYS)\n");
}

pub(super) fn handle_exo_mount(args: &[u8]) {
    let (dev, tail) = next_arg(args);
    let (mnt, _) = next_arg(tail);
    let dev_name = if dev.is_empty() { b"/dev/usb0" as &[u8] } else { dev };
    let mnt_name = if mnt.is_empty() { b"/mnt/usb" as &[u8] } else { mnt };
    write_all(b"exo mount: cannot mount ");
    write_bytes(dev_name);
    write_all(b" on ");
    write_bytes(mnt_name);
    write_all(b" (dynamic filesystem mount syscall not implemented: ENOSYS)\n");
}

pub(super) fn handle_exo_umount(args: &[u8]) {
    let (mnt, _) = next_arg(args);
    let mnt_name = if mnt.is_empty() { b"/mnt/usb" as &[u8] } else { mnt };
    let rc = unsafe { syscall::syscall0(syscall::SYS_SYNC) };
    write_all(b"exo umount: synced rootfs (rc=");
    write_i64(rc);
    write_all(b"), dynamic unmount of ");
    write_bytes(mnt_name);
    write_all(b" not implemented (ENOSYS)\n");
}

pub(super) fn handle_exo_cp(args: &[u8], state: &ShellState) {
    let (src_arg, tail) = next_arg(args);
    let (dst_arg, _) = next_arg(tail);
    if src_arg.is_empty() || dst_arg.is_empty() {
        write_all(b"exo cp: usage: exo cp <src> <dst>\n");
        return;
    }

    let mut src_path = [0u8; PATH_MAX];
    let Some(src_len) = absolute_path(state.cwd(), src_arg, &mut src_path) else {
        write_all(b"exo cp: source path invalid\n");
        return;
    };
    let mut dst_path = [0u8; PATH_MAX];
    let Some(dst_len) = absolute_path(state.cwd(), dst_arg, &mut dst_path) else {
        write_all(b"exo cp: destination path invalid\n");
        return;
    };
    src_path[src_len] = 0;
    dst_path[dst_len] = 0;

    let in_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            src_path.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if in_fd < 0 {
        print_errno(b"exo cp (open src)", in_fd);
        return;
    }

    let out_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            dst_path.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
            0o644,
        )
    };
    if out_fd < 0 {
        print_errno(b"exo cp (open dst)", out_fd);
        let _ = close_fd(in_fd);
        return;
    }

    let mut buf = [0u8; 1024];
    let mut total_copied = 0usize;
    let mut had_err = false;
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                in_fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == 0 {
            break;
        }
        if n < 0 {
            print_errno(b"exo cp read", n);
            had_err = true;
            break;
        }
        let rc = write_fd_all(out_fd, &buf[..n as usize]);
        if rc < 0 {
            print_errno(b"exo cp write", rc);
            had_err = true;
            break;
        }
        total_copied += n as usize;
    }
    let _ = close_fd(out_fd);
    let _ = close_fd(in_fd);

    if !had_err {
        write_all(b"exo cp: copied ");
        write_u64(total_copied as u64);
        write_all(b" bytes\n");
    }
}

