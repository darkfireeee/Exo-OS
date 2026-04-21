#![no_std]
#![no_main]

//! # scheduler_server — politique scheduling Ring 1
//!
//! Ce serveur maintient l’état de politique demandé par les processus Ring 3 :
//! - priorités / nice / classes de scheduling ;
//! - budgets temps réel bornés ;
//! - affinité CPU et métriques de yield.

use core::panic::PanicInfo;

use spin::Mutex;

mod policy_advisor;
mod protocol;
mod realtime_admit;
mod stats_collector;
mod thread_table;

use policy_advisor::{PolicyAdvisor, SchedulingClass};
use protocol::{
    read_i32, read_u32, read_u64, recv_request, register_endpoint, send_heartbeat, send_reply,
    SchedulerReply, SchedulerRequest, SCHED_MSG_GET_STAT, SCHED_MSG_HEARTBEAT,
    SCHED_MSG_REALTIME_ADMIT, SCHED_MSG_REALTIME_RELEASE, SCHED_MSG_SET_AFFINITY,
    SCHED_MSG_SET_POLICY, SCHED_MSG_SET_PRIORITY, SCHED_MSG_THREAD_REGISTER, SCHED_MSG_YIELD,
};
use realtime_admit::RealtimeAdmission;
use stats_collector::StatsCollector;
use thread_table::ThreadTable;

struct SchedulerService {
    advisor: PolicyAdvisor,
    threads: ThreadTable,
    realtime: RealtimeAdmission,
    stats: StatsCollector,
}

impl SchedulerService {
    const fn new() -> Self {
        Self {
            advisor: PolicyAdvisor::new(),
            threads: ThreadTable::new(),
            realtime: RealtimeAdmission::new(),
            stats: StatsCollector::new(),
        }
    }

    fn handle_register(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let raw_nice = match read_i32(payload, 4) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let raw_class = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let affinity_mask = match read_u64(payload, 16) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let flags = match read_u32(payload, 24) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let class = SchedulingClass::from_u32(raw_class).unwrap_or(SchedulingClass::Cfs);
        let profile = self.advisor.recommend(raw_nice, 8, class);

        match self
            .threads
            .register(sender_pid, tid, profile.nice, profile.class, affinity_mask, profile.priority_weight, flags)
        {
            Ok(snapshot) => {
                self.stats.note_register(snapshot);
                SchedulerReply::ok(
                    snapshot.tid as u64,
                    snapshot.priority_weight as u64,
                    ((self.threads.active_count() as u64) << 32) | (snapshot.affinity_mask & 0xffff_ffff),
                    snapshot.class.as_u32() | ((snapshot.flags & 0xff) << 8),
                )
            }
            Err(err) => SchedulerReply::error(err),
        }
    }

    fn handle_set_priority(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let raw_nice = match read_i32(payload, 4) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let latency_hint = match read_u32(payload, 8) {
            Ok(value) => value as u16,
            Err(err) => return SchedulerReply::error(err),
        };

        let existing = match self.threads.snapshot_owned(sender_pid, tid) {
            Ok(snapshot) => snapshot,
            Err(err) => return SchedulerReply::error(err),
        };
        let profile = self.advisor.recommend(raw_nice, latency_hint, existing.class);
        if let Err(err) = apply_kernel_priority(existing.pid, profile.nice) {
            self.stats.note_error(existing.pid, existing.tid, err);
            return SchedulerReply::error(err);
        }

        match self
            .threads
            .update_priority(sender_pid, tid, profile.nice, profile.priority_weight)
        {
            Ok(snapshot) => {
                self.stats.note_priority(snapshot);
                SchedulerReply::ok(
                    snapshot.tid as u64,
                    snapshot.priority_weight as u64,
                    ((profile.quantum_ms as u64) << 32) | (snapshot.nice as i16 as u16 as u64),
                    profile.class.as_u32() | ((snapshot.flags & 0xff) << 8),
                )
            }
            Err(err) => SchedulerReply::error(err),
        }
    }

    fn handle_set_policy(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let raw_class = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let raw_nice = match read_i32(payload, 8) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let runtime_us = match read_u32(payload, 12) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let period_us = match read_u32(payload, 16) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let flags = match read_u32(payload, 20) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };

        let Some(class) = SchedulingClass::from_u32(raw_class) else {
            return SchedulerReply::error(exo_syscall_abi::EINVAL);
        };
        let profile = self.advisor.recommend(raw_nice, 4, class);
        let owner_pid = match self.threads.owner_pid(tid) {
            Some(pid) if pid == sender_pid => pid,
            Some(_) => return SchedulerReply::error(exo_syscall_abi::EPERM),
            None => return SchedulerReply::error(exo_syscall_abi::ENOENT),
        };

