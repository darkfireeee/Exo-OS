//! `ProcessControlBlock` constructors, credential/state accessors, and thread registry ops.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::process::core::pid::Pid;
use crate::process::signal::default::SigHandlerTable;
use crate::scheduler::core::task::ThreadId;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::security::capability::table::CapTable;

use super::types::try_box_new;
use super::{
    process_flags, types, Credentials, OpenFileTable, ProcessControlBlock, ProcessState,
    DEFAULT_UMASK, PROCESS_NAME_LEN,
};

impl ProcessControlBlock {
    /// Creates a new PCB for `fork()` / `create_process()`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pid: Pid,
        ppid: Pid,
        tgid: Pid,
        main_thread: ThreadId,
        creds: Credentials,
        fd_limit: usize,
        cr3: u64,
        addr_space: usize,
    ) -> Box<Self> {
        Self::try_new(
            pid,
            ppid,
            tgid,
            main_thread,
            creds,
            fd_limit,
            cr3,
            addr_space,
        )
        .expect("ProcessControlBlock::new: allocation échouée")
    }

    /// Creates a PCB without panicking on OOM.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        pid: Pid,
        ppid: Pid,
        tgid: Pid,
        main_thread: ThreadId,
        creds: Credentials,
        fd_limit: usize,
        cr3: u64,
        addr_space: usize,
    ) -> Option<Box<Self>> {
        let files = OpenFileTable::try_new(fd_limit)?;
        try_box_new(ProcessControlBlock {
            pid,
            name: SpinLock::new([0u8; PROCESS_NAME_LEN]),
            ppid: AtomicU32::new(ppid.0),
            tgid,
            sid: AtomicU32::new(0),
            pgid: AtomicU32::new(pid.0),
            state: AtomicU32::new(ProcessState::Creating as u32),
            flags: AtomicU32::new(0),
            exit_code: AtomicU32::new(0),
            umask: AtomicU32::new(DEFAULT_UMASK),
            pdeathsig: AtomicU32::new(0),
            personality: AtomicU32::new(0),
            thread_count: AtomicU32::new(1),
            max_threads: AtomicU32::new(types::DEFAULT_MAX_THREADS_PER_PROCESS),
            main_thread,
            threads: SpinLock::new(Vec::new()),
            creds: SpinLock::new(creds),
            euid_atomic: AtomicU32::new(creds.euid),
            files: Arc::new(SpinLock::new(files)),
            address_space: AtomicUsize::new(addr_space),
            as_lock: SpinLock::new(()),
            cr3: AtomicU64::new(cr3),
            brk_current: AtomicU64::new(0),
            brk_start: AtomicU64::new(0),
            utime_ns: AtomicU64::new(0),
            stime_ns: AtomicU64::new(0),
            major_faults: AtomicU64::new(0),
            minor_faults: AtomicU64::new(0),
            io_read_bytes: AtomicU64::new(0),
            io_write_bytes: AtomicU64::new(0),
            sig_handlers: SpinLock::new(SigHandlerTable::new()),
            signal_tcb_vaddr: AtomicU64::new(0),
            main_thread_rawptr: AtomicPtr::new(core::ptr::null_mut()),
            pid_ns: 0,
            mnt_ns: 0,
            net_ns: 0,
            uts_ns: 0,
            user_ns: 0,
            cap_table: Box::new(CapTable::new()),
        })
    }

    #[inline]
    fn copy_normalized_name(dst: &mut [u8; PROCESS_NAME_LEN], src: &[u8]) -> usize {
        let mut written = 0usize;
        while written < PROCESS_NAME_LEN && written < src.len() {
            let b = src[written];
            if b == 0 {
                break;
            }
            dst[written] = if b == b'-' { b'_' } else { b };
            written += 1;
        }
        written
    }

    pub fn set_name_bytes(&self, name: &[u8]) {
        let mut out = [0u8; PROCESS_NAME_LEN];
        let written = Self::copy_normalized_name(&mut out, name);
        if written == 0 {
            let _ = Self::copy_normalized_name(&mut out, b"process");
        }
        *self.name.lock() = out;
    }

    pub fn set_name_from_path(&self, path: &[u8]) {
        let mut end = 0usize;
        while end < path.len() && path[end] != 0 {
            end += 1;
        }

        let mut start = 0usize;
        let mut i = 0usize;
        while i < end {
            if path[i] == b'/' {
                start = i + 1;
            }
            i += 1;
        }

        let base = &path[start..end];
        if base.is_empty() {
            self.set_name_bytes(b"process");
            return;
        }

        if base == b"exo-shield" {
            self.set_name_bytes(base);
            return;
        }

        if base == b"exo-compositor" {
            self.set_name_bytes(b"exo_compositor");
            return;
        }

        if base.starts_with(b"exo-") && base.ends_with(b"-server") && base.len() > 11 {
            let service = &base[4..base.len() - 7];
            let mut out = [0u8; PROCESS_NAME_LEN];
            let mut written = Self::copy_normalized_name(&mut out, service);
            if written < PROCESS_NAME_LEN {
                out[written] = b'_';
                written += 1;
            }
            let suffix = b"server";
            let mut s = 0usize;
            while written < PROCESS_NAME_LEN && s < suffix.len() {
                out[written] = suffix[s];
                written += 1;
                s += 1;
            }
            *self.name.lock() = out;
            return;
        }

        if base.starts_with(b"exo-") && base.len() > 4 {
            self.set_name_bytes(&base[4..]);
        } else {
            self.set_name_bytes(base);
        }
    }

    #[inline]
    pub fn name_snapshot(&self) -> [u8; PROCESS_NAME_LEN] {
        *self.name.lock()
    }

    #[inline(always)]
    pub fn state(&self) -> ProcessState {
        unsafe { core::mem::transmute(self.state.load(Ordering::Acquire) as u8) }
    }

    #[inline(always)]
    pub fn set_state(&self, s: ProcessState) {
        self.state.store(s as u32, Ordering::Release);
    }

    #[inline(always)]
    pub fn is_zombie(&self) -> bool {
        self.state() == ProcessState::Zombie
    }

    #[inline(always)]
    pub fn is_exiting(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & process_flags::EXITING != 0
    }

    #[inline(always)]
    pub fn set_exiting(&self) {
        self.flags
            .fetch_or(process_flags::EXITING, Ordering::Release);
    }

    #[inline(always)]
    pub fn has_no_new_privs(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & process_flags::NO_NEW_PRIVS) != 0
    }

    #[inline(always)]
    pub fn set_no_new_privs(&self) {
        self.flags
            .fetch_or(process_flags::NO_NEW_PRIVS, Ordering::Release);
    }

    #[inline(always)]
    pub fn is_dumpable(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & process_flags::NO_DUMP) == 0
    }

    #[inline(always)]
    pub fn set_dumpable(&self, dumpable: bool) {
        if dumpable {
            self.flags
                .fetch_and(!process_flags::NO_DUMP, Ordering::Release);
        } else {
            self.flags
                .fetch_or(process_flags::NO_DUMP, Ordering::Release);
        }
    }

    #[inline(always)]
    pub fn pdeathsig(&self) -> u32 {
        self.pdeathsig.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub fn set_pdeathsig(&self, sig: u32) {
        self.pdeathsig.store(sig, Ordering::Release);
    }

    #[inline(always)]
    pub fn personality(&self) -> u32 {
        self.personality.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub fn set_personality(&self, persona: u32) -> u32 {
        self.personality.swap(persona, Ordering::AcqRel)
    }

    #[inline(always)]
    pub fn umask(&self) -> u32 {
        self.umask.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub fn swap_umask(&self, mask: u32) -> u32 {
        self.umask.swap(mask & 0o777, Ordering::AcqRel)
    }

    #[inline(always)]
    pub fn inc_threads(&self) -> u32 {
        self.thread_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    #[inline(always)]
    pub fn dec_threads(&self) -> u32 {
        self.thread_count.fetch_sub(1, Ordering::Relaxed) - 1
    }

    #[inline(always)]
    pub fn add_utime(&self, ns: u64) {
        self.utime_ns.fetch_add(ns, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn add_stime(&self, ns: u64) {
        self.stime_ns.fetch_add(ns, Ordering::Relaxed);
    }

    pub fn is_root(&self) -> bool {
        self.creds.lock().is_root()
    }

    #[inline]
    pub fn get_creds(&self) -> Credentials {
        *self.creds.lock()
    }

    #[inline]
    pub fn set_uid(&self, uid: u32) {
        let mut c = self.creds.lock();
        c.uid = uid;
        c.fsuid = uid;
    }

    #[inline]
    pub fn set_gid(&self, gid: u32) {
        let mut c = self.creds.lock();
        c.gid = gid;
        c.fsgid = gid;
    }

    #[inline]
    pub fn set_euid(&self, euid: u32) {
        self.creds.lock().euid = euid;
        self.euid_atomic.store(euid, Ordering::Release);
    }

    #[inline(always)]
    pub fn euid_fast(&self) -> u32 {
        self.euid_atomic.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_egid(&self, egid: u32) {
        self.creds.lock().egid = egid;
    }

    #[inline]
    pub fn set_fsuid(&self, fsuid: u32) {
        self.creds.lock().fsuid = fsuid;
    }

    #[inline]
    pub fn set_fsgid(&self, fsgid: u32) {
        self.creds.lock().fsgid = fsgid;
    }

    #[inline]
    pub fn set_resuid(&self, ruid: u32, euid: u32, suid: u32) {
        let mut c = self.creds.lock();
        if ruid != u32::MAX {
            c.uid = ruid;
            c.fsuid = ruid;
        }
        if euid != u32::MAX {
            c.euid = euid;
            self.euid_atomic.store(euid, Ordering::Release);
        }
        if suid != u32::MAX {
            c.suid = suid;
        }
    }

    #[inline]
    pub fn set_resgid(&self, rgid: u32, egid: u32, sgid: u32) {
        let mut c = self.creds.lock();
        if rgid != u32::MAX {
            c.gid = rgid;
            c.fsgid = rgid;
        }
        if egid != u32::MAX {
            c.egid = egid;
        }
        if sgid != u32::MAX {
            c.sgid = sgid;
        }
    }

    #[inline(always)]
    pub fn address_space_ptr(&self) -> *mut u8 {
        self.address_space.load(Ordering::Relaxed) as *mut u8
    }

    #[inline(always)]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    #[inline(always)]
    pub fn ppid(&self) -> Pid {
        Pid(self.ppid.load(Ordering::Acquire))
    }

    #[inline(always)]
    pub fn pgroup_id(&self) -> u32 {
        self.pgid.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub fn set_pgroup_id(&self, pgid: u32) {
        self.pgid.store(pgid, Ordering::Release);
    }

    #[inline(always)]
    pub fn session_id(&self) -> u32 {
        self.sid.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub fn set_session_id(&self, sid: u32) {
        self.sid.store(sid, Ordering::Release);
    }

    #[inline(always)]
    pub fn is_session_leader(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & process_flags::SESSION_LEADER != 0
    }

    #[inline(always)]
    pub fn is_pgroup_leader(&self) -> bool {
        self.pgid.load(Ordering::Acquire) == self.pid.0
    }

    #[inline(always)]
    pub fn main_thread_ptr(&self) -> *mut crate::process::core::tcb::ProcessThread {
        self.main_thread_rawptr.load(Ordering::Acquire)
    }

    pub fn register_thread_ptr(&self, ptr: *mut crate::process::core::tcb::ProcessThread) -> bool {
        if ptr.is_null() {
            return false;
        }
        let limit = self.max_threads.load(Ordering::Acquire).max(1) as usize;
        let mut threads = self.threads.lock();
        if !threads.contains(&ptr) {
            if threads.len() >= limit || threads.try_reserve(1).is_err() {
                return false;
            }
            threads.push(ptr);
        }
        true
    }

    pub fn unregister_thread_ptr(&self, ptr: *mut crate::process::core::tcb::ProcessThread) {
        if ptr.is_null() {
            return;
        }
        let mut threads = self.threads.lock();
        threads.retain(|&p| p != ptr);
    }

    pub fn take_thread_by_tid(
        &self,
        tid: crate::process::core::pid::Tid,
    ) -> *mut crate::process::core::tcb::ProcessThread {
        let mut found = core::ptr::null_mut();
        {
            let mut threads = self.threads.lock();
            if let Some(idx) = threads
                .iter()
                .position(|&p| !p.is_null() && unsafe { (*p).tid == tid })
            {
                found = threads.swap_remove(idx);
            }
        }
        if !found.is_null() {
            let _ = self.main_thread_rawptr.compare_exchange(
                found,
                core::ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            return found;
        }
        let main_ptr = self.main_thread_rawptr.load(Ordering::Acquire);
        if !main_ptr.is_null()
            && unsafe { (*main_ptr).tid == tid }
            && self
                .main_thread_rawptr
                .compare_exchange(
                    main_ptr,
                    core::ptr::null_mut(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        {
            return main_ptr;
        }
        core::ptr::null_mut()
    }

    pub fn for_each_thread_ptr<F>(&self, mut f: F)
    where
        F: FnMut(*mut crate::process::core::tcb::ProcessThread),
    {
        let snapshot: Vec<*mut crate::process::core::tcb::ProcessThread> =
            self.threads.lock().clone();
        for ptr in snapshot {
            if !ptr.is_null() {
                f(ptr);
            }
        }
    }

    #[inline]
    pub fn find_thread_by_tid(
        &self,
        tid: crate::process::core::pid::Tid,
    ) -> *mut crate::process::core::tcb::ProcessThread {
        let main_ptr = self.main_thread_rawptr.load(Ordering::Acquire);
        if !main_ptr.is_null() && unsafe { (*main_ptr).tid == tid } {
            return main_ptr;
        }
        let threads = self.threads.lock();
        for &ptr in threads.iter() {
            if !ptr.is_null() && unsafe { (*ptr).tid == tid } {
                return ptr;
            }
        }
        core::ptr::null_mut()
    }

    #[inline]
    fn is_thread_alive(ptr: *mut crate::process::core::tcb::ProcessThread) -> bool {
        if ptr.is_null() {
            return false;
        }
        !matches!(
            unsafe { (&*ptr).state() },
            crate::scheduler::core::task::TaskState::Zombie
                | crate::scheduler::core::task::TaskState::Dead
        )
    }

    #[inline]
    pub fn find_alive_thread(&self) -> *mut crate::process::core::tcb::ProcessThread {
        let main_ptr = self.main_thread_ptr();
        if Self::is_thread_alive(main_ptr) {
            return main_ptr;
        }
        let threads = self.threads.lock();
        for &ptr in threads.iter() {
            if Self::is_thread_alive(ptr) {
                return ptr;
            }
        }
        core::ptr::null_mut()
    }

    #[inline(always)]
    pub fn set_main_thread_ptr(&self, ptr: *mut crate::process::core::tcb::ProcessThread) {
        self.main_thread_rawptr.store(ptr, Ordering::Release);
        if !ptr.is_null() {
            let _ = self.register_thread_ptr(ptr);
        }
    }

    pub fn cleanup_all_threads(&self) {
        let main_ptr = self
            .main_thread_rawptr
            .swap(core::ptr::null_mut(), Ordering::AcqRel);
        let ptrs = core::mem::take(&mut *self.threads.lock());

        for ptr in ptrs {
            if !ptr.is_null() && ptr != main_ptr {
                unsafe {
                    let thread_box = alloc::boxed::Box::from_raw(ptr);
                    let tid = thread_box.tid;
                    drop(thread_box);
                    crate::process::core::pid::TID_ALLOCATOR.free(tid.0);
                }
            }
        }

        if !main_ptr.is_null() {
            unsafe {
                let thread_box = alloc::boxed::Box::from_raw(main_ptr);
                let tid = thread_box.tid;
                drop(thread_box);
                crate::process::core::pid::TID_ALLOCATOR.free(tid.0);
            }
        }
    }
}

impl Drop for ProcessControlBlock {
    fn drop(&mut self) {
        self.cleanup_all_threads();
    }
}
