// kernel/src/memory/utils/oom_killer.rs
//
// OOM Killer — sélection et signalement de la victime lors de manque mémoire.
//
// Architecture :
//   • L'OOM killer est déclenché par `swap/policy.rs` (is_critical) ou par
//     l'allocateur buddy quand toutes les zones sont épuisées.
//   • Il sélectionne la victime via un `OomScorer` trait (inversion dep).
//   • Il signale la mort via un fn pointer (OOM_KILL_SENDER) enregistré par
//     process/ — couche 0 ne peut pas appeler process/ directement.
//
// COUCHE 0 — pas de dépendance scheduler/process/ipc/fs.


use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Trait OomScorer — fourni par process/
// ─────────────────────────────────────────────────────────────────────────────

/// Informations sur un candidat à l'élimination OOM.
#[derive(Debug, Clone, Copy)]
pub struct OomKillCandidate {
    /// Process ID.
    pub pid:      u64,
    /// Score OOM calculé (plus grand = plus prioritaire à tuer).
    pub oom_score: u64,
    /// Résistant RSS en pages.
    pub vm_rss:   u64,
    /// Nom du processus (pour le log).
    pub name:     [u8; 16],
}

impl OomKillCandidate {
    pub const fn invalid() -> Self {
        Self { pid: 0, oom_score: 0, vm_rss: 0, name: [0; 16] }
    }

    pub fn is_valid(&self) -> bool { self.pid != 0 }
}

/// Trait que process/ implémente pour fournir la liste des candidats OOM.
pub trait OomScorer {
    /// Retourne le candidat le plus approprié à tuer.
    fn pick_victim(&self) -> Option<OomKillCandidate>;
    /// Retourne un slice de tous les candidats triés par score décroissant.
    fn candidates(&self, buf: &mut [OomKillCandidate]) -> usize;
}

// ─────────────────────────────────────────────────────────────────────────────
// Scoreur par défaut (RSS-based) avant que process/ soit initialisé
// ─────────────────────────────────────────────────────────────────────────────

pub struct DefaultOomScorer;

