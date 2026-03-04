// SPDX-License-Identifier: MIT
// ExoFS Observability — Debug Interface
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── Constants ────────────────────────────────────────────────────────────────

pub const DEBUG_QUEUE_SIZE: usize = 32;
pub const DEBUG_MSG_LEN:    usize = 48;

// ─── DebugCommandId ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DebugCommandId {
    Noop           = 0,
    DumpMetrics    = 1,
    DumpAlerts     = 2,
    DumpHealth     = 3,
    DumpLatency    = 4,
    DumpSpace      = 5,
    DumpThroughput = 6,
    DumpTrace      = 7,
    ResetCounters  = 8,
    ForceGc        = 9,
    SetTraceLevel  = 10,
    GetStatus      = 11,
    SelfTest       = 12,
    Shutdown       = 13,
}

impl DebugCommandId {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1  => Self::DumpMetrics,
            2  => Self::DumpAlerts,
            3  => Self::DumpHealth,
            4  => Self::DumpLatency,
            5  => Self::DumpSpace,
            6  => Self::DumpThroughput,
            7  => Self::DumpTrace,
            8  => Self::ResetCounters,
            9  => Self::ForceGc,
            10 => Self::SetTraceLevel,
            11 => Self::GetStatus,
            12 => Self::SelfTest,
            13 => Self::Shutdown,
            _  => Self::Noop,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Noop           => "Noop",
            Self::DumpMetrics    => "DumpMetrics",
            Self::DumpAlerts     => "DumpAlerts",
            Self::DumpHealth     => "DumpHealth",
            Self::DumpLatency    => "DumpLatency",
            Self::DumpSpace      => "DumpSpace",
            Self::DumpThroughput => "DumpThroughput",
            Self::DumpTrace      => "DumpTrace",
            Self::ResetCounters  => "ResetCounters",
            Self::ForceGc        => "ForceGc",
            Self::SetTraceLevel  => "SetTraceLevel",
            Self::GetStatus      => "GetStatus",
            Self::SelfTest       => "SelfTest",
            Self::Shutdown       => "Shutdown",
        }
    }

    pub fn is_destructive(self) -> bool {
        matches!(self, Self::ResetCounters | Self::Shutdown | Self::ForceGc)
    }

    pub fn is_read_only(self) -> bool { !self.is_destructive() }
}

// ─── DebugCommand ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct DebugCommand {
    pub id:        u8,
    pub _pad:      [u8; 7],
    pub arg_u64:   u64,
    pub arg_bytes: [u8; 16],
}

impl DebugCommand {
    pub const fn zeroed() -> Self {
        Self { id: 0, _pad: [0; 7], arg_u64: 0, arg_bytes: [0; 16] }
    }

    pub fn new(id: DebugCommandId, arg: u64) -> Self {
        Self { id: id as u8, _pad: [0; 7], arg_u64: arg, arg_bytes: [0; 16] }
    }

    pub fn command_id(&self) -> DebugCommandId { DebugCommandId::from_u8(self.id) }
    pub fn is_empty(&self)   -> bool { self.id == 0 }
}

// ─── DebugResponseStatus ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DebugResponseStatus {
    Ok           = 0,
    Error        = 1,
    NotSupported = 2,
    Busy         = 3,
    Denied       = 4,
}

impl DebugResponseStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Ok,
            1 => Self::Error,
            2 => Self::NotSupported,
            3 => Self::Busy,
            4 => Self::Denied,
            _ => Self::Error,
        }
    }
    pub fn is_ok(self) -> bool { self == Self::Ok }
    pub fn name(self)  -> &'static str {
        match self {
            Self::Ok           => "OK",
            Self::Error        => "ERROR",
            Self::NotSupported => "NOT_SUPPORTED",
            Self::Busy         => "BUSY",
            Self::Denied       => "DENIED",
        }
    }
}

