// kernel/src/security/shield_feed.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ShieldFeed — anneau d'événements de sécurité kernel → exo_shield (TIER 3.1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le serveur NGAV `exo_shield` (Ring 1) est réactif : il traite les événements
// EVENT_REPORT/PMC_ANOMALY qu'on lui envoie. Sans alimentation, il est AVEUGLE.
//
// Ce module fournit le FEED : le kernel POUSSE (non-bloquant, best-effort) les
// événements de sécurité dans un ring borné ; le serveur les DRAINE via le
// syscall `SYS_EXO_SHIELD_DRAIN` et les injecte dans son moteur.
//
// CHOIX : feed ASYNCHRONE (ring + drain) plutôt qu'IPC synchrone par événement —
//   • coût push ≈ un lock court + copie 40 o (jamais d'IPC dans le hot-path) ;
//   • aucun deadlock kernel→userspace (le kernel n'attend jamais le serveur) ;
//   • best-effort : si le ring est plein (serveur en retard), on DROP et on
//     compte (`DROPPED`) — la sécurité kernel (capability/zero_trust/exoledger)
//     reste l'autorité ; le shield est une couche de détection complémentaire.
//
// RÈGLE SHIELDFEED-01 : `push` ne bloque JAMAIS et ne panique JAMAIS.
// RÈGLE SHIELDFEED-02 : les sites de push sont des événements de sécurité RÉELS
//   (refus capability/IPC/syscall, exec) — déjà journalisés ailleurs (exoledger).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// Capacité du ring (puissance de 2). 256 événements en attente de drain.
const RING_SIZE: usize = 256;

/// Catégorie d'événement — miroir de `engine::EventType` côté exo_shield.
/// Valeurs stables (ABI du feed).
pub mod event_type {
    pub const PROCESS: u8 = 0;
    pub const SYSCALL: u8 = 1;
    pub const MEMORY: u8 = 2;
    pub const NETWORK: u8 = 3;
    pub const IPC: u8 = 4;
    pub const CAPABILITY: u8 = 5;
}

/// Niveau de menace — miroir de `engine::ThreatLevel` côté exo_shield.
pub mod severity {
    pub const LOW: u8 = 0;
    pub const MEDIUM: u8 = 1;
    pub const HIGH: u8 = 2;
    pub const CRITICAL: u8 = 3;
}

/// Événement de sécurité forwardé au shield. Layout `#[repr(C)]` stable = ABI du
/// syscall de drain (le serveur lit exactement cette structure).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ShieldEvent {
    /// PID concerné par l'événement.
    pub pid: u32,
    /// Catégorie (`event_type::*`).
    pub event_type: u8,
    /// Sévérité (`severity::*`).
    pub severity: u8,
    pub _pad: [u8; 2],
    /// Code d'opération (n° de syscall, opcode réseau, action exec…).
    pub opcode: u32,
    pub _pad2: u32,
    /// Argument générique 0 (adresse, ppid, taille…).
    pub arg0: u64,
    /// Argument générique 1.
    pub arg1: u64,
    /// Numéro de séquence monotone (ordre + détection de perte côté serveur).
    pub seq: u64,
}

impl ShieldEvent {
    pub const fn zero() -> Self {
        Self {
            pid: 0,
            event_type: 0,
            severity: 0,
            _pad: [0; 2],
            opcode: 0,
            _pad2: 0,
            arg0: 0,
            arg1: 0,
            seq: 0,
        }
    }

    /// Constructeur ergonomique pour les sites de push.
    #[inline]
    pub fn new(pid: u32, event_type: u8, severity: u8, opcode: u32, arg0: u64, arg1: u64) -> Self {
        Self {
            pid,
            event_type,
            severity,
            _pad: [0; 2],
            opcode,
            _pad2: 0,
            arg0,
            arg1,
            seq: 0,
        }
    }
}

/// Taille en octets d'un `ShieldEvent` — exposée pour l'ABI du syscall.
pub const SHIELD_EVENT_SIZE: usize = core::mem::size_of::<ShieldEvent>();

static RING: Mutex<[ShieldEvent; RING_SIZE]> = Mutex::new([ShieldEvent::zero(); RING_SIZE]);
/// Position d'écriture (monotone, modulo RING_SIZE à l'indexation).
static HEAD: AtomicUsize = AtomicUsize::new(0);
/// Position de lecture/drain (monotone).
static TAIL: AtomicUsize = AtomicUsize::new(0);
/// Compteur de séquence global.
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Événements perdus (ring plein — serveur en retard ou absent).
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Événements poussés au total.
static PUSHED: AtomicU64 = AtomicU64::new(0);

