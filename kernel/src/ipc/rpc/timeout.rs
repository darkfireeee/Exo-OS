// ipc/rpc/timeout.rs — Gestion des timeouts RPC pour Exo-OS
//
// Ce module implémente la politique de timeout et de retry pour les appels RPC :
//   - `RpcTimeout` : configuration de timeout + backoff exponentiel
//   - `RpcDeadline` : échéance absolue calculée à partir d'un timestamp de boot
//   - `RetryPolicy` : politique de retry (N tentatives + backoff)
//   - Constantes : RPC_DEFAULT_TIMEOUT_NS, RPC_MAX_RETRIES, etc.
//
// RÈGLE TIMEOUT-01 : pas de timer hardware ici — le module lit le compteur
//                    TSC via crate::arch::tsc::rdtsc() (ou une function pointer).
// RÈGLE TIMEOUT-02 : backoff exponentiel borné par RPC_MAX_BACKOFF_NS.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::ipc::core::types::IpcError;

// ---------------------------------------------------------------------------
// Constantes de timeout
// ---------------------------------------------------------------------------

/// Timeout RPC par défaut : 5 ms en nanosecondes
pub const RPC_DEFAULT_TIMEOUT_NS: u64 = 5_000_000;

/// Timeout RPC minimum : 100 µs
pub const RPC_MIN_TIMEOUT_NS: u64 = 100_000;

/// Timeout RPC maximum : 1 s
pub const RPC_MAX_TIMEOUT_NS: u64 = 1_000_000_000;

/// Nombre maximum de tentatives par appel RPC (inclut la première)
pub const RPC_MAX_RETRIES: u32 = 3;

/// Délai initial de backoff : 1 ms
pub const RPC_INITIAL_BACKOFF_NS: u64 = 1_000_000;

/// Délai maximal de backoff : 50 ms
pub const RPC_MAX_BACKOFF_NS: u64 = 50_000_000;

/// Facteur de multiplication du backoff (en fixedpoint ×2)
pub const RPC_BACKOFF_FACTOR: u64 = 2;

// ---------------------------------------------------------------------------
// Source de temps
// ---------------------------------------------------------------------------

/// Type de fonction de lecture du temps courant (en nanosecondes depuis le boot)
pub type TimeFn = fn() -> u64;

/// Fonction de temps par défaut : retourne 0 (utilisable avant init de l'horloge)
fn time_zero() -> u64 { 0 }

/// Pointeur de fonction de temps configurable
/// Initialisé à 0 (cast de fn pointer interdit en const, installé au runtime).
static TIME_FN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Installe une fonction de lecture du temps.
/// Doit être appelée par l'arch layer lors de l'init du TSC.
pub fn install_time_fn(f: TimeFn) {
    TIME_FN.store(f as usize, Ordering::Release);
}

/// Lit le temps courant via la fonction installée.
/// Retourne 0 si l'horloge n'est pas encore initialisée (avant install_time_fn()).
pub fn now_ns() -> u64 {
    let f_ptr = TIME_FN.load(Ordering::Relaxed);
    if f_ptr == 0 {
        // Horloge non initialisée : retourner 0 (avant init du TSC).
        return 0;
    }
    // SAFETY: f_ptr a été stocké via install_time_fn() depuis une TimeFn valide.
    let f: TimeFn = unsafe { core::mem::transmute(f_ptr) };
    f()
}

// ---------------------------------------------------------------------------
// RpcTimeout — configuration de timeout
// ---------------------------------------------------------------------------

/// Configuration de timeout pour un appel RPC.
#[derive(Debug, Clone, Copy)]
pub struct RpcTimeout {
    /// Durée du timeout en ns (0 = utiliser RPC_DEFAULT_TIMEOUT_NS)
    pub duration_ns: u64,
    /// Timestamp de début (ns depuis boot), 0 = non initialisé
    pub start_ns: u64,
}

impl RpcTimeout {
    /// Timeout avec durée spécifique
    pub fn with_ns(duration_ns: u64) -> Self {
        let d = if duration_ns < RPC_MIN_TIMEOUT_NS {
            RPC_MIN_TIMEOUT_NS
        } else if duration_ns > RPC_MAX_TIMEOUT_NS {
            RPC_MAX_TIMEOUT_NS
        } else {
            duration_ns
        };
        Self { duration_ns: d, start_ns: now_ns() }
    }

    /// Timeout par défaut (5 ms)
    pub fn default() -> Self {
        Self::with_ns(RPC_DEFAULT_TIMEOUT_NS)
    }

    /// Crée un timeout infini (attente indéfinie)
    pub fn infinite() -> Self {
        Self { duration_ns: 0, start_ns: 0 }
    }

    /// Vérifie si le timeout est infini
    pub fn is_infinite(&self) -> bool {
        self.duration_ns == 0
    }

    /// Démarre le timeout (enregistre le timestamp de début)
    pub fn start(&mut self) {
        self.start_ns = now_ns();
    }

