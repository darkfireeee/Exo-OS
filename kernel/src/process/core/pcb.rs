// kernel/src/process/core/pcb.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessControlBlock — Structure de contrôle d'un processus (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le PCB est partagé entre tous les threads d'un même processus.
// Accès protégé par un spinlock interne ; les champs atomiques sont lus
// sans verrou depuis les hot paths (scheduler, signal delivery).
//
// RÈGLES :
//   • Jamais d'import fs/ ou ipc/ directs — traits abstraits uniquement.
//   • PCB libéré via RAII (Drop) avec cleanup ordonné.
//   • Les champs file_table et mmap_regions sont des index opaques
//     enregistrés as handle — pas de type concret fs/.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, AtomicPtr, Ordering};
use alloc::vec::Vec;
use alloc::boxed::Box;
use super::pid::Pid;
use crate::scheduler::core::task::ThreadId;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::process::signal::default::SigHandlerTable;

// ─────────────────────────────────────────────────────────────────────────────
// ProcessState — machine d'états du processus
// ─────────────────────────────────────────────────────────────────────────────

/// État du processus (distinct de l'état des threads individuels).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ProcessState {
    /// En cours de création (fork / execve non terminé).
    Creating    = 0,
    /// Au moins un thread actif.
    Running     = 1,
    /// Tous les threads bloqués.
    Sleeping    = 2,
    /// Arrêté via SIGSTOP (tous threads Stopped).
    Stopped     = 3,
    /// exit() appelé, en attente de waitpid() par le parent.
    Zombie      = 4,
    /// Ressources libérées (après reap).
    Dead        = 5,
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessFlags
// ─────────────────────────────────────────────────────────────────────────────

pub mod process_flags {
    /// fork() par copy-on-write (CLONE_VM absent).
    pub const FORKED:           u32 = 1 << 0;
    /// execve() effectué au moins une fois.
    pub const EXEC_DONE:        u32 = 1 << 1;
    /// Processus leader de session.
    pub const SESSION_LEADER:   u32 = 1 << 2;
    /// Processus démonisé (no ctty).
    pub const DAEMON:           u32 = 1 << 3;
    /// Undergoing exit — ne pas envoyer de nouveaux signaux.
    pub const EXITING:          u32 = 1 << 4;
    /// Setuid actif.
    pub const SETUID:           u32 = 1 << 5;
    /// Setgid actif.
    pub const SETGID:           u32 = 1 << 6;
    /// Dump core interdit (prctl(PR_SET_DUMPABLE, 0)).
    pub const NO_DUMP:          u32 = 1 << 7;
    /// Processus soumis à ptrace.
    pub const TRACED:           u32 = 1 << 8;
    /// Processus dans un namespace PID (non-init).
    pub const IN_PID_NS:        u32 = 1 << 9;
    /// Vfork en attente — libérer le parent bloqué.
    pub const VFORK_DONE:       u32 = 1 << 10;
}

pub use process_flags as ProcessFlags;

// ─────────────────────────────────────────────────────────────────────────────
// OpenFileTable — table des descripteurs de fichiers ouverts
// Opaque : les handles sont des indices dans la table fs/.
// Pas d'import fs/ direct — RÈGLE PROC-01.
// ─────────────────────────────────────────────────────────────────────────────

/// Un descripteur de fichier : opaque (géré par fs/ via handle entier).
#[derive(Copy, Clone, Debug)]
pub struct FileDescriptor {
    /// Numéro fd (0=stdin, 1=stdout, 2=stderr, 3+...).
    pub fd:     i32,
    /// Handle opaque vers l'entrée fs/ (index dans la table vfs).
    pub handle: u64,
    /// Flags O_CLOEXEC, O_NONBLOCK...
    pub flags:  u32,
}

/// Table des fichiers ouverts d'un processus (partagée entre threads via fork+CLONE_FILES).
pub struct OpenFileTable {
    /// fd_limit par processus (configurable via rlimit RLIMIT_NOFILE).
    fd_limit: usize,
    /// Descripteurs actifs. Index = numéro fd.
    descriptors: Vec<Option<FileDescriptor>>,
    /// Prochain fd à essayer en premier (hint, pas garanti).
    next_hint: usize,
    /// Compteur d'ouvertures cumulées.
    open_count: AtomicU64,
    /// Compteur de fermetures cumulées.
    close_count: AtomicU64,
}

