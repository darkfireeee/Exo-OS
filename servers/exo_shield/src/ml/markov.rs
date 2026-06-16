//! Chaîne de Markov ordre-2 sur les EventType pour ExoShield.
//!
//! Suit les trigrammes (e_{t-2}, e_{t-1}, e_t) par PID.
//! Calcule la "surprise" = -log₂(P(e_t | e_{t-2}, e_{t-1})) quand un type
//! d'événement inhabituel est observé dans son contexte.
//!
//! 9 catégories d'EventType × 9 × 9 = 729 compteurs de trigrammes.
//! Lissage de Laplace (prior=1) : pas de surprise infinie, prêt dès le boot.
//! Apprend en ligne : la surprise diminue sur les patterns récurrents.
//! État par PID : 64 slots max, éviction LRU simplifiée par modulo.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Nombre de catégories d'EventType (doit matcher behavioral::sequence::EventType)
pub const MARKOV_CATS: usize = 9;
/// PIDs suivis simultanément
pub const MARKOV_MAX_PIDS: usize = 64;
/// Lissage de Laplace : count initial
const LAPLACE: u32 = 1;

// ── -log₂(n/d) en Q16.16 ────────────────────────────────────────────────────

fn neg_log2_ratio(num: u32, denom: u32) -> i32 {
    if denom == 0 { return 20 << 16; }
    if num >= denom { return 0; }
    if num == 0 { return 20 << 16; }

    // -log₂(n/d) = floor_log₂(d) - floor_log₂(n), approximation entière
    let fl_d = (31u32.saturating_sub(denom.leading_zeros())) as i32;
    let fl_n = (31u32.saturating_sub(num.leading_zeros())) as i32;
    ((fl_d - fl_n) << 16).clamp(0, 20 << 16)
}

// ── État PID ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct PidState {
    pid: u32,
    e1: u8,     // event 2 steps avant
    e2: u8,     // event 1 step avant
    active: bool,
}

impl PidState {
    const fn empty() -> Self {
        Self { pid: 0, e1: 0, e2: 0, active: false }
    }
}

// ── Chaîne de Markov ─────────────────────────────────────────────────────────

pub struct MarkovChain {
    /// counts[e1 * 81 + e2 * 9 + e3]
    counts: [u32; MARKOV_CATS * MARKOV_CATS * MARKOV_CATS],
    /// bigram[e1 * 9 + e2] = Σ_{e3} counts[e1][e2][e3]
    bigram: [u32; MARKOV_CATS * MARKOV_CATS],
    pid_states: [PidState; MARKOV_MAX_PIDS],
    total_events: u64,
}

impl MarkovChain {
    pub const fn new() -> Self {
        Self {
            // Prior uniforme de Laplace
            counts: [LAPLACE; MARKOV_CATS * MARKOV_CATS * MARKOV_CATS],
            bigram: [LAPLACE * MARKOV_CATS as u32; MARKOV_CATS * MARKOV_CATS],
            pid_states: [PidState::empty(); MARKOV_MAX_PIDS],
            total_events: 0,
        }
    }

    #[inline]
    fn cat(et: u8) -> usize {
        (et as usize).min(MARKOV_CATS - 1)
    }

    fn pid_slot(&mut self, pid: u32) -> usize {
        // Cherche slot existant
        for i in 0..MARKOV_MAX_PIDS {
            if self.pid_states[i].active && self.pid_states[i].pid == pid {
                return i;
            }
        }
        // Slot libre
        for i in 0..MARKOV_MAX_PIDS {
            if !self.pid_states[i].active {
                self.pid_states[i] = PidState { pid, e1: 0, e2: 0, active: true };
                return i;
            }
        }
        // Éviction déterministe par pid % MAX_PIDS
        let slot = (pid as usize) % MARKOV_MAX_PIDS;
        self.pid_states[slot] = PidState { pid, e1: 0, e2: 0, active: true };
        slot
    }

    /// Observe un événement pour un PID.
    /// Retourne la surprise Q16.16 [0, 20<<16] AVANT mise à jour des compteurs.
    pub fn observe(&mut self, pid: u32, event_type: u8) -> i32 {
        let slot = self.pid_slot(pid);
        let e1 = self.pid_states[slot].e1 as usize;
        let e2 = self.pid_states[slot].e2 as usize;
        let e3 = Self::cat(event_type);

        let surprise = neg_log2_ratio(
            self.counts[e1 * 81 + e2 * 9 + e3],
            self.bigram[e1 * 9 + e2],
        );

        // Mise à jour des compteurs
        self.counts[e1 * 81 + e2 * 9 + e3] =
            self.counts[e1 * 81 + e2 * 9 + e3].saturating_add(1);
        self.bigram[e1 * 9 + e2] =
            self.bigram[e1 * 9 + e2].saturating_add(1);

        // Avance l'état PID
        self.pid_states[slot].e1 = e2 as u8;
        self.pid_states[slot].e2 = e3 as u8;
        self.total_events = self.total_events.wrapping_add(1);

        surprise
    }

    /// Surprise pour un trigramme (lecture seule, sans update).
    pub fn surprise_for(&self, e1: u8, e2: u8, e3: u8) -> i32 {
        let i1 = Self::cat(e1);
        let i2 = Self::cat(e2);
        let i3 = Self::cat(e3);
        neg_log2_ratio(
            self.counts[i1 * 81 + i2 * 9 + i3],
            self.bigram[i1 * 9 + i2],
        )
    }

