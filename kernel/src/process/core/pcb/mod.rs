//! # process/core/pcb/mod.rs — ProcessControlBlock (Exo-OS Couche 1.5)
//!
//! Le PCB est partagé entre tous les threads d'un même processus.
//! Accès protégé par un spinlock interne ; les champs atomiques sont lus
//! sans verrou depuis les hot paths (scheduler, signal delivery).
//!
//! RÈGLES :
//!   • Jamais d'import fs/ ou ipc/ directs — traits abstraits uniquement.
//!   • PCB libéré via RAII (Drop) avec cleanup ordonné.
//!   • Les champs file_table et mmap_regions sont des index opaques
//!     enregistrés as handle — pas de type concret fs/.

pub mod file_table;
pub mod types;

pub use file_table::{FileDescriptor, OpenFileTable};
pub use types::{
    process_flags, Credentials, ProcessFlags, ProcessState, DEFAULT_UMASK, MAX_THREADS_PER_PROCESS,
    PROCESS_NAME_LEN,
};

use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use crate::process::core::pid::Pid;
use crate::process::signal::default::SigHandlerTable;
use crate::scheduler::core::task::ThreadId;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::security::capability::table::CapTable;
use types::try_box_new;

/// Process Control Block — structure centrale par processus.
///
/// Partagée entre tous les threads du processus.
/// Protégée par son propre spinlock pour les champs mutables non atomiques.
pub struct ProcessControlBlock {
    // ── Identité ──────────────────────────────────────────────────────────────
    /// PID du processus.
    pub pid: Pid,
    /// Nom court issu du binaire exécuté, exposé par SYS_EXO_PROCESS_LIST.
    pub name: SpinLock<[u8; PROCESS_NAME_LEN]>,
    /// PID du processus parent.
    pub ppid: AtomicU32,
    /// PID du leader de thread group (= pid du premier thread, tgid POSIX).
    pub tgid: Pid,
    /// Session ID.
    pub sid: AtomicU32,
    /// Process group ID.
    pub pgid: AtomicU32,

    // ── État ──────────────────────────────────────────────────────────────────
    /// État courant du processus.
    pub state: AtomicU32, // ProcessState as u32
    /// Flags de processus (process_flags::*).
    pub flags: AtomicU32,
    /// Code de sortie (renseigné par exit()).
    pub exit_code: AtomicU32,
    /// Umask POSIX par processus.
    pub umask: AtomicU32,

    // ── Threads ───────────────────────────────────────────────────────────────
    /// Nombre de threads actifs dans ce processus.
    pub thread_count: AtomicU32,
    /// ID du thread principal (TID = PID au sens POSIX).
    pub main_thread: ThreadId,
    /// Registre borné des threads du processus, utilisé par exit_group().
    pub thread_slots:
        [AtomicPtr<crate::process::core::tcb::ProcessThread>; MAX_THREADS_PER_PROCESS],

    // ── Ressources ────────────────────────────────────────────────────────────
    /// Credentials (uid/gid...).
    pub creds: SpinLock<Credentials>,
    /// Table des fichiers ouverts.
    pub files: SpinLock<OpenFileTable>,

    // ── Mémoire virtuelle ──────────────────────────────────────────────────────
    /// Pointeur opaque vers l'espace d'adressage virtuel (géré par memory/virt/).
    pub address_space: AtomicUsize, // *mut opaque
    /// CR3 courant (base de la PML4 physique).
    pub cr3: AtomicU64,
    /// Break courant absolu du heap utilisateur.
    pub brk_current: AtomicU64,
    /// Break initial absolu publié par le chargeur ELF.
    pub brk_start: AtomicU64,

    // ── Compteurs de performance ───────────────────────────────────────────────
    /// Temps CPU utilisateur total (ns).
    pub utime_ns: AtomicU64,
    /// Temps CPU système total (ns).
    pub stime_ns: AtomicU64,
    /// Major page faults (swap-in).
    pub major_faults: AtomicU64,
    /// Minor page faults (demand paging).
    pub minor_faults: AtomicU64,
    /// Octets lus depuis des devices.
    pub io_read_bytes: AtomicU64,
    /// Octets écrits vers des devices.
    pub io_write_bytes: AtomicU64,

    // ── Signaux ────────────────────────────────────────────────────────────────
    /// Table des handlers de signaux installés (partagée entre tous les threads).
    pub sig_handlers: SpinLock<SigHandlerTable>,
    /// Pointeur vers le ProcessThread principal (TID = PID).
    pub main_thread_rawptr: AtomicPtr<crate::process::core::tcb::ProcessThread>,

    // ── Namespaces ────────────────────────────────────────────────────────────
    /// Index (handle) dans la table de PID namespaces.
    pub pid_ns: u32,
    /// Index dans la table de mount namespaces.
    pub mnt_ns: u32,
    /// Index dans la table de net namespaces.
    pub net_ns: u32,
    /// Index dans la table d'UTS namespaces.
    pub uts_ns: u32,
    /// Index dans la table de user namespaces.
    pub user_ns: u32,