        if matches!(class, SchedulingClass::Realtime | SchedulingClass::Deadline) {
            if let Err(err) = self.realtime.admit(tid, runtime_us.max(1), period_us.max(runtime_us.max(1))) {
                self.stats.note_error(owner_pid, tid, err);
                return SchedulerReply::error(err);
            }
        }

        if let Err(err) = apply_kernel_policy(owner_pid, class) {
            self.stats.note_error(owner_pid, tid, err);
            return SchedulerReply::error(err);
        }

        if let Err(err) = apply_kernel_priority(owner_pid, profile.nice) {
            self.stats.note_error(owner_pid, tid, err);
            return SchedulerReply::error(err);
        }

        match self.threads.update_class(sender_pid, tid, class, flags) {
            Ok(mut snapshot) => {
                if let Ok(updated) =
                    self.threads.update_priority(sender_pid, tid, profile.nice, profile.priority_weight)
                {
                    snapshot = updated;
                }
                self.stats.note_policy(snapshot);
                let rt_total = self.realtime.total_utilization_ppm() as u64;
                SchedulerReply::ok(
                    snapshot.tid as u64,
                    snapshot.priority_weight as u64,
                    rt_total,
                    snapshot.class.as_u32(),
                )
            }
            Err(err) => SchedulerReply::error(err),
        }
    }

    fn handle_set_affinity(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let affinity_mask = match read_u64(payload, 8) {
            Ok(value) => value.max(1),
            Err(err) => return SchedulerReply::error(err),
        };

        let owner_pid = match self.threads.owner_pid(tid) {
            Some(pid) if pid == sender_pid => pid,
            Some(_) => return SchedulerReply::error(exo_syscall_abi::EPERM),
            None => return SchedulerReply::error(exo_syscall_abi::ENOENT),
        };

        if let Err(err) = apply_kernel_affinity(owner_pid, affinity_mask) {
            self.stats.note_error(owner_pid, tid, err);
            return SchedulerReply::error(err);
        }

        match self.threads.set_affinity(sender_pid, tid, affinity_mask) {
            Ok(snapshot) => {
                self.stats.note_affinity(snapshot);
                SchedulerReply::ok(
                    snapshot.tid as u64,
                    snapshot.affinity_mask,
                    self.stats.active_count() as u64,
                    snapshot.class.as_u32(),
                )
            }
            Err(err) => SchedulerReply::error(err),
        }
    }

    fn handle_yield(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let owner_pid = match self.threads.owner_pid(tid) {
            Some(pid) if pid == sender_pid => pid,
            Some(_) => return SchedulerReply::error(exo_syscall_abi::EPERM),
            None => return SchedulerReply::error(exo_syscall_abi::ENOENT),
        };

        // SAFETY: appel sans argument, effet borné au thread courant.
        let rc = unsafe { exo_syscall_abi::syscall0(exo_syscall_abi::SYS_SCHED_YIELD) };
        if rc < 0 {
            self.stats.note_error(owner_pid, tid, rc);
            return SchedulerReply::error(rc);
        }
        self.stats.note_yield(owner_pid, tid);
        let yields = self.stats.snapshot(tid).map(|stats| stats.yield_count).unwrap_or(0);
        SchedulerReply::ok(tid as u64, yields as u64, 0, 0)
    }

    fn handle_get_stat(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let thread = match self.threads.snapshot_any(tid) {
            Some(snapshot) => snapshot,
            None => return SchedulerReply::error(exo_syscall_abi::ENOENT),
        };
        if thread.pid != sender_pid && sender_pid != 1 {
            return SchedulerReply::error(exo_syscall_abi::EPERM);
        }

        let stats = self.stats.snapshot(tid).unwrap_or_else(|| {
            self.stats.note_register(thread);
            self.stats.snapshot(tid).unwrap()
        });
        let rt = self.realtime.snapshot(tid);
        SchedulerReply::ok(
            ((stats.pid as u64) << 32) | stats.tid as u64,
            ((stats.yield_count as u64) << 32) | (stats.priority_weight as u64),
            rt.map(|entry| ((entry.runtime_us as u64) << 32) | entry.period_us as u64)
                .unwrap_or(((stats.affinity_mask & 0xffff_ffff) << 32) | ((stats.last_error as i32 as u32) as u64)),
            stats.class.as_u32()
                | ((stats.priority_updates.min(0xff) as u32) << 8)
                | ((stats.policy_updates.min(0xff) as u32) << 16)
                | ((stats.affinity_updates.min(0xff) as u32) << 24),
        )
    }

    fn handle_realtime_admit(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let runtime_us = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let period_us = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let owner_pid = match self.threads.owner_pid(tid) {
            Some(pid) if pid == sender_pid => pid,
            Some(_) => return SchedulerReply::error(exo_syscall_abi::EPERM),
            None => return SchedulerReply::error(exo_syscall_abi::ENOENT),
        };

        match self.realtime.admit(tid, runtime_us, period_us) {
            Ok(snapshot) => SchedulerReply::ok(
                snapshot.tid as u64,
                snapshot.utilization_ppm as u64,
                snapshot.total_utilization_ppm as u64,
                owner_pid,
            ),
            Err(err) => {
                self.stats.note_error(owner_pid, tid, err);
                SchedulerReply::error(err)
            }
        }
    }

    fn handle_realtime_release(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
        let tid = match read_u32(payload, 0) {
            Ok(0) => sender_pid,
            Ok(value) => value,
            Err(err) => return SchedulerReply::error(err),
        };
        let owner_pid = match self.threads.owner_pid(tid) {
            Some(pid) if pid == sender_pid => pid,
            Some(_) => return SchedulerReply::error(exo_syscall_abi::EPERM),
            None => return SchedulerReply::error(exo_syscall_abi::ENOENT),
        };

        match self.realtime.release(tid) {
            Some(snapshot) => SchedulerReply::ok(
                snapshot.tid as u64,
                snapshot.utilization_ppm as u64,
                snapshot.total_utilization_ppm as u64,
                owner_pid,
            ),
            None => SchedulerReply::error(exo_syscall_abi::ENOENT),
        }
    }
}