/// Pousse un événement de sécurité vers le shield. **Non-bloquant, best-effort** :
/// si le ring est plein, l'événement le plus récent est DROP (compté). Ne panique
/// jamais (RÈGLE SHIELDFEED-01). Sûr depuis n'importe quel contexte non-NMI.
pub fn push(mut ev: ShieldEvent) {
    let mut ring = RING.lock();
    let head = HEAD.load(Ordering::Relaxed);
    let tail = TAIL.load(Ordering::Acquire);
    if head.wrapping_sub(tail) >= RING_SIZE {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    ev.seq = SEQ.fetch_add(1, Ordering::Relaxed);
    ring[head % RING_SIZE] = ev;
    HEAD.store(head.wrapping_add(1), Ordering::Release);
    PUSHED.fetch_add(1, Ordering::Relaxed);
}

/// Raccourci de push (évite de construire un `ShieldEvent` au site d'appel).
#[inline]
pub fn push_event(pid: u32, event_type: u8, severity: u8, opcode: u32, arg0: u64, arg1: u64) {
    push(ShieldEvent::new(pid, event_type, severity, opcode, arg0, arg1));
}

/// Draine jusqu'à `out.len()` événements dans `out`. Retourne le nombre copié.
/// Appelé par le syscall de drain (serveur exo_shield).
pub fn drain(out: &mut [ShieldEvent]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let ring = RING.lock();
    let head = HEAD.load(Ordering::Acquire);
    let mut tail = TAIL.load(Ordering::Relaxed);
    let mut n = 0;
    while tail != head && n < out.len() {
        out[n] = ring[tail % RING_SIZE];
        tail = tail.wrapping_add(1);
        n += 1;
    }
    TAIL.store(tail, Ordering::Release);
    n
}

/// Snapshot de statistiques du feed.
#[derive(Debug, Clone, Copy)]
pub struct ShieldFeedStats {
    pub pushed: u64,
    pub dropped: u64,
    pub pending: usize,
}

pub fn stats() -> ShieldFeedStats {
    let head = HEAD.load(Ordering::Acquire);
    let tail = TAIL.load(Ordering::Acquire);
    ShieldFeedStats {
        pushed: PUSHED.load(Ordering::Relaxed),
        dropped: DROPPED.load(Ordering::Relaxed),
        pending: head.wrapping_sub(tail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        // Drain tout pour repartir propre (les tests partagent le ring statique).
        let mut sink = [ShieldEvent::zero(); RING_SIZE];
        while drain(&mut sink) == RING_SIZE {}
    }

    #[test]
    fn push_then_drain_preserves_order_and_data() {
        reset();
        push_event(42, event_type::SYSCALL, severity::HIGH, 57, 0xAA, 0xBB);
        push_event(43, event_type::PROCESS, severity::CRITICAL, 59, 0xCC, 0xDD);

        let mut out = [ShieldEvent::zero(); 8];
        let n = drain(&mut out);
        assert!(n >= 2);
        // Les deux derniers de notre paire doivent apparaître dans l'ordre.
        let a = out[n - 2];
        let b = out[n - 1];
        assert_eq!(a.pid, 42);
        assert_eq!(a.event_type, event_type::SYSCALL);
        assert_eq!(a.opcode, 57);
        assert_eq!(b.pid, 43);
        assert_eq!(b.severity, severity::CRITICAL);
        assert!(b.seq > a.seq, "seq monotone");
    }

    #[test]
    fn drain_empty_returns_zero() {
        reset();
        let mut out = [ShieldEvent::zero(); 4];
        assert_eq!(drain(&mut out), 0);
    }

    #[test]
    fn overflow_drops_without_panic() {
        reset();
        // Remplir au-delà de la capacité → les surplus sont DROP, pas de panic.
        for i in 0..(RING_SIZE + 32) {
            push_event(i as u32, event_type::MEMORY, severity::LOW, 0, 0, 0);
        }
        assert!(stats().dropped >= 32, "les surplus doivent être comptés");
        // Le ring contient au plus RING_SIZE événements.
        let mut out = [ShieldEvent::zero(); RING_SIZE];
        let n = drain(&mut out);
        assert!(n <= RING_SIZE);
    }
}