impl OpenFileTable {
    /// Crée une table vide avec les 3 fds standards pré-réservés (mais vides).
    pub fn new(fd_limit: usize) -> Self {
        let mut descriptors = Vec::with_capacity(32.min(fd_limit));
        // Allouer slots 0,1,2 pour stdin/stdout/stderr (initialement None).
        descriptors.push(None); // stdin
        descriptors.push(None); // stdout
        descriptors.push(None); // stderr
        Self {
            fd_limit,
            descriptors,
            next_hint: 3,
            open_count:  AtomicU64::new(0),
            close_count: AtomicU64::new(0),
        }
    }

    /// Installe le triplet stdin/stdout/stderr.
    pub fn install_std_fds(&mut self, stdin: u64, stdout: u64, stderr: u64) {
        self.descriptors[0] = Some(FileDescriptor { fd: 0, handle: stdin,  flags: 0 });
        self.descriptors[1] = Some(FileDescriptor { fd: 1, handle: stdout, flags: 0 });
        self.descriptors[2] = Some(FileDescriptor { fd: 2, handle: stderr, flags: 0 });
    }

    /// Alloue le prochain fd disponible et y associe le handle.
    /// Retourne le numéro fd alloué ou -1 si la table est pleine.
    pub fn install(&mut self, handle: u64, flags: u32) -> i32 {
        let start = self.next_hint;
        let limit = self.fd_limit;

        // Scanner à partir du hint.
        for idx in start..limit {
            if idx < self.descriptors.len() {
                if self.descriptors[idx].is_none() {
                    self.descriptors[idx] = Some(FileDescriptor { fd: idx as i32, handle, flags });
                    self.next_hint = idx + 1;
                    self.open_count.fetch_add(1, Ordering::Relaxed);
                    return idx as i32;
                }
            } else {
                // Étendre le vecteur.
                self.descriptors.push(Some(FileDescriptor { fd: idx as i32, handle, flags }));
                self.next_hint = idx + 1;
                self.open_count.fetch_add(1, Ordering::Relaxed);
                return idx as i32;
            }
        }
        // Rescan depuis 0 au cas où il y a des trous avant start.
        for idx in 3..start {
            if idx < self.descriptors.len() && self.descriptors[idx].is_none() {
                self.descriptors[idx] = Some(FileDescriptor { fd: idx as i32, handle, flags });
                self.next_hint = idx + 1;
                self.open_count.fetch_add(1, Ordering::Relaxed);
                return idx as i32;
            }
        }
        -1 // EMFILE
    }

    /// Ferme le fd donné. Retourne le handle associé pour que fs/ puisse fermer le fichier.
    pub fn close(&mut self, fd: i32) -> Option<u64> {
        if fd < 0 || fd as usize >= self.descriptors.len() {
            return None;
        }
        let entry = self.descriptors[fd as usize].take()?;
        if (fd as usize) < self.next_hint {
            self.next_hint = fd as usize;
        }
        self.close_count.fetch_add(1, Ordering::Relaxed);
        Some(entry.handle)
    }

    /// Lit le handle associé à un fd (sans fermer).
    #[inline(always)]
    pub fn get(&self, fd: i32) -> Option<&FileDescriptor> {
        self.descriptors.get(fd as usize)?.as_ref()
    }

    /// Ferme tous les fds marqués O_CLOEXEC (appelé lors de execve).
    pub fn close_on_exec(&mut self) -> Vec<u64> {
        const O_CLOEXEC: u32 = 0x80000;
        let mut closed_handles = Vec::new();
        for slot in &mut self.descriptors {
            if let Some(fd_entry) = slot {
                if fd_entry.flags & O_CLOEXEC != 0 {
                    closed_handles.push(fd_entry.handle);
                    *slot = None;
                }
            }
        }
        self.next_hint = 3;
        closed_handles
    }

    /// Clone la table pour fork() (les handles sont dupliqués).
    pub fn clone_for_fork(&self) -> Self {
        Self {
            fd_limit:    self.fd_limit,
            descriptors: self.descriptors.clone(),
            next_hint:   self.next_hint,
            open_count:  AtomicU64::new(self.open_count.load(Ordering::Relaxed)),
            close_count: AtomicU64::new(0),
        }
    }

