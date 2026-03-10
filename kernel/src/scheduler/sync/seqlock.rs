// kernel/src/scheduler/sync/seqlock.rs
//
// ═════════════════════════════════════════════════════════════════════════════
// SeqLock — Verrou séquentiel ISR-safe (Exo-OS · Couche 1)
// ═════════════════════════════════════════════════════════════════════════════
//
// ## Principe
//   Un SeqLock permet à des lecteurs multiples de lire des données sans jamais
//   se bloquer, au prix d'un retry si un écrivain est actif au même moment.
//
//   Invariant de compteur :
//     - Pair   = état stable (aucun write en cours)
//     - Impair = write en cours (lecteurs doivent retry)
//
//   Lecteur :
//     1. Lire seq1 via Acquire → si impair, spin_loop et retry
//     2. Copier les données
//     3. Lire seq2 via Acquire → si seq1 != seq2, les données ont bougé → retry
//     4. Données cohérentes : retourner
//
//   Écrivain :
//     1. seq.fetch_add(1, Release) → seq devient impair (write start)
//     2. Mettre à jour les données
//     3. seq.fetch_add(1, Release) → seq redevient pair (write end)
//
// ## Règles (ARCH-TIME-03)
//   RÈGLE SEQLOCK-01 : Jamais de Mutex dans ktime_get_ns() ni dans une ISR.
//     → ktime_get_ns() utilise ce pattern directement (cf. ktime.rs).
//   RÈGLE SEQLOCK-02 : Le write ne doit JAMAIS appeler les données qu'il
//     protège (dépendance circulaire).
//   RÈGLE SEQLOCK-03 : Les données protégées par SeqLock<T> DOIVENT être
//     Copy — un SeqLock sur une chaîne allouée heap est un antipattern.
//   RÈGLE SEQLOCK-04 : L'écrivain ne doit PAS être préempté entre
//     write_begin() et write_end() — utiliser PreemptGuard.
//
// ## Complexité
//   Lecture  : O(1) amortissant — O(N) en contention haute (rare)
//   Écriture : O(1) — 2 fetch_add atomiques + stores Relaxed
//   Mémoire  : sizeof(T) + 8 octets (le compteur seq)
//
// ## Cas d'usage
//   - ktime::KtimeState  — horloge monotone + offset per-CPU (ktime.rs)
//   - ktime::WallState   — offset epoch UNIX (ktime.rs)
//   - Toute donnée petite (≤ 2 cache lines) lue fréquemment depuis ISR
//   - À NE PAS utiliser pour des buffers larges (overhead copie côté lecteur)
// ═════════════════════════════════════════════════════════════════════════════


use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SeqLock<T>
// ─────────────────────────────────────────────────────────────────────────────

/// SeqLock générique pour des données `Copy`.
///
/// Garantit des lectures cohérentes sans jamais bloquer le lecteur, y compris
/// depuis une ISR (pas de Mutex, pas de spin continu).
///
/// ## Contrainte de taille
/// T doit tenir dans une ou deux cache lines (≤ 128 octets) pour que le
/// pattern lecteur soit efficace. Au-delà, preferez un RwLock.
#[repr(C)]
pub struct SeqLock<T: Copy> {
    /// Compteur séquentiel : pair = stable, impair = write en cours.
    seq:  AtomicU64,
    /// Données protégées.
    data: UnsafeCell<T>,
}