impl OomScorer for DefaultOomScorer {
    fn pick_victim(&self) -> Option<OomKillCandidate> {
        // Avant que process/ soit disponible : pas de victime connue.
        None
    }
    fn candidates(&self, _buf: &mut [OomKillCandidate]) -> usize { 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fn pointer de signalement → process/
// ─────────────────────────────────────────────────────────────────────────────

/// Prototype de la fonction de signalement OOM enregistrée par process/.
/// `pid` : PID à tuer.
/// Returns `true` si le signal a pu être envoyé.
pub type OomKillSendFn = fn(pid: u64) -> bool;

#[allow(dead_code)]
fn nop_oom_kill(_pid: u64) -> bool { false }

/// Pointeur fonction vers le handler OOM de process/ — write-once.
static OOM_KILL_SENDER: AtomicUsize = AtomicUsize::new(0);

/// Enregistre le handler OOM de process/.
/// Doit être appelé une seule fois par process/ lors de son init.
pub fn register_oom_kill_sender(f: OomKillSendFn) {
    OOM_KILL_SENDER.compare_exchange(
        0,
        f as usize,
        Ordering::Release,
        Ordering::Relaxed,
    ).ok();
}

/// Appelle le sender OOM enregistré.
fn invoke_kill_sender(pid: u64) -> bool {
    let ptr = OOM_KILL_SENDER.load(Ordering::Acquire);
    if ptr == 0 {
        return false;
    }
    let f: OomKillSendFn = unsafe { core::mem::transmute(ptr) };
    f(pid)
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques OOM
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct OomStats {
    pub oom_invocations:  AtomicU64,
    pub victims_selected: AtomicU64,
    pub kills_sent:       AtomicU64,
    pub kills_failed:     AtomicU64,
    /// Déclenchements en situation critique (zone vide).
    pub critical_events:  AtomicU64,
    /// Ignorer OOM : flag levé quand kernel est en shutdown.
    pub suppressed:       AtomicBool,
}

impl OomStats {
    const fn new() -> Self {
        Self {
            oom_invocations:  AtomicU64::new(0),
            victims_selected: AtomicU64::new(0),
            kills_sent:       AtomicU64::new(0),
            kills_failed:     AtomicU64::new(0),
            critical_events:  AtomicU64::new(0),
            suppressed:       AtomicBool::new(false),
        }
    }
}

unsafe impl Sync for OomStats {}
pub static OOM_STATS: OomStats = OomStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Cooldown — éviter les kills en rafale
// ─────────────────────────────────────────────────────────────────────────────

/// TSC du dernier kill.  Empêche un second kill dans les 100 ms suivants.
static LAST_KILL_TSC: AtomicU64 = AtomicU64::new(0);
/// Fréquence TSC en Hz (estimée à l'init, 3 GHz par défaut).
static TSC_HZ: AtomicU64 = AtomicU64::new(3_000_000_000);

/// Configure la fréquence TSC (appelé par arch/ après calibration).
pub fn set_tsc_hz(hz: u64) {
    TSC_HZ.store(hz, Ordering::Relaxed);
}

#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdtsc disponible sur x86_64; non-sérialisé suffisant pour timestamp OOM.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Retourne `true` si assez de temps s'est écoulé depuis le dernier kill OOM.
fn cooldown_elapsed() -> bool {
    let now = rdtsc();
    let last = LAST_KILL_TSC.load(Ordering::Relaxed);
    if last == 0 { return true; }
    let elapsed_ticks = now.wrapping_sub(last);
    let hz = TSC_HZ.load(Ordering::Relaxed);
    // Cooldown 100 ms.
    elapsed_ticks >= hz / 10
}

// ─────────────────────────────────────────────────────────────────────────────
// Sélection de victime
// ─────────────────────────────────────────────────────────────────────────────

/// Buffer statique pour les candidats OOM (évite toute allocation sur le
/// chemin d'urgence).
static OOM_CANDIDATE_BUF: Mutex<[OomKillCandidate; 64]> =
    Mutex::new([OomKillCandidate { pid: 0, oom_score: 0, vm_rss: 0, name: [0; 16] }; 64]);

/// Sélectionne la victime OOM via le scorer fourni.
///
/// Retourne `Some(pid)` ou `None` (aucun candidat ou cooldown actif).
pub fn select_oom_victim<S: OomScorer>(scorer: &S) -> Option<u64> {
    if OOM_STATS.suppressed.load(Ordering::Acquire) {
        return None;
    }
    if !cooldown_elapsed() {
        return None;
    }

    OOM_STATS.oom_invocations.fetch_add(1, Ordering::Relaxed);

    // Essayer `pick_victim` d'abord (O(1) si scorer le supporte).
    if let Some(victim) = scorer.pick_victim() {
        if victim.is_valid() {
            OOM_STATS.victims_selected.fetch_add(1, Ordering::Relaxed);
            return Some(victim.pid);
        }
    }

    // Fallback : obtenir la liste et prendre le premier.
    let mut buf = OOM_CANDIDATE_BUF.lock();
    let n = scorer.candidates(&mut *buf);
    if n == 0 {
        return None;
    }
    // Trier par score décroissant (simple, n ≤ 64).
    let slice = &mut buf[..n];
    slice.sort_unstable_by(|a, b| b.oom_score.cmp(&a.oom_score));
    let pid = slice[0].pid;
    OOM_STATS.victims_selected.fetch_add(1, Ordering::Relaxed);
    Some(pid)
}

// ─────────────────────────────────────────────────────────────────────────────
// Point d'entrée OOM
// ─────────────────────────────────────────────────────────────────────────────

/// Déclenche l'OOM killer avec `scorer` comme source de candidats.
///
/// Flow :
///   1. select_oom_victim(scorer)
///   2. invoke_kill_sender(pid)
///   3. Mettre à jour LAST_KILL_TSC
///
/// Returns `true` si un kill a été envoyé.
pub fn oom_kill<S: OomScorer>(scorer: &S, critical: bool) -> bool {
    if critical {
        OOM_STATS.critical_events.fetch_add(1, Ordering::Relaxed);
    }

    let Some(pid) = select_oom_victim(scorer) else {
        return false;
    };

    let sent = invoke_kill_sender(pid);
    if sent {
        LAST_KILL_TSC.store(rdtsc(), Ordering::Relaxed);
        OOM_STATS.kills_sent.fetch_add(1, Ordering::Relaxed);
    } else {
        OOM_STATS.kills_failed.fetch_add(1, Ordering::Relaxed);
    }
    sent
}

/// Supprime le déclenchement OOM (ex. shutdown, recover).
pub fn oom_suppress() {
    OOM_STATS.suppressed.store(true, Ordering::Release);
}

/// Réactive l'OOM killer.
pub fn oom_unsuppress() {
    OOM_STATS.suppressed.store(false, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// OOM killer global avec scorer par défaut
// ─────────────────────────────────────────────────────────────────────────────

/// Déclenche l'OOM killer avec le scorer par défaut (avant que process/ soit
/// initialisé).
pub fn oom_kill_default() -> bool {
    let scorer = DefaultOomScorer;
    oom_kill(&scorer, false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // Rien à initialiser ; structures déclarées const.
}
