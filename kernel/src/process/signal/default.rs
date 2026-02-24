// kernel/src/process/signal/default.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Définitions POSIX des signaux et actions par défaut (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::fmt;

// ─────────────────────────────────────────────────────────────────────────────
// Numéros de signal POSIX
// ─────────────────────────────────────────────────────────────────────────────

/// Signal POSIX (numéros Linux x86_64).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[repr(u8)]
pub enum Signal {
    SIGHUP    =  1,
    SIGINT    =  2,
    SIGQUIT   =  3,
    SIGILL    =  4,
    SIGTRAP   =  5,
    SIGABRT   =  6,  // = SIGIOT
    SIGBUS    =  7,
    SIGFPE    =  8,
    SIGKILL   =  9,
    SIGUSR1   = 10,
    SIGSEGV   = 11,
    SIGUSR2   = 12,
    SIGPIPE   = 13,
    SIGALRM   = 14,
    SIGTERM   = 15,
    SIGSTKFLT = 16,
    SIGCHLD   = 17,
    SIGCONT   = 18,
    SIGSTOP   = 19,
    SIGTSTP   = 20,
    SIGTTIN   = 21,
    SIGTTOU   = 22,
    SIGURG    = 23,
    SIGXCPU   = 24,
    SIGXFSZ   = 25,
    SIGVTALRM = 26,
    SIGPROF   = 27,
    SIGWINCH  = 28,
    SIGIO     = 29,  // = SIGPOLL
    SIGPWR    = 30,
    SIGSYS    = 31,
}

impl Signal {
    pub const NSIG: usize = 64;
    pub const SIGRTMIN: u8 = 32;
    pub const SIGRTMAX: u8 = 63;

    #[inline(always)]
    pub fn from_u8(n: u8) -> Option<Self> {
        if n == 0 || n > 31 { return None; }
        // SAFETY: n est dans [1..31], un discriminant valide de Signal.
        Some(unsafe { core::mem::transmute(n) })
    }

    #[inline(always)]
    pub fn number(self) -> u8 { self as u8 }

    /// Vrai si ce signal est un signal temps-réel (SIGRT*).
    #[inline(always)]
    pub fn is_realtime(n: u8) -> bool {
        n >= Self::SIGRTMIN && n <= Self::SIGRTMAX
    }

    /// Ce signal peut-il être bloqué ?
    #[inline(always)]
    pub fn is_blockable(self) -> bool {
        !matches!(self, Signal::SIGKILL | Signal::SIGSTOP)
    }

    /// Ce signal peut-il être ignoré ?
    #[inline(always)]
    pub fn is_ignorable(self) -> bool {
        !matches!(self, Signal::SIGKILL | Signal::SIGSTOP)
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Signal::SIGHUP    => "SIGHUP",
            Signal::SIGINT    => "SIGINT",
            Signal::SIGQUIT   => "SIGQUIT",
            Signal::SIGILL    => "SIGILL",
            Signal::SIGTRAP   => "SIGTRAP",
            Signal::SIGABRT   => "SIGABRT",
            Signal::SIGBUS    => "SIGBUS",
            Signal::SIGFPE    => "SIGFPE",
            Signal::SIGKILL   => "SIGKILL",
            Signal::SIGUSR1   => "SIGUSR1",
            Signal::SIGSEGV   => "SIGSEGV",
            Signal::SIGUSR2   => "SIGUSR2",
            Signal::SIGPIPE   => "SIGPIPE",
            Signal::SIGALRM   => "SIGALRM",
            Signal::SIGTERM   => "SIGTERM",
            Signal::SIGCHLD   => "SIGCHLD",
            Signal::SIGCONT   => "SIGCONT",
            Signal::SIGSTOP   => "SIGSTOP",
            Signal::SIGTSTP   => "SIGTSTP",
            Signal::SIGTTIN   => "SIGTTIN",
            Signal::SIGTTOU   => "SIGTTOU",
            Signal::SIGSYS    => "SIGSYS",
            _                 => "SIG???",
        };
        write!(f, "{name}")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Actions par défaut (POSIX)
// ─────────────────────────────────────────────────────────────────────────────

/// Action sur réception d'un signal.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum SigActionKind {
    /// Terminer le processus.
    Term   = 0,
    /// Terminer + générer core dump.
    Core   = 1,
    /// Ignorer le signal.
    Ignore = 2,
    /// Arrêter le processus (SIGSTOP).
    Stop   = 3,
    /// Reprendre le processus (SIGCONT).
    Cont   = 4,
    /// Handler utilisateur installé.
    User   = 5,
}

/// Description complète d'une action associée à un signal.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct SigAction {
    /// Action courante.
    pub kind:     SigActionKind,
    /// Adresse du handler utilisateur (0 si SIG_DFL / SIG_IGN).
    pub handler:  u64,
    /// Drapeaux SA_RESTART, SA_NODEFER, SA_SIGINFO...
    pub flags:    u32,
    /// Masque additionnel pendant l'exécution du handler.
    pub mask:     u64,
    /// Adresse de la fonction de nettoyage (SA_RESTORER).
    pub restorer: u64,
}