// SAFETY: SeqLock est Send+Sync si T l'est — les accès concurrents sont sûrs
// grâce au protocole seqlock (les lectures copient via retry).
unsafe impl<T: Copy + Send> Send for SeqLock<T> {}
unsafe impl<T: Copy + Send + Sync> Sync for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    /// Crée un `SeqLock` avec la valeur initiale donnée.
    /// Le compteur démarre à 0 (pair = état stable).
    pub const fn new(value: T) -> Self {
        Self {
            seq:  AtomicU64::new(0),
            data: UnsafeCell::new(value),
        }
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Lit les données de manière cohérente avec retry seqlock.
    ///
    /// ISR-safe : jamais de lock, pas d'alloc, retry si write en cours.
    ///
    /// ## Nombre de retries
    /// En pratique < 5 retries sauf contention extrême (écrivain très fréquent).
    /// Si T est volumineux, chaque retry copie T → préférer read_with() pour
    /// minimiser la taille de la copie.
    #[inline(always)]
    pub fn read(&self) -> T {
        loop {
            // 1. Seq AVANT — doit être pair.
            let seq1 = self.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                // Write en cours → spin et retry.
                core::hint::spin_loop();
                continue;
            }

            // 2. Copier les données (T est Copy → copie sur la pile).
            //    SAFETY: la lecture est cohérente si seq ne change pas.
            //    Si un write arrive entre seq1 et seq2, on détecte via seq2 ≠ seq1.
            let value = unsafe { *self.data.get() };

            // 3. Vérifier que seq n'a pas changé.
            let seq2 = self.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return value;
            }

            // Write s'est intercalé → retry.
            core::hint::spin_loop();
        }
    }

    /// Lit les données et les passe à une fonction de projection.
    ///
    /// Évite une copie complète de T si seul un champ est nécessaire.
    /// Le closure F est appelé avec une référence TEMPORAIRE à T — ne pas
    /// conserver de référence au-delà de l'appel.
    ///
    /// ## Safety
    /// F doit être non-bloquante et sans effet de bord observable
    /// (peut être appelée plusieurs fois en cas de retry).
    #[inline(always)]
    pub fn read_with<R, F: Fn(&T) -> R>(&self, f: F) -> R {
        loop {
            let seq1 = self.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: F lit via référence immutable dans la fenêtre seqlock.
            let result = unsafe { f(&*self.data.get()) };
            let seq2 = self.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return result;
            }
            core::hint::spin_loop();
        }
    }

    // ── Écriture ──────────────────────────────────────────────────────────────

    /// Écrit de nouvelles données de manière atomique vis-à-vis des lecteurs.
    ///
    /// RÈGLE SEQLOCK-04 : caller doit avoir désactivé la préemption
    /// (via `PreemptGuard`) si un ordonnanceur est actif.
    ///
    /// ## Safety
    /// - Thread d'écriture exclusif — un seul écrivain à la fois.
    ///   (SeqLock *sans* lock ne détecte pas les writes concurrents —
    ///    si plusieurs écrivains possibles, utiliser `write_guarded()`)
    /// - Pas de `read()` ni `read_with()` dans le corps de l'écrivain.
    #[inline]
    pub unsafe fn write(&self, value: T) {
        // seq impair = write start.
        self.seq.fetch_add(1, Ordering::AcqRel);
        // Barrière : les stores suivants sont visibles APRÈS le seq impair.
        core::sync::atomic::fence(Ordering::Release);

        // Mettre à jour les données.
        // SAFETY: exclusivité garantie par le contrat d'appel (un seul écrivain).
        *self.data.get() = value;

        // Barrière : les stores data sont visibles AVANT le seq pair.
        core::sync::atomic::fence(Ordering::Release);
        // seq pair = write end.
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// Modifie les données via un closure de mise à jour.
    ///
    /// F reçoit une référence mutable à T et peut la modifier en place,
    /// sans construire d'abord la valeur entière (utile si T est large).
    ///
    /// ## Safety
    /// Mêmes contraintes que `write()` — un seul écrivain à la fois.
    /// F ne doit pas appeler `read()` ou `read_with()` sur ce même SeqLock.
    #[inline]
    pub unsafe fn update<F: FnOnce(&mut T)>(&self, f: F) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        core::sync::atomic::fence(Ordering::Release);
        // SAFETY: exclusivité garantie par contrat d'appel.
        f(&mut *self.data.get());
        core::sync::atomic::fence(Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
    }

    // ── Accès non-protégés (init seulement) ───────────────────────────────────

    /// Initialise les données SANS signaler de write (pour la phase d'init).
    ///
    /// À n'appeler qu'UNE SEULE FOIS avant que d'autres threads puissent lire
    /// (typiquement depuis BSP avant SMP init). Le seq reste à 0 → pair → stable.
    ///
    /// ## Safety
    /// - Doit être appelé avant tout appel concurrent à `read()`.
    /// - Doit correspondre à une valeur initiale cohérente (pas d'undefined behavior).
    #[inline]
    pub unsafe fn init_once(&self, value: T) {
        // Stocker directement — pas de signal write car personne ne lit encore.
        *self.data.get() = value;
        // Seq pair (= 0) → stable. Fence pour rendre visible avant les reads.
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Retourne le numéro de séquence courant (diagnostics uniquement).
    #[inline(always)]
    pub fn seq_number(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Retourne `true` si un write est actuellement en cours (seq impair).
    #[inline(always)]
    pub fn write_in_progress(&self) -> bool {
        self.seq.load(Ordering::Relaxed) & 1 != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqGuard — Guard RAII pour l'écriture (garantit le write_end par Drop)
// ─────────────────────────────────────────────────────────────────────────────

/// Guard RAII pour une section d'écriture SeqLock.
///
/// Émet `seq.fetch_add(1)` à la création (write start) et au drop (write end).
/// Garantit que le compteur repasse toujours à pair même en cas de panique.
///
/// ## Exemple
/// ```rust,ignore
/// let _guard = SeqWriteGuard::new(&my_seqlock.seq);
/// // Écrire dans my_seqlock.data ici...
/// // _guard dropped → write_end automatique
/// ```
pub struct SeqWriteGuard<'a> {
    seq: &'a AtomicU64,
}

impl<'a> SeqWriteGuard<'a> {
    /// Commence une section d'écriture (seq devient impair).
    ///
    /// ## Safety
    /// - Un seul `SeqWriteGuard` actif à la fois sur le même SeqLock.
    /// - Ne pas appeler `read()` sur le même SeqLock pendant ce guard.
    #[inline]
    pub unsafe fn new(seq: &'a AtomicU64) -> Self {
        seq.fetch_add(1, Ordering::AcqRel);
        core::sync::atomic::fence(Ordering::Release);
        Self { seq }
    }
}

impl<'a> Drop for SeqWriteGuard<'a> {
    #[inline]
    fn drop(&mut self) {
        // Barrière : rendre les stores data visibles AVANT de repasser à pair.
        core::sync::atomic::fence(Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqLockU64 — Spécialisation optimisée pour un unique u64
// ─────────────────────────────────────────────────────────────────────────────

/// SeqLock optimisé pour un unique `u64` — pas de copie, lecture atomique.
///
/// Sur x86_64, la lecture d'un u64 aligné est naturellement atomique (64-bit
/// load). Le seqlock garantit simplement la cohérence si la valeur est mise à
/// jour depuis plusieurs cœurs en même temps que le contenu d'un multi-field.
///
/// Utilisé par `ktime.rs` pour les champs individuels (tsc_hz, ns_base…).
pub struct SeqLockU64 {
    seq:   AtomicU64,
    value: AtomicU64,
}

impl SeqLockU64 {
    pub const fn new(v: u64) -> Self {
        Self { seq: AtomicU64::new(0), value: AtomicU64::new(v) }
    }

    /// Lecture cohérente — ISR-safe, wait-free.
    #[inline(always)]
    pub fn read(&self) -> u64 {
        loop {
            let seq1 = self.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 { core::hint::spin_loop(); continue; }
            let v    = self.value.load(Ordering::Acquire);
            let seq2 = self.seq.load(Ordering::Acquire);
            if seq1 == seq2 { return v; }
            core::hint::spin_loop();
        }
    }

    /// Écriture atomique.
    ///
    /// ## Safety
    /// Un seul écrivain à la fois.
    #[inline]
    pub unsafe fn write(&self, v: u64) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        core::sync::atomic::fence(Ordering::Release);
        self.value.store(v, Ordering::Relaxed);
        core::sync::atomic::fence(Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires (cfg(test) — no_std compatible via core::)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct ClockSnapshot { ns: u64, hz: u64 }

    #[test]
    fn test_seqlock_read_write() {
        let lock = SeqLock::new(ClockSnapshot { ns: 0, hz: 3_000_000_000 });

        // Vérifier la valeur initiale.
        let snap = lock.read();
        assert_eq!(snap.ns, 0);
        assert_eq!(snap.hz, 3_000_000_000);

        // Write.
        unsafe { lock.write(ClockSnapshot { ns: 1_000_000, hz: 3_200_000_000 }); }

        let snap2 = lock.read();
        assert_eq!(snap2.ns, 1_000_000);
        assert_eq!(snap2.hz, 3_200_000_000);
    }

    #[test]
    fn test_seqlock_seq_invariant() {
        let lock = SeqLock::new(0u64);
        // Après init : seq doit être pair.
        assert_eq!(lock.seq_number() & 1, 0);
        // Pas de write en cours.
        assert!(!lock.write_in_progress());
    }

    #[test]
    fn test_seqlock_u64() {
        let lock = SeqLockU64::new(42);
        assert_eq!(lock.read(), 42);
        unsafe { lock.write(99); }
        assert_eq!(lock.read(), 99);
    }
}
