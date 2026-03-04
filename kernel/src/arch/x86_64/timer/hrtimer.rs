// kernel/src/scheduler/timer/hrtimer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// HRTimer — minuteries haute résolution (ns) à base de liste triée
// ═══════════════════════════════════════════════════════════════════════════════
//
// Supporte jusqu'à MAX_HRTIMERS minuteries simultanées par CPU.
// La liste est triée par expiration croissante (pas d'arbre rouge-noir en
// no_alloc — tableau fixe trié par insertion).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

#[inline]
fn monotonic_ns() -> u64 {
    // Lecture du TSC comme base de temps monotone.
    unsafe { crate::arch::x86_64::cpu::tsc::read_tsc() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

const MAX_HRTIMERS: usize = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Structure d'une minuterie
// ─────────────────────────────────────────────────────────────────────────────

type HrTimerCallback = unsafe fn(u32, u64);

#[derive(Clone, Copy)]
struct HrTimerEntry {
    expiry_ns: u64,
    id:        u32,
    data:      u64,
    callback:  Option<HrTimerCallback>,
}

impl HrTimerEntry {
    const fn empty() -> Self {
        Self { expiry_ns: u64::MAX, id: 0, data: 0, callback: None }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Liste de minuteries par CPU
// ─────────────────────────────────────────────────────────────────────────────

struct HrTimerList {
    entries: [HrTimerEntry; MAX_HRTIMERS],
    count:   usize,
    next_id: u32,
}

impl HrTimerList {
    const fn new() -> Self {
        Self {
            entries: [HrTimerEntry::empty(); MAX_HRTIMERS],
            count:   0,
            next_id: 1,
        }
    }

    /// Insère une minuterie triée par expiration. Retourne son ID ou 0 si plein.
    fn insert(&mut self, expiry_ns: u64, data: u64, cb: HrTimerCallback) -> u32 {
        if self.count >= MAX_HRTIMERS { return 0; }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);

        let entry = HrTimerEntry { expiry_ns, id, data, callback: Some(cb) };
        // Insertion triée par expiry_ns.
        let mut pos = self.count;
        for i in 0..self.count {
            if self.entries[i].expiry_ns > expiry_ns {
                pos = i;
                break;
            }
        }
        // Décaler vers la droite.
        let mut j = self.count;
        while j > pos {
            self.entries[j] = self.entries[j - 1];
            j -= 1;
        }
        self.entries[pos] = entry;
        self.count += 1;
        id
    }

    /// Annule la minuterie par ID.
    fn cancel(&mut self, id: u32) -> bool {
        for i in 0..self.count {
            if self.entries[i].id == id {
                let mut j = i;
                while j + 1 < self.count {
                    self.entries[j] = self.entries[j + 1];
                    j += 1;
                }
                self.entries[self.count - 1] = HrTimerEntry::empty();
                self.count -= 1;
                return true;
            }
        }
        false
    }

    /// Déclenche toutes les minuteries expirées. Retourne leur nombre.
    unsafe fn fire_expired(&mut self) -> usize {
        let now = monotonic_ns();
        let mut fired = 0usize;
        while self.count > 0 && self.entries[0].expiry_ns <= now {
            let e = self.entries[0];
            // Retirer en premier pour éviter la réentrance.
            let mut j = 0;
            while j + 1 < self.count {
                self.entries[j] = self.entries[j + 1];
                j += 1;
            }
            self.entries[self.count - 1] = HrTimerEntry::empty();
            self.count -= 1;
            if let Some(cb) = e.callback {
                cb(e.id, e.data);
            }
            fired += 1;
        }
        fired
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Instances globales (une par CPU — indexées par CPU ID)
// ─────────────────────────────────────────────────────────────────────────────

use crate::scheduler::smp::topology::MAX_CPUS;
use core::mem::MaybeUninit;

static mut HR_LISTS: [MaybeUninit<HrTimerList>; MAX_CPUS] =
    [const { MaybeUninit::uninit() }; MAX_CPUS];

pub static HRTIMER_FIRED: AtomicU64 = AtomicU64::new(0);
pub static HRTIMER_CANCELLED: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise les listes hrtimer pour `nr_cpus` CPUs.
///
/// # Safety
/// Appelé une seule fois depuis `scheduler::init()`.
pub unsafe fn init(nr_cpus: usize) {
    for cpu in 0..nr_cpus.min(MAX_CPUS) {
        HR_LISTS[cpu].write(HrTimerList::new());
    }
}

/// Arme une minuterie sur le CPU `cpu`, déclenchée `delay_ns` après maintenant.
///
/// # Safety
/// Préemption désactivée requise ; le CPU doit être initialisé.
pub unsafe fn arm(cpu: usize, delay_ns: u64, data: u64, cb: HrTimerCallback) -> u32 {
    let expiry = monotonic_ns().saturating_add(delay_ns);
    HR_LISTS[cpu].assume_init_mut().insert(expiry, data, cb)
}

/// Annule la minuterie `id` sur le CPU `cpu`.
///
/// # Safety
/// Préemption désactivée requise.
pub unsafe fn cancel(cpu: usize, id: u32) -> bool {
    let r = HR_LISTS[cpu].assume_init_mut().cancel(id);
    if r { HRTIMER_CANCELLED.fetch_add(1, Ordering::Relaxed); }
    r
}

/// Déclenche les minuteries expirées sur le CPU `cpu`.
/// Appelé depuis le tick handler après chaque tick.
///
/// # Safety
/// Préemption désactivée requise.
pub unsafe fn fire_expired(cpu: usize) -> usize {
    let fired = HR_LISTS[cpu].assume_init_mut().fire_expired();
    HRTIMER_FIRED.fetch_add(fired as u64, Ordering::Relaxed);
    fired
}