    // ── Capabilities (FIX-P2-A03) ─────────────────────────────────────────
    /// Table de capabilities propre au processus.
    pub cap_table: Box<CapTable>,
}

impl ProcessControlBlock {
    /// Crée un nouveau PCB pour `fork()` / `create_process()`.
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

    /// Crée un PCB sans paniquer en cas d'OOM.
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
            thread_count: AtomicU32::new(1),
            main_thread,
            thread_slots: [const { AtomicPtr::new(core::ptr::null_mut()) };
                MAX_THREADS_PER_PROCESS],
            creds: SpinLock::new(creds),
            files: SpinLock::new(files),
            address_space: AtomicUsize::new(addr_space),
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
            main_thread_rawptr: AtomicPtr::new(core::ptr::null_mut()),
            pid_ns: 0,
            mnt_ns: 0,
            net_ns: 0,
            uts_ns: 0,
            user_ns: 0,
            cap_table: Box::new(CapTable::new()),
        })
    }

    // ── Accesseurs d'état ──────────────────────────────────────────────────────

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
        // SAFETY: seules des valeurs ProcessState valides sont stockées via set_state.
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

    /// Lit l'umask POSIX du processus.
    #[inline(always)]
    pub fn umask(&self) -> u32 {
        self.umask.load(Ordering::Acquire)
    }

    /// Remplace l'umask POSIX et retourne l'ancienne valeur.
    #[inline(always)]
    pub fn swap_umask(&self, mask: u32) -> u32 {
        self.umask.swap(mask & 0o777, Ordering::AcqRel)
    }

    /// Incrémente le compteur de threads actifs.
    #[inline(always)]
    pub fn inc_threads(&self) -> u32 {
        self.thread_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Décrémente et retourne le nouveau compteur.
    #[inline(always)]
    pub fn dec_threads(&self) -> u32 {
        self.thread_count.fetch_sub(1, Ordering::Relaxed) - 1
    }

    /// Accumule le temps CPU utilisateur (appelé lors de context switch).
    #[inline(always)]
    pub fn add_utime(&self, ns: u64) {
        self.utime_ns.fetch_add(ns, Ordering::Relaxed);
    }

    /// Accumule le temps CPU système.
    #[inline(always)]
    pub fn add_stime(&self, ns: u64) {
        self.stime_ns.fetch_add(ns, Ordering::Relaxed);
    }

    /// Retourne true si le processus est root (euid == 0).
    pub fn is_root(&self) -> bool {
        self.creds.lock().is_root()
    }

    // ────────────────────────────────────────────────────────────────────────────
    // Mutateurs de credentials POSIX
    // ────────────────────────────────────────────────────────────────────────────

    /// Retourne une copie des credentials courants.
    #[inline]
    pub fn get_creds(&self) -> Credentials {
        *self.creds.lock()
    }

    /// `setuid(uid)` — met à jour uid et fsuid.
    #[inline]
    pub fn set_uid(&self, uid: u32) {
        let mut c = self.creds.lock();
        c.uid = uid;
        c.fsuid = uid;
    }

    /// `setgid(gid)` — met à jour gid et fsgid.
    #[inline]
    pub fn set_gid(&self, gid: u32) {
        let mut c = self.creds.lock();
        c.gid = gid;
        c.fsgid = gid;
    }

    /// `seteuid(euid)`.
    #[inline]
    pub fn set_euid(&self, euid: u32) {
        self.creds.lock().euid = euid;
    }

    /// `setegid(egid)`.
    #[inline]
    pub fn set_egid(&self, egid: u32) {
        self.creds.lock().egid = egid;
    }

    /// `setfsuid(fsuid)`.
    #[inline]
    pub fn set_fsuid(&self, fsuid: u32) {
        self.creds.lock().fsuid = fsuid;
    }

    /// `setfsgid(fsgid)`.
    #[inline]
    pub fn set_fsgid(&self, fsgid: u32) {
        self.creds.lock().fsgid = fsgid;
    }

    /// `setresuid(ruid, euid, suid)` — -1 signifie "ne pas changer".
    #[inline]
    pub fn set_resuid(&self, ruid: u32, euid: u32, suid: u32) {
        let mut c = self.creds.lock();
        if ruid != u32::MAX {
            c.uid = ruid;
            c.fsuid = ruid;
        }
        if euid != u32::MAX {
            c.euid = euid;
        }
        if suid != u32::MAX {
            c.suid = suid;
        }
    }

    /// `setresgid(rgid, egid, sgid)` — u32::MAX signifie "ne pas changer".
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

    /// Pointeur vers l'espace d'adressage (opaque).
    #[inline(always)]
    pub fn address_space_ptr(&self) -> *mut u8 {
        self.address_space.load(Ordering::Relaxed) as *mut u8
    }

    // ────────────────────────────────────────────────────────────────────────────
    // Accesseurs PID / groupe / session
    // ────────────────────────────────────────────────────────────────────────────

    /// PID du processus.
    #[inline(always)]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// PID du parent.
    #[inline(always)]
    pub fn ppid(&self) -> Pid {
        Pid(self.ppid.load(Ordering::Acquire))
    }

    /// PGID.
    #[inline(always)]
    pub fn pgroup_id(&self) -> u32 {
        self.pgid.load(Ordering::Acquire)
    }

    /// Définit le PGID.
    #[inline(always)]
    pub fn set_pgroup_id(&self, pgid: u32) {
        self.pgid.store(pgid, Ordering::Release);
    }

    /// SID.
    #[inline(always)]
    pub fn session_id(&self) -> u32 {
        self.sid.load(Ordering::Acquire)
    }

    /// Définit le SID.
    #[inline(always)]
    pub fn set_session_id(&self, sid: u32) {
        self.sid.store(sid, Ordering::Release);
    }

    /// Vrai si ce processus est leader de session (SID == PID).
    #[inline(always)]
    pub fn is_session_leader(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & process_flags::SESSION_LEADER != 0
    }

    /// Vrai si ce processus est leader de groupe (PGID == PID).
    #[inline(always)]
    pub fn is_pgroup_leader(&self) -> bool {
        self.pgid.load(Ordering::Acquire) == self.pid.0
    }

    /// Pointeur vers le thread principal du processus (TID = PID).
    #[inline(always)]
    pub fn main_thread_ptr(&self) -> *mut crate::process::core::tcb::ProcessThread {
        self.main_thread_rawptr.load(Ordering::Acquire)
    }

    /// Enregistre un thread dans le registre borné du processus.
    pub fn register_thread_ptr(&self, ptr: *mut crate::process::core::tcb::ProcessThread) -> bool {
        if ptr.is_null() {
            return false;
        }
        for slot in &self.thread_slots {
            if slot.load(Ordering::Acquire) == ptr {
                return true;
            }
        }
        for slot in &self.thread_slots {
            if slot
                .compare_exchange(
                    core::ptr::null_mut(),
                    ptr,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
        false
    }

    /// Retire un thread du registre borné.
    pub fn unregister_thread_ptr(&self, ptr: *mut crate::process::core::tcb::ProcessThread) {
        if ptr.is_null() {
            return;
        }
        for slot in &self.thread_slots {
            let _ = slot.compare_exchange(
                ptr,
                core::ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    /// Parcourt les threads connus du processus.
    pub fn for_each_thread_ptr<F>(&self, mut f: F)
    where
        F: FnMut(*mut crate::process::core::tcb::ProcessThread),
    {
        for slot in &self.thread_slots {
            let ptr = slot.load(Ordering::Acquire);
            if !ptr.is_null() {
                f(ptr);
            }
        }
    }

    /// Retourne un thread vivant pouvant recevoir un signal.
    #[inline]
    pub fn find_alive_thread(&self) -> *mut crate::process::core::tcb::ProcessThread {
        let ptr = self.main_thread_ptr();
        if ptr.is_null() {
            return core::ptr::null_mut();
        }
        let state = unsafe { (&*ptr).state() };
        match state {
            crate::scheduler::core::task::TaskState::Zombie
            | crate::scheduler::core::task::TaskState::Dead => core::ptr::null_mut(),
            _ => ptr,
        }
    }

    /// Définit le pointeur vers le thread principal.
    #[inline(always)]
    pub fn set_main_thread_ptr(&self, ptr: *mut crate::process::core::tcb::ProcessThread) {
        self.main_thread_rawptr.store(ptr, Ordering::Release);
        if !ptr.is_null() {
            let _ = self.register_thread_ptr(ptr);
        }
    }

    /// Libère tous les threads enregistrés dans ce PCB.
    pub fn cleanup_all_threads(&self) {
        let main_ptr = self.main_thread_rawptr.swap(core::ptr::null_mut(), Ordering::AcqRel);

        for slot in &self.thread_slots {
            let ptr = slot.swap(core::ptr::null_mut(), Ordering::AcqRel);
            if !ptr.is_null() && ptr != main_ptr {
                // SAFETY: thread secondaire alloué par Box::into_raw.
                // Le thread est Dead et retiré de toute runqueue.
                unsafe {
                    let thread_box = alloc::boxed::Box::from_raw(ptr);
                    let tid = thread_box.tid;
                    drop(thread_box);
                    crate::process::core::pid::TID_ALLOCATOR.free(tid.0);
                }
            }
        }

        if !main_ptr.is_null() {
            // SAFETY: main thread alloué par Box::into_raw.
            // Le processus est Dead/Zombie et n'est plus exécuté par aucun CPU.
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

// SAFETY: ProcessControlBlock est partagé entre threads du même processus.
// Tous les champs mutables sont soit atomiques, soit protégés par SpinLock.
unsafe impl Send for ProcessControlBlock {}
unsafe impl Sync for ProcessControlBlock {}
