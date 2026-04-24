// kernel/src/ipc/core/sequence.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// NUMÉROS DE SÉQUENCE — Ordering garantis pour IPC
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module garantit l'ordre causal des messages dans un canal IPC.
// Chaque canal maintient un compteur de séquence (u64) côté émetteur.
// Le récepteur valide l'ordre et détecte les pertes/doublons via une
// fenêtre glissante (sliding window bitmask).
//
// PERFORMANCE : toutes les opérations sont O(1), lock-free côté émetteur.
// CONTRAINTE : zéro allocation heap (conforme Zone NO-ALLOC).
// ═══════════════════════════════════════════════════════════════════════════════

use super::constants::SEQ_WINDOW_SIZE;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SeqSender — côté émetteur (monotone croissant)
// ─────────────────────────────────────────────────────────────────────────────

/// Générateur de numéros de séquence — côté émetteur.
/// Un SeqSender est propre à un canal/direction.
#[repr(C, align(8))]
pub struct SeqSender {
    /// Numéro de séquence du prochain message à envoyer.
    next_seq: AtomicU64,
}

impl SeqSender {
    /// Crée un nouveau SeqSender commençant à la séquence 1.
    /// La séquence 0 est réservée comme "invalide".
    pub const fn new() -> Self {
        Self {
            next_seq: AtomicU64::new(1),
        }
    }

    /// Alloue le prochain numéro de séquence.
    /// Retourne la valeur et l'incrémente de manière atomique.
    /// Thread-safe pour plusieurs producteurs sur le même canal.
    #[inline(always)]
    pub fn next(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Consulte le numéro courant sans l'incrémenter.
    #[inline(always)]
    pub fn current(&self) -> u64 {
        self.next_seq.load(Ordering::Relaxed)
    }
}

impl Default for SeqSender {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqReceiver — côté récepteur (validation fenêtre glissante)
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la validation d'un numéro de séquence.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SeqCheck {
    /// Message dans l'ordre attendu : à traiter immédiatement.
    InOrder,
    /// Message en avance sur la fenêtre : à mettre en tampon.
    Future { gap: u64 },
    /// Message déjà reçu (doublon à rejeter).
    Duplicate,
    /// Message trop ancien, hors fenêtre (à rejeter).
    TooOld,
}

/// Validateur côté récepteur.
/// Maintient l'état nécessaire à la détection d'ordre et doublons.
///
/// CONTRAINTE : utilise uniquement de l'espace statique (pas de Vec).
/// La fenêtre est un bitmask de SEQ_WINDOW_BITS bits (u128 max 128 bits).
pub struct SeqReceiver {
    /// Prochain numéro de séquence attendu en ordre.
    expected: AtomicU64,
    /// Bitmask de présence pour la fenêtre [expected .. expected + WINDOW).
    /// Bit i = 1 si le message expected+i a déjà été reçu.
    window: AtomicU64,
    /// Compteur de messages hors-ordre détectés.
    out_of_order_count: AtomicU64,
    /// Compteur de messages dupliqués rejetés.
    duplicate_count: AtomicU64,
}

impl SeqReceiver {
    /// Crée un récepteur s'attendant à recevoir la séquence 1 en premier.
    pub const fn new() -> Self {
        Self {
            expected: AtomicU64::new(1),
            window: AtomicU64::new(0),
            out_of_order_count: AtomicU64::new(0),
            duplicate_count: AtomicU64::new(0),
        }
    }