    /// Efface l'état d'un PID (appeler à la mort du processus).
    pub fn clear_pid(&mut self, pid: u32) {
        for i in 0..MARKOV_MAX_PIDS {
            if self.pid_states[i].active && self.pid_states[i].pid == pid {
                self.pid_states[i].active = false;
                return;
            }
        }
    }

    pub fn total_events(&self) -> u64 { self.total_events }
}

// ── Normalisation ─────────────────────────────────────────────────────────────

/// Normalise la surprise brute (0..20<<16) en Q16.16 [0, 65536].
/// ≥ 8 bits de surprise → 65536 (anomalie maximale).
pub fn normalize_surprise(s: i32) -> i32 {
    const MAX_BITS: i32 = 8 << 16;
    if s >= MAX_BITS { return 1 << 16; }
    if s <= 0 { return 0; }
    ((s as i64 * 65_536) / (MAX_BITS as i64)) as i32
}

// ── Static global ─────────────────────────────────────────────────────────────

static MARKOV: Mutex<MarkovChain> = Mutex::new(MarkovChain::new());
static MARKOV_READY: AtomicBool = AtomicBool::new(false);

/// Markov démarre prêt (prior Laplace uniforme, pas d'init nécessaire).
pub fn markov_init() {
    MARKOV_READY.store(true, Ordering::Release);
}

/// Observe un événement pour un PID. Retourne surprise normalisée Q16.16 [0, 65536].
pub fn markov_observe(pid: u32, event_type: u8) -> i32 {
    if !MARKOV_READY.load(Ordering::Acquire) { return 0; }
    let raw = MARKOV.lock().observe(pid, event_type);
    normalize_surprise(raw)
}

/// Efface l'état d'un PID (fin de processus).
pub fn markov_clear_pid(pid: u32) {
    if MARKOV_READY.load(Ordering::Acquire) {
        MARKOV.lock().clear_pid(pid);
    }
}

pub fn markov_total_events() -> u64 {
    MARKOV.lock().total_events()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markov_uniform_prior_gives_moderate_surprise() {
        let m = MarkovChain::new();
        // Avec prior uniforme P(e3|e1,e2) = 1/9 → surprise ≈ 3 bits
        let s = m.surprise_for(0, 0, 0);
        // floor_log2(9)=3, floor_log2(1)=0 → 3*65536=196608 en Q16.16
        assert!(s >= 2 << 16, "surprise={}", s);
        assert!(s <= 20 << 16);
    }

    #[test]
    fn markov_repeated_pattern_lowers_surprise() {
        let mut m = MarkovChain::new();
        // Renforcer Syscall→FileAccess→Syscall 100 fois
        for _ in 0..100 {
            m.observe(1, 0); // Syscall
            m.observe(1, 4); // FileAccess
            m.observe(1, 0); // Syscall
        }
        let after = m.surprise_for(0, 4, 0); // Syscall|FileAccess|Syscall : courant
        let novel = m.surprise_for(0, 1, 2); // Syscall|MemAccess|NetConnect : rare
        assert!(after < novel, "after={} novel={}", after, novel);
    }

    #[test]
    fn markov_novel_sequence_higher_surprise() {
        let mut m = MarkovChain::new();
        // Renforcer FileAccess→FileAccess→FileAccess massivement
        for _ in 0..1000 {
            m.observe(42, 4);
            m.observe(42, 4);
            m.observe(42, 4);
        }
        let normal = m.surprise_for(4, 4, 4); // pattern commun
        let novel = m.surprise_for(4, 4, 5);  // PrivChange inattendu
        assert!(novel > normal, "novel={} normal={}", novel, normal);
    }

    #[test]
    fn markov_pid_states_independent() {
        let mut m = MarkovChain::new();
        m.observe(1, 0); m.observe(1, 0);
        m.observe(2, 4); m.observe(2, 4);
        let s1 = m.observe(1, 0);
        let s2 = m.observe(2, 4);
        let s3 = m.observe(3, 5); // PID 3, première observation
        // Toutes valeurs bornées
        assert!(s1 >= 0 && s1 <= 20 << 16);
        assert!(s2 >= 0 && s2 <= 20 << 16);
        assert!(s3 >= 0 && s3 <= 20 << 16);
    }

    #[test]
    fn markov_normalize_bounds() {
        assert_eq!(normalize_surprise(0), 0);
        assert_eq!(normalize_surprise(8 << 16), 1 << 16);
        assert_eq!(normalize_surprise(20 << 16), 1 << 16); // clamp
        let mid = normalize_surprise(4 << 16);
        assert!(mid >= (1 << 15) - 1000 && mid <= (1 << 15) + 1000, "mid={}", mid);
    }

    #[test]
    fn markov_clear_pid_removes_state() {
        let mut m = MarkovChain::new();
        m.observe(99, 3);
        m.observe(99, 3);
        m.clear_pid(99);
        let active = m.pid_states.iter().any(|s| s.active && s.pid == 99);
        assert!(!active, "PID 99 devrait être effacé");
    }

    #[test]
    fn markov_neg_log2_ratio_monotone() {
        // Plus n/d est petit, plus la surprise est grande
        let s_half = neg_log2_ratio(1, 2);   // P=0.5 → 1 bit
        let s_ninth = neg_log2_ratio(1, 9);  // P≈0.11 → ≈3 bits
        assert!(s_ninth > s_half, "ninth={} half={}", s_ninth, s_half);
    }
}