    /// Vérifie si le timeout est expiré
    pub fn is_expired(&self) -> bool {
        if self.is_infinite() { return false; }
        if self.start_ns == 0 { return false; }
        let elapsed = now_ns().saturating_sub(self.start_ns);
        elapsed >= self.duration_ns
    }

    /// Temps restant en ns (0 si expiré ou infini)
    pub fn remaining_ns(&self) -> u64 {
        if self.is_infinite() { return u64::MAX; }
        if self.start_ns == 0 { return self.duration_ns; }
        let elapsed = now_ns().saturating_sub(self.start_ns);
        self.duration_ns.saturating_sub(elapsed)
    }

    /// Nombre de spins suggérés pour attendre ce timeout.
    /// Heuristique : 1 ns ≈ 1 spin à ~1 GHz.
    pub fn to_spin_max(&self) -> u64 {
        if self.is_infinite() { return 0; }
        self.remaining_ns()
    }
}

// ---------------------------------------------------------------------------
// RpcDeadline — échéance absolue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct RpcDeadline {
    pub deadline_ns: u64,
}

impl RpcDeadline {
    pub fn from_timeout(t: &RpcTimeout) -> Self {
        if t.is_infinite() {
            return Self { deadline_ns: u64::MAX };
        }
        let start = if t.start_ns == 0 { now_ns() } else { t.start_ns };
        Self { deadline_ns: start.saturating_add(t.duration_ns) }
    }

    pub fn is_expired(&self) -> bool {
        now_ns() >= self.deadline_ns
    }

    pub fn remaining_ns(&self) -> u64 {
        self.deadline_ns.saturating_sub(now_ns())
    }
}

// ---------------------------------------------------------------------------
// RetryPolicy — politique de retry avec backoff exponentiel
// ---------------------------------------------------------------------------

/// État interne d'un retryeur.
pub struct RetryState {
    pub max_retries: u32,
    pub attempt: u32,
    pub backoff_ns: u64,
    pub timeout: RpcTimeout,
}

impl RetryState {
    /// Initialise une politique de retry avec timeout.
    pub fn new(max_retries: u32, timeout: RpcTimeout) -> Self {
        let n = if max_retries > RPC_MAX_RETRIES { RPC_MAX_RETRIES } else { max_retries };
        Self {
            max_retries: n,
            attempt: 0,
            backoff_ns: RPC_INITIAL_BACKOFF_NS,
            timeout,
        }
    }

    /// Politique par défaut (max retries = RPC_MAX_RETRIES, timeout = défaut)
    pub fn default() -> Self {
        Self::new(RPC_MAX_RETRIES, RpcTimeout::default())
    }

    /// Vérifie si une nouvelle tentative est possible.
    pub fn should_retry(&self) -> bool {
        self.attempt < self.max_retries && !self.timeout.is_expired()
    }

    /// Prépare la prochaine tentative.
    /// Retourne `Ok(wait_ns)` si retry possible, `Err(Timeout)` sinon.
    pub fn next_attempt(&mut self) -> Result<u64, IpcError> {
        if self.attempt >= self.max_retries {
            return Err(IpcError::Retry);
        }
        if self.timeout.is_expired() {
            return Err(IpcError::Timeout);
        }
        self.attempt += 1;
        let wait = self.backoff_ns;
        // Backoff exponentiel borné
        self.backoff_ns = self.backoff_ns
            .saturating_mul(RPC_BACKOFF_FACTOR)
            .min(RPC_MAX_BACKOFF_NS);
        Ok(wait)
    }

    /// Spin-wait pour `ns` nanosecondes (approximation par itérations).
    pub fn spin_wait_ns(ns: u64) {
        let spins = ns.min(10_000_000); // borne à 10M iterations max
        for _ in 0..spins {
            core::hint::spin_loop();
        }
    }

    /// Nombre de tentatives effectuées
    pub fn attempts(&self) -> u32 {
        self.attempt
    }
}

// ---------------------------------------------------------------------------
// Statistiques globales des timeouts RPC
// ---------------------------------------------------------------------------

pub struct RpcTimeoutStats {
    pub total_timeouts: AtomicU64,
    pub total_retries: AtomicU64,
    pub total_success_after_retry: AtomicU64,
}

impl RpcTimeoutStats {
    pub const fn new() -> Self {
        Self {
            total_timeouts: AtomicU64::new(0),
            total_retries: AtomicU64::new(0),
            total_success_after_retry: AtomicU64::new(0),
        }
    }

    pub fn record_timeout(&self) {
        self.total_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_retry(&self) {
        self.total_retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success_after_retry(&self) {
        self.total_success_after_retry.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> RpcTimeoutStatsSnapshot {
        RpcTimeoutStatsSnapshot {
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
            total_retries: self.total_retries.load(Ordering::Relaxed),
            total_success_after_retry: self.total_success_after_retry.load(Ordering::Relaxed),
        }
    }
}

pub static RPC_TIMEOUT_STATS: RpcTimeoutStats = RpcTimeoutStats::new();

#[derive(Debug, Clone, Copy)]
pub struct RpcTimeoutStatsSnapshot {
    pub total_timeouts: u64,
    pub total_retries: u64,
    pub total_success_after_retry: u64,
}