    /// Valide et enregistre la réception d'un numéro de séquence.
    ///
    /// # Retour
    /// - `InOrder`     : message dans l'ordre, traiter maintenant.
    /// - `Future {gap}`: message en avance de `gap` positions, à tamponner.
    /// - `Duplicate`   : déjà reçu, ignorer.
    /// - `TooOld`      : hors fenêtre, ignorer.
    ///
    /// # Note concurrence
    /// Cette fonction n'est pas atomique sur l'ensemble de l'opération.
    /// Elle est conçue pour un usage mono-consommateur (SPSC ou protégé par lock).
    pub fn check_and_advance(&self, seq: u64) -> SeqCheck {
        let expected = self.expected.load(Ordering::Acquire);

        if seq == expected {
            // Cas rapide : message dans l'ordre.
            // Avancer la fenêtre en consommant les bits consécutifs déjà reçus.
            let mut new_expected = expected + 1;
            let mut win = self.window.load(Ordering::Relaxed);

            // Avancer tant que le bit suivant est déjà présent.
            while (win & 1) != 0 {
                win >>= 1;
                new_expected += 1;
            }

            self.window.store(win, Ordering::Relaxed);
            self.expected.store(new_expected, Ordering::Release);
            SeqCheck::InOrder
        } else if seq > expected {
            let gap = seq - expected;
            if gap >= SEQ_WINDOW_SIZE {
                // Trop loin dans le futur — on ne peut pas bufferiser.
                SeqCheck::Future { gap }
            } else {
                // Dans la fenêtre — marquer le bit.
                let bit = 1u64 << (gap - 1);
                let win = self.window.fetch_or(bit, Ordering::Relaxed);
                if win & bit != 0 {
                    // Bit déjà présent → doublon.
                    self.duplicate_count.fetch_add(1, Ordering::Relaxed);
                    SeqCheck::Duplicate
                } else {
                    self.out_of_order_count.fetch_add(1, Ordering::Relaxed);
                    SeqCheck::Future { gap }
                }
            }
        } else {
            // seq < expected — ancien message.
            let behind = expected - seq;
            if behind < SEQ_WINDOW_SIZE {
                // Dans la fenêtre passée — doublon.
                self.duplicate_count.fetch_add(1, Ordering::Relaxed);
                SeqCheck::Duplicate
            } else {
                SeqCheck::TooOld
            }
        }
    }

    /// Retourne le prochain numéro attendu.
    #[inline(always)]
    pub fn expected(&self) -> u64 {
        self.expected.load(Ordering::Relaxed)
    }

    /// Statistiques : messages hors-ordre.
    #[inline(always)]
    pub fn out_of_order_count(&self) -> u64 {
        self.out_of_order_count.load(Ordering::Relaxed)
    }

    /// Statistiques : doublons détectés.
    #[inline(always)]
    pub fn duplicate_count(&self) -> u64 {
        self.duplicate_count.load(Ordering::Relaxed)
    }

    /// Réinitialise le récepteur à la séquence donnée.
    /// À utiliser lors de la reconnexion ou du reset d'un canal.
    pub fn reset(&self, start_seq: u64) {
        self.expected.store(start_seq, Ordering::Release);
        self.window.store(0, Ordering::Relaxed);
        self.out_of_order_count.store(0, Ordering::Relaxed);
        self.duplicate_count.store(0, Ordering::Relaxed);
    }
}

impl Default for SeqReceiver {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqPair — paire émetteur/récepteur pour un canal unidirectionnel
// ─────────────────────────────────────────────────────────────────────────────

/// Un canal unidirectionnel complet pour la gestion de séquence.
pub struct SeqPair {
    pub sender: SeqSender,
    pub receiver: SeqReceiver,
}

impl SeqPair {
    pub const fn new() -> Self {
        Self {
            sender: SeqSender::new(),
            receiver: SeqReceiver::new(),
        }
    }
}

impl Default for SeqPair {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests internes
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inorder_sequence() {
        let recv = SeqReceiver::new();
        assert_eq!(recv.check_and_advance(1), SeqCheck::InOrder);
        assert_eq!(recv.check_and_advance(2), SeqCheck::InOrder);
        assert_eq!(recv.check_and_advance(3), SeqCheck::InOrder);
        assert_eq!(recv.expected(), 4);
    }

    #[test]
    fn test_out_of_order_then_gap_fill() {
        let recv = SeqReceiver::new();
        // Arrive séq 2 avant séq 1.
        assert!(matches!(
            recv.check_and_advance(2),
            SeqCheck::Future { gap: 1 }
        ));
        // Maintenant séq 1 arrive → doit déclencher InOrder et avancer.
        assert_eq!(recv.check_and_advance(1), SeqCheck::InOrder);
        // expected doit avoir sauté à 3 (2 était déjà dans la fenêtre).
        assert_eq!(recv.expected(), 3);
    }

    #[test]
    fn test_duplicate_detection() {
        let recv = SeqReceiver::new();
        assert_eq!(recv.check_and_advance(1), SeqCheck::InOrder);
        // Renvoyer le même numéro.
        assert_eq!(recv.check_and_advance(1), SeqCheck::TooOld);
        assert_eq!(recv.duplicate_count(), 0); // TooOld, pas Duplicate
    }
}