    /// Nombre de fds ouverts actuellement.
    pub fn open_fd_count(&self) -> usize {
        self.descriptors.iter().filter(|s| s.is_some()).count()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Credentials — UID/GID
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiants d'un processus (clones entre fork, remplacés par setuid/setgid).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct Credentials {
    pub uid:  u32,
    pub gid:  u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
    /// Filesystem UID/GID (Linux-compat).
    pub fsuid: u32,
    pub fsgid: u32,
}

impl Credentials {
    pub const ROOT: Self = Self {
        uid: 0, gid: 0, euid: 0, egid: 0,
        suid: 0, sgid: 0, fsuid: 0, fsgid: 0,
    };

    pub fn new(uid: u32, gid: u32) -> Self {
        Self { uid, gid, euid: uid, egid: gid, suid: uid, sgid: gid, fsuid: uid, fsgid: gid }
    }

    #[inline(always)]
    pub fn is_root(&self) -> bool {
        self.euid == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessControlBlock
// ─────────────────────────────────────────────────────────────────────────────

/// Process Control Block — structure centrale par processus.
///
/// Partagée entre tous les threads du processus.
/// Protégée par son propre spinlock pour les champs mutables non atomiques.
pub struct ProcessControlBlock {
    // ── Identité ──────────────────────────────────────────────────────────────
    /// PID du processus.
    pub pid:            Pid,
    /// PID du processus parent.
    pub ppid:           AtomicU32,
    /// PID du leader de thread group (= pid du premier thread, tgid POSIX).
    pub tgid:           Pid,
    /// Session ID.
    pub sid:            AtomicU32,
    /// Process group ID.
    pub pgid:           AtomicU32,

    // ── État ──────────────────────────────────────────────────────────────────
    /// État courant du processus.
    pub state:          AtomicU32,   // ProcessState as u32
    /// Flags de processus (process_flags::*).
    pub flags:          AtomicU32,
    /// Code de sortie (renseigné par exit()).
    pub exit_code:      AtomicU32,

    // ── Threads ───────────────────────────────────────────────────────────────
    /// Nombre de threads actifs dans ce processus.
    pub thread_count:   AtomicU32,
    /// ID du thread principal (TID = PID au sens POSIX).
    pub main_thread:    ThreadId,

    // ── Ressources ────────────────────────────────────────────────────────────
    /// Credentials (uid/gid...).
    pub creds:          SpinLock<Credentials>,
    /// Table des fichiers ouverts.
    pub files:          SpinLock<OpenFileTable>,

    // ── Mémoire virtuelle ──────────────────────────────────────────────────────
    /// Pointeur opaque vers l'espace d'adressage virtuel (géré par memory/virt/).
    /// Type réel : *mut memory::virt::address_space::user::UserAddressSpace.
    pub address_space:  AtomicUsize,  // *mut opaque
    /// CR3 courant (base de la PML4 physique).
    pub cr3:            AtomicU64,
    /// taille du heap brk courant (bytes au-dessus de brk_start).
    pub brk_current:    AtomicU64,
    pub brk_start:      AtomicU64,

    // ── Compteurs de performance ───────────────────────────────────────────────
    /// Temps CPU utilisateur total (ns).
    pub utime_ns:       AtomicU64,
    /// Temps CPU système total (ns).
    pub stime_ns:       AtomicU64,
    /// Major page faults (swap-in).
    pub major_faults:   AtomicU64,
    /// Minor page faults (demand paging).
    pub minor_faults:   AtomicU64,
    /// Octets lus depuis des devices.
    pub io_read_bytes:  AtomicU64,
    /// Octets écrits vers des devices.
    pub io_write_bytes: AtomicU64,

    // ── Signaux ────────────────────────────────────────────────────────────────
    /// Table des handlers de signaux installés (partagée entre tous les threads).
    pub sig_handlers:   SpinLock<SigHandlerTable>,
    /// Pointeur vers le ProcessThread principal (TID = PID).
    pub main_thread_rawptr: AtomicPtr<crate::process::core::tcb::ProcessThread>,

    // ── Namespaces ────────────────────────────────────────────────────────────
    /// Index (handle) dans la table de PID namespaces.
    pub pid_ns:         u32,
    /// Index dans la table de mount namespaces.
    pub mnt_ns:         u32,
    /// Index dans la table de net namespaces.
    pub net_ns:         u32,
    /// Index dans la table d'UTS namespaces.
    pub uts_ns:         u32,
    /// Index dans la table de user namespaces.
    pub user_ns:        u32,
}

impl ProcessControlBlock {
    /// Crée un nouveau PCB pour `fork()` / `create_process()`.
    pub fn new(
        pid:         Pid,
        ppid:        Pid,
        tgid:        Pid,
        main_thread: ThreadId,
        creds:       Credentials,
        fd_limit:    usize,
        cr3:         u64,
        addr_space:  usize,
    ) -> Box<Self> {
        Box::new(ProcessControlBlock {
            pid,
            ppid:           AtomicU32::new(ppid.0),
            tgid,
            sid:            AtomicU32::new(0),
            pgid:           AtomicU32::new(pid.0),
            state:          AtomicU32::new(ProcessState::Creating as u32),
            flags:          AtomicU32::new(0),
            exit_code:      AtomicU32::new(0),
            thread_count:   AtomicU32::new(1),
            main_thread,
            creds:          SpinLock::new(creds),
            files:          SpinLock::new(OpenFileTable::new(fd_limit)),
            address_space:  AtomicUsize::new(addr_space),
            cr3:            AtomicU64::new(cr3),
            brk_current:    AtomicU64::new(0),
            brk_start:      AtomicU64::new(0),
            utime_ns:       AtomicU64::new(0),
            stime_ns:       AtomicU64::new(0),
            major_faults:   AtomicU64::new(0),
            minor_faults:   AtomicU64::new(0),
            io_read_bytes:  AtomicU64::new(0),
            io_write_bytes: AtomicU64::new(0),
            sig_handlers:   SpinLock::new(SigHandlerTable::new()),
            main_thread_rawptr: AtomicPtr::new(core::ptr::null_mut()),
            pid_ns:  0,
            mnt_ns:  0,
            net_ns:  0,
            uts_ns:  0,
            user_ns: 0,
        })
    }

    // ── Accesseurs d'état ──────────────────────────────────────────────────────

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
        self.flags.fetch_or(process_flags::EXITING, Ordering::Release);
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
        c.uid   = uid;
        c.fsuid = uid;
    }

    /// `setgid(gid)` — met à jour gid et fsgid.
    #[inline]
    pub fn set_gid(&self, gid: u32) {
        let mut c = self.creds.lock();
        c.gid   = gid;
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
        // Convention POSIX : (u32::MAX) = ne pas modifier
        if ruid != u32::MAX { c.uid  = ruid; c.fsuid = ruid; }
        if euid != u32::MAX { c.euid = euid; }
        if suid != u32::MAX { c.suid = suid; }
    }

    /// `setresgid(rgid, egid, sgid)` — u32::MAX signifie "ne pas changer".
    #[inline]
    pub fn set_resgid(&self, rgid: u32, egid: u32, sgid: u32) {
        let mut c = self.creds.lock();
        if rgid != u32::MAX { c.gid  = rgid; c.fsgid = rgid; }
        if egid != u32::MAX { c.egid = egid; }
        if sgid != u32::MAX { c.sgid = sgid; }
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
    pub fn pid(&self) -> Pid { self.pid }

    /// PID du parent.
    #[inline(always)]
    pub fn ppid(&self) -> Pid { Pid(self.ppid.load(Ordering::Acquire)) }

    /// PGID.
    #[inline(always)]
    pub fn pgroup_id(&self) -> u32 { self.pgid.load(Ordering::Acquire) }

    /// Définit le PGID.
    #[inline(always)]
    pub fn set_pgroup_id(&self, pgid: u32) { self.pgid.store(pgid, Ordering::Release); }

    /// SID.
    #[inline(always)]
    pub fn session_id(&self) -> u32 { self.sid.load(Ordering::Acquire) }

    /// Définit le SID.
    #[inline(always)]
    pub fn set_session_id(&self, sid: u32) { self.sid.store(sid, Ordering::Release); }

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
    /// Null si pas encore initialisé.
    #[inline(always)]
    pub fn main_thread_ptr(
        &self,
    ) -> *mut crate::process::core::tcb::ProcessThread {
        self.main_thread_rawptr.load(Ordering::Acquire)
    }

    /// Définit le pointeur vers le thread principal.
    #[inline(always)]
    pub fn set_main_thread_ptr(
        &self,
        ptr: *mut crate::process::core::tcb::ProcessThread,
    ) {
        self.main_thread_rawptr.store(ptr, Ordering::Release);
    }
}

// SAFETY: ProcessControlBlock est partagé entre threads du même processus.
// Tous les champs mutables sont soit atomiques, soit protégés par SpinLock.
unsafe impl Send for ProcessControlBlock {}
unsafe impl Sync for ProcessControlBlock {}