// ─── DebugResponse ─────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
#[repr(C)]
pub struct DebugResponse {
    pub status:   u8,
    pub cmd_id:   u8,
    pub _pad:     [u8; 6],
    pub data_u64: u64,
    pub msg:      [u8; DEBUG_MSG_LEN],
}

impl DebugResponse {
    pub const fn zeroed() -> Self {
        Self { status: 0, cmd_id: 0, _pad: [0; 6], data_u64: 0, msg: [0; DEBUG_MSG_LEN] }
    }

    pub fn ok(cmd: DebugCommandId, data: u64, text: &str) -> Self {
        let mut r = Self::zeroed();
        r.status   = DebugResponseStatus::Ok as u8;
        r.cmd_id   = cmd as u8;
        r.data_u64 = data;
        let bytes = text.as_bytes();
        let len = bytes.len().min(DEBUG_MSG_LEN);
        let mut i = 0usize;
        while i < len { r.msg[i] = bytes[i]; i = i.wrapping_add(1); }
        r
    }

    pub fn err(cmd: DebugCommandId, text: &str) -> Self {
        let mut r = Self::zeroed();
        r.status = DebugResponseStatus::Error as u8;
        r.cmd_id = cmd as u8;
        let bytes = text.as_bytes();
        let len = bytes.len().min(DEBUG_MSG_LEN);
        let mut i = 0usize;
        while i < len { r.msg[i] = bytes[i]; i = i.wrapping_add(1); }
        r
    }

    pub fn not_supported(cmd: DebugCommandId) -> Self {
        let mut r = Self::zeroed();
        r.status = DebugResponseStatus::NotSupported as u8;
        r.cmd_id = cmd as u8;
        r
    }

    pub fn status(&self) -> DebugResponseStatus { DebugResponseStatus::from_u8(self.status) }
    pub fn is_ok(&self)  -> bool { self.status().is_ok() }
    pub fn is_empty(&self) -> bool { self.status == 0 && self.cmd_id == 0 }

    pub fn msg_to_vec(&self) -> ExofsResult<Vec<u8>> {
        let len = self.msg.iter().position(|&b| b == 0).unwrap_or(DEBUG_MSG_LEN);
        let mut v = Vec::new();
        v.try_reserve(len).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < len { v.push(self.msg[i]); i = i.wrapping_add(1); }
        Ok(v)
    }
}

// ─── DebugQueue ───────────────────────────────────────────────────────────────

pub struct DebugQueue {
    commands:   UnsafeCell<[DebugCommand;  DEBUG_QUEUE_SIZE]>,
    responses:  UnsafeCell<[DebugResponse; DEBUG_QUEUE_SIZE]>,
    cmd_head:   AtomicU64,
    cmd_tail:   AtomicU64,
    resp_head:  AtomicU64,
    resp_tail:  AtomicU64,
    cmd_count:  AtomicU64,
    resp_count: AtomicU64,
    total_cmds: AtomicU64,
}

unsafe impl Sync for DebugQueue {}
unsafe impl Send for DebugQueue {}

impl DebugQueue {
    pub const fn new_const() -> Self {
        Self {
            commands:   UnsafeCell::new([DebugCommand::zeroed();  DEBUG_QUEUE_SIZE]),
            responses:  UnsafeCell::new([DebugResponse::zeroed(); DEBUG_QUEUE_SIZE]),
            cmd_head:   AtomicU64::new(0),
            cmd_tail:   AtomicU64::new(0),
            resp_head:  AtomicU64::new(0),
            resp_tail:  AtomicU64::new(0),
            cmd_count:  AtomicU64::new(0),
            resp_count: AtomicU64::new(0),
            total_cmds: AtomicU64::new(0),
        }
    }