impl SigAction {
    pub const DFL: Self   = Self { kind: SigActionKind::Term,   handler: 0, flags: 0, mask: 0, restorer: 0 };
    pub const IGN: Self   = Self { kind: SigActionKind::Ignore, handler: 0, flags: 0, mask: 0, restorer: 0 };
    pub const STOP: Self  = Self { kind: SigActionKind::Stop,   handler: 0, flags: 0, mask: 0, restorer: 0 };
    pub const CONT: Self  = Self { kind: SigActionKind::Cont,   handler: 0, flags: 0, mask: 0, restorer: 0 };
    pub const CORE: Self  = Self { kind: SigActionKind::Core,   handler: 0, flags: 0, mask: 0, restorer: 0 };

    pub const SA_RESTART:  u32 = 1 << 28;
    pub const SA_NODEFER:  u32 = 1 << 30;
    pub const SA_SIGINFO:  u32 = 1 << 2;
    pub const SA_ONSTACK:  u32 = 1 << 3;
    pub const SA_RESETHAND: u32 = 1 << 31;

    pub fn is_user_handler(&self) -> bool {
        self.kind == SigActionKind::User && self.handler != 0
    }
}

/// Table des actions par défaut pour les signaux 1..31.
/// Index i = signal numéro i (signal 0 = unused).
pub const DEFAULT_ACTIONS: [SigAction; 32] = [
    SigAction::DFL,   //  0  unused
    SigAction::DFL,   //  1  SIGHUP   → Term
    SigAction::DFL,   //  2  SIGINT   → Term
    SigAction::CORE,  //  3  SIGQUIT  → Core
    SigAction::CORE,  //  4  SIGILL   → Core
    SigAction::CORE,  //  5  SIGTRAP  → Core
    SigAction::CORE,  //  6  SIGABRT  → Core
    SigAction::CORE,  //  7  SIGBUS   → Core
    SigAction::CORE,  //  8  SIGFPE   → Core
    SigAction::DFL,   //  9  SIGKILL  → Term (non-bloquable)
    SigAction::DFL,   // 10  SIGUSR1  → Term
    SigAction::CORE,  // 11  SIGSEGV  → Core
    SigAction::DFL,   // 12  SIGUSR2  → Term
    SigAction::DFL,   // 13  SIGPIPE  → Term
    SigAction::DFL,   // 14  SIGALRM  → Term
    SigAction::DFL,   // 15  SIGTERM  → Term
    SigAction::DFL,   // 16  SIGSTKFLT → Term
    SigAction::IGN,   // 17  SIGCHLD  → Ignore
    SigAction::CONT,  // 18  SIGCONT  → Cont
    SigAction::STOP,  // 19  SIGSTOP  → Stop
    SigAction::STOP,  // 20  SIGTSTP  → Stop
    SigAction::STOP,  // 21  SIGTTIN  → Stop
    SigAction::STOP,  // 22  SIGTTOU  → Stop
    SigAction::IGN,   // 23  SIGURG   → Ignore
    SigAction::DFL,   // 24  SIGXCPU  → Term (+ core sur Linux)
    SigAction::DFL,   // 25  SIGXFSZ  → Term (+ core sur Linux)
    SigAction::DFL,   // 26  SIGVTALRM→ Term
    SigAction::DFL,   // 27  SIGPROF  → Term
    SigAction::IGN,   // 28  SIGWINCH → Ignore
    SigAction::DFL,   // 29  SIGIO    → Term
    SigAction::DFL,   // 30  SIGPWR   → Term
    SigAction::CORE,  // 31  SIGSYS   → Core
];

/// Retourne l'action par défaut d'un signal standard.
#[inline]
pub fn default_action(sig: u8) -> SigAction {
    if sig < 32 { DEFAULT_ACTIONS[sig as usize] }
    else { SigAction::DFL } // RT signals → Term par défaut
}

/// Table des actions installées pour un processus (SigAction par signal).
/// Partagé entre threads (les handlers sont globaux au processus).
#[repr(C)]
pub struct SigHandlerTable {
    /// Slots 0..63 (signal 0 = unused, 1..31 = standard, 32..63 = RT).
    actions: [SigAction; 64],
}

impl SigHandlerTable {
    /// Crée une table avec les actions par défaut.
    pub const fn new() -> Self {
        let mut actions = [SigAction::DFL; 64];
        // Les 32 premiers = DEFAULT_ACTIONS.
        let mut i = 0usize;
        while i < 32 {
            actions[i] = DEFAULT_ACTIONS[i];
            i += 1;
        }
        Self { actions }
    }

    /// Obtient l'action courante pour un signal.
    #[inline(always)]
    pub fn get(&self, sig: u8) -> SigAction {
        if (sig as usize) < 64 { self.actions[sig as usize] }
        else { SigAction::DFL }
    }

    /// Définit l'action (sigaction()).
    /// Retourne l'ancienne action.
    #[inline]
    pub fn set(&mut self, sig: u8, action: SigAction) -> SigAction {
        if (sig as usize) >= 64 { return SigAction::DFL; }
        let old = self.actions[sig as usize];
        self.actions[sig as usize] = action;
        old
    }

    /// Réinitialise tous les handlers non-ignorés à SIG_DFL (appelé par execve).
    pub fn reset_on_exec(&mut self) {
        for i in 0..64usize {
            if self.actions[i].kind == SigActionKind::User {
                self.actions[i] = if i < 32 { DEFAULT_ACTIONS[i] } else { SigAction::DFL };
            }
        }
    }
}