static SCHEDULER_SERVICE: Mutex<SchedulerService> = Mutex::new(SchedulerService::new());

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = SchedulerRequest::zeroed();

    loop {
        match recv_request(&mut request) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => continue,
        }

        let reply = if request.msg_type == SCHED_MSG_HEARTBEAT {
            send_heartbeat()
        } else {
            dispatch(&request)
        };
        let _ = send_reply(request.sender_pid, &reply);
    }
}

fn dispatch(request: &SchedulerRequest) -> SchedulerReply {
    let mut service = SCHEDULER_SERVICE.lock();

    match request.msg_type {
        SCHED_MSG_THREAD_REGISTER => service.handle_register(request.sender_pid, &request.payload),
        SCHED_MSG_SET_PRIORITY => service.handle_set_priority(request.sender_pid, &request.payload),
        SCHED_MSG_SET_POLICY => service.handle_set_policy(request.sender_pid, &request.payload),
        SCHED_MSG_SET_AFFINITY => service.handle_set_affinity(request.sender_pid, &request.payload),
        SCHED_MSG_YIELD => service.handle_yield(request.sender_pid, &request.payload),
        SCHED_MSG_GET_STAT => service.handle_get_stat(request.sender_pid, &request.payload),
        SCHED_MSG_REALTIME_ADMIT => service.handle_realtime_admit(request.sender_pid, &request.payload),
        SCHED_MSG_REALTIME_RELEASE => service.handle_realtime_release(request.sender_pid, &request.payload),
        _ => SchedulerReply::error(exo_syscall_abi::EINVAL),
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        // SAFETY: panic terminale pour un serveur no_std monothread.
        unsafe { core::arch::asm!("hlt", options(nostack, nomem)); }
    }
}

fn apply_kernel_priority(pid: u32, nice: i8) -> Result<(), i64> {
    // SAFETY: ABI POSIX standard `setpriority(PRIO_PROCESS=0, pid, nice)`.
    let rc = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_SETPRIORITY,
            0,
            pid as u64,
            (nice as i64) as u64,
        )
    };
    if rc < 0 && rc != exo_syscall_abi::ENOSYS {
        Err(rc)
    } else {
        Ok(())
    }
}

fn apply_kernel_policy(pid: u32, class: SchedulingClass) -> Result<(), i64> {
    // SAFETY: `sched_setscheduler(pid, policy, NULL)` côté noyau peut être absent ; fallback accepté.
    let rc = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_SCHED_SETSCHEDULER,
            pid as u64,
            class.as_u32() as u64,
            0,
        )
    };
    if rc < 0 && rc != exo_syscall_abi::ENOSYS {
        Err(rc)
    } else {
        Ok(())
    }
}

fn apply_kernel_affinity(pid: u32, affinity_mask: u64) -> Result<(), i64> {
    let mask = affinity_mask;
    // SAFETY: pointeur vers une valeur locale immuable valide pendant l’appel.
    let rc = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_SCHED_SETAFFINITY,
            pid as u64,
            core::mem::size_of::<u64>() as u64,
            &mask as *const u64 as u64,
        )
    };
    if rc < 0 && rc != exo_syscall_abi::ENOSYS {
        Err(rc)
    } else {
        Ok(())
    }
}