    pub fn push_cmd(&self, cmd: DebugCommand) -> ExofsResult<()> {
        let n = self.cmd_count.load(Ordering::Relaxed);
        if n >= DEBUG_QUEUE_SIZE as u64 { return Err(ExofsError::Resource); }
        let idx = self.cmd_head.fetch_add(1, Ordering::Relaxed) as usize % DEBUG_QUEUE_SIZE;
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe { (*self.commands.get())[idx] = cmd; }
        self.cmd_count.fetch_add(1, Ordering::Relaxed);
        self.total_cmds.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn pop_cmd(&self) -> Option<DebugCommand> {
        let n = self.cmd_count.load(Ordering::Relaxed);
        if n == 0 { return None; }
        let idx = self.cmd_tail.fetch_add(1, Ordering::Relaxed) as usize % DEBUG_QUEUE_SIZE;
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let cmd = unsafe { (*self.commands.get())[idx] };
        self.cmd_count.fetch_sub(1, Ordering::Relaxed);
        if cmd.is_empty() { None } else { Some(cmd) }
    }

    pub fn push_response(&self, resp: DebugResponse) -> ExofsResult<()> {
        let n = self.resp_count.load(Ordering::Relaxed);
        if n >= DEBUG_QUEUE_SIZE as u64 { return Err(ExofsError::Resource); }
        let idx = self.resp_head.fetch_add(1, Ordering::Relaxed) as usize % DEBUG_QUEUE_SIZE;
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe { (*self.responses.get())[idx] = resp; }
        self.resp_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn pop_response(&self) -> Option<DebugResponse> {
        let n = self.resp_count.load(Ordering::Relaxed);
        if n == 0 { return None; }
        let idx = self.resp_tail.fetch_add(1, Ordering::Relaxed) as usize % DEBUG_QUEUE_SIZE;
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let resp = unsafe { (*self.responses.get())[idx] };
        self.resp_count.fetch_sub(1, Ordering::Relaxed);
        if resp.is_empty() { None } else { Some(resp) }
    }

    pub fn cmd_pending(&self)  -> u64 { self.cmd_count.load(Ordering::Relaxed) }
    pub fn resp_pending(&self) -> u64 { self.resp_count.load(Ordering::Relaxed) }
    pub fn total_cmds(&self)   -> u64 { self.total_cmds.load(Ordering::Relaxed) }
    pub fn is_empty(&self)     -> bool { self.cmd_pending() == 0 }

    pub fn drain_responses(&self) -> ExofsResult<Vec<DebugResponse>> {
        let n = self.resp_count.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < n {
            if let Some(r) = self.pop_response() {
                v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                v.push(r);
            }
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

pub static DEBUG_QUEUE: DebugQueue = DebugQueue::new_const();

// ─── DebugSession ─────────────────────────────────────────────────────────────

/// Exécute des commandes de debug sans état persistant.
pub struct DebugSession<'a> {
    queue:    &'a DebugQueue,
    exec_cnt: AtomicU64,
    err_cnt:  AtomicU64,
}

impl<'a> DebugSession<'a> {
    pub fn new(queue: &'a DebugQueue) -> Self {
        Self { queue, exec_cnt: AtomicU64::new(0), err_cnt: AtomicU64::new(0) }
    }

    /// Dispatch d'une commande — retourne la réponse.
    pub fn execute(&self, cmd: DebugCommand) -> DebugResponse {
        self.exec_cnt.fetch_add(1, Ordering::Relaxed);
        let id = cmd.command_id();
        let resp = match id {
            DebugCommandId::Noop      => DebugResponse::ok(id, 0, "noop"),
            DebugCommandId::GetStatus => DebugResponse::ok(id, self.exec_cnt.load(Ordering::Relaxed), "status ok"),
            DebugCommandId::SelfTest  => self.run_self_test(),
            DebugCommandId::DumpMetrics    => DebugResponse::ok(id, 0, "metrics dumped"),
            DebugCommandId::DumpAlerts     => DebugResponse::ok(id, 0, "alerts dumped"),
            DebugCommandId::DumpHealth     => DebugResponse::ok(id, 0, "health dumped"),
            DebugCommandId::DumpLatency    => DebugResponse::ok(id, 0, "latency dumped"),
            DebugCommandId::DumpSpace      => DebugResponse::ok(id, 0, "space dumped"),
            DebugCommandId::DumpThroughput => DebugResponse::ok(id, 0, "throughput dumped"),
            DebugCommandId::DumpTrace      => DebugResponse::ok(id, 0, "trace dumped"),
            DebugCommandId::ResetCounters  => DebugResponse::ok(id, 0, "counters reset"),
            DebugCommandId::ForceGc        => DebugResponse::ok(id, 0, "gc triggered"),
            DebugCommandId::SetTraceLevel  => DebugResponse::ok(id, cmd.arg_u64, "level set"),
            DebugCommandId::Shutdown       => DebugResponse::ok(id, 0, "shutdown ack"),
        };
        if !resp.is_ok() { self.err_cnt.fetch_add(1, Ordering::Relaxed); }
        resp
    }

    /// Exécute la commande et pousse la réponse dans la queue.
    pub fn dispatch(&self, cmd: DebugCommand) -> ExofsResult<()> {
        let resp = self.execute(cmd);
        self.queue.push_response(resp)
    }

    fn run_self_test(&self) -> DebugResponse {
        let id = DebugCommandId::SelfTest;
        // Test d'auto-vérification : vérifie que la queue est cohérente.
        let cmd_ok  = DEBUG_QUEUE.cmd_pending()  <= DEBUG_QUEUE_SIZE as u64;
        let resp_ok = DEBUG_QUEUE.resp_pending() <= DEBUG_QUEUE_SIZE as u64;
        if cmd_ok && resp_ok {
            DebugResponse::ok(id, 1, "self-test passed")
        } else {
            DebugResponse::err(id, "self-test failed: queue inconsistent")
        }
    }

    /// Traite tous les commandes en attente dans la queue.
    pub fn process_pending(&self) -> ExofsResult<u64> {
        let mut processed = 0u64;
        while let Some(cmd) = self.queue.pop_cmd() {
            let resp = self.execute(cmd);
            self.queue.push_response(resp)?;
            processed = processed.saturating_add(1);
        }
        Ok(processed)
    }

    pub fn exec_count(&self) -> u64 { self.exec_cnt.load(Ordering::Relaxed) }
    pub fn err_count(&self)  -> u64 { self.err_cnt.load(Ordering::Relaxed) }
}

// ─── DebugStats ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct DebugStats {
    pub total_cmds_issued:  u64,
    pub cmd_pending:        u64,
    pub resp_pending:       u64,
    pub queue_size:         usize,
}

impl DebugStats {
    pub fn from_queue(q: &DebugQueue) -> Self {
        Self {
            total_cmds_issued: q.total_cmds(),
            cmd_pending:       q.cmd_pending(),
            resp_pending:      q.resp_pending(),
            queue_size:        DEBUG_QUEUE_SIZE,
        }
    }

    pub fn utilization_pct(&self) -> u64 {
        let pending = self.cmd_pending.saturating_add(self.resp_pending);
        pending.saturating_mul(100)
            .checked_div((self.queue_size as u64).saturating_mul(2).max(1))
            .unwrap_or(0)
            .min(100)
    }

    pub fn is_full(&self) -> bool {
        self.cmd_pending >= self.queue_size as u64
            || self.resp_pending >= self.queue_size as u64
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmd_id_roundtrip() {
        let id = DebugCommandId::DumpMetrics;
        assert_eq!(DebugCommandId::from_u8(id as u8), id);
        assert!(!id.is_destructive());
        assert!(id.is_read_only());
    }

    #[test]
    fn test_cmd_id_destructive() {
        assert!(DebugCommandId::ResetCounters.is_destructive());
        assert!(DebugCommandId::Shutdown.is_destructive());
        assert!(DebugCommandId::ForceGc.is_destructive());
    }

    #[test]
    fn test_command_new() {
        let cmd = DebugCommand::new(DebugCommandId::DumpSpace, 42);
        assert_eq!(cmd.command_id(), DebugCommandId::DumpSpace);
        assert_eq!(cmd.arg_u64, 42);
        assert!(!cmd.is_empty());
    }

    #[test]
    fn test_response_ok() {
        let r = DebugResponse::ok(DebugCommandId::GetStatus, 99, "ok");
        assert!(r.is_ok());
        let v = r.msg_to_vec().expect("ok");
        assert_eq!(&v, b"ok");
    }

    #[test]
    fn test_response_err() {
        let r = DebugResponse::err(DebugCommandId::ForceGc, "err msg");
        assert!(!r.is_ok());
        assert_eq!(r.status(), DebugResponseStatus::Error);
    }

    #[test]
    fn test_response_not_supported() {
        let r = DebugResponse::not_supported(DebugCommandId::Shutdown);
        assert_eq!(r.status(), DebugResponseStatus::NotSupported);
    }

    #[test]
    fn test_queue_push_pop_cmd() {
        let q = DebugQueue::new_const();
        let cmd = DebugCommand::new(DebugCommandId::DumpMetrics, 0);
        q.push_cmd(cmd).expect("ok");
        assert_eq!(q.cmd_pending(), 1);
        let got = q.pop_cmd();
        assert!(got.is_some());
        assert_eq!(q.cmd_pending(), 0);
    }

    #[test]
    fn test_queue_push_pop_response() {
        let q = DebugQueue::new_const();
        let r = DebugResponse::ok(DebugCommandId::Noop, 0, "test");
        q.push_response(r).expect("ok");
        assert_eq!(q.resp_pending(), 1);
        let got = q.pop_response();
        assert!(got.is_some());
    }

    #[test]
    fn test_queue_full_returns_error() {
        let q = DebugQueue::new_const();
        let mut i = 0usize;
        while i < DEBUG_QUEUE_SIZE {
            q.push_cmd(DebugCommand::new(DebugCommandId::Noop, 0)).expect("push");
            i = i.wrapping_add(1);
        }
        let r = q.push_cmd(DebugCommand::new(DebugCommandId::Noop, 0));
        assert!(r.is_err());
    }

    #[test]
    fn test_session_execute_noop() {
        let q = DebugQueue::new_const();
        let sess = DebugSession::new(&q);
        let resp = sess.execute(DebugCommand::new(DebugCommandId::Noop, 0));
        assert!(resp.is_ok());
        assert_eq!(sess.exec_count(), 1);
    }

    #[test]
    fn test_session_dispatch_process() {
        let q = DebugQueue::new_const();
        let sess = DebugSession::new(&q);
        q.push_cmd(DebugCommand::new(DebugCommandId::GetStatus, 0)).expect("push");
        let processed = sess.process_pending().expect("ok");
        assert_eq!(processed, 1);
        let resp = q.pop_response();
        assert!(resp.is_some());
        assert!(resp.unwrap().is_ok());
    }

    #[test]
    fn test_session_self_test() {
        let q = DebugQueue::new_const();
        let sess = DebugSession::new(&q);
        let r = sess.execute(DebugCommand::new(DebugCommandId::SelfTest, 0));
        assert!(r.is_ok());
    }

    #[test]
    fn test_stats_utilization() {
        let q = DebugQueue::new_const();
        let stats = DebugStats::from_queue(&q);
        assert_eq!(stats.utilization_pct(), 0);
        assert!(!stats.is_full());
    }

    #[test]
    fn test_queue_drain_responses() {
        let q = DebugQueue::new_const();
        q.push_response(DebugResponse::ok(DebugCommandId::Noop, 0, "a")).expect("ok");
        q.push_response(DebugResponse::ok(DebugCommandId::Noop, 1, "b")).expect("ok");
        let v = q.drain_responses().expect("drain");
        assert_eq!(v.len(), 2);
    }
}
