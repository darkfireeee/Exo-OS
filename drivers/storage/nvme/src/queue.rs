//! Gestion des anneaux de file NVMe (indices tail/head + phase tag).
//!
//! Logique **pure** (pas de MMIO) → testable. Le wraparound d'index et le
//! basculement du phase tag sont une source classique de bugs (lecture d'une
//! complétion périmée, doorbell hors borne). On les isole et on les teste.

/// État d'une Submission Queue : anneau de `size` entrées.
#[derive(Clone, Copy, Debug)]
pub struct SqRing {
    pub size: u16,
    pub tail: u16,
}

impl SqRing {
    pub const fn new(size: u16) -> Self {
        Self { size, tail: 0 }
    }

    /// Index où écrire la prochaine entrée (= tail courant).
    #[inline]
    pub fn tail(&self) -> u16 {
        self.tail
    }

    /// Y a-t-il de la place ? (le tail ne doit pas rattraper le head côté
    /// contrôleur ; ici on borne au pire à `size-1` entrées en vol).
    #[inline]
    pub fn next_tail(&self) -> u16 {
        if self.tail + 1 >= self.size {
            0
        } else {
            self.tail + 1
        }
    }

    /// Avance le tail après soumission ; retourne la nouvelle valeur de tail à
    /// écrire dans le doorbell.
    #[inline]
    pub fn advance(&mut self) -> u16 {
        self.tail = self.next_tail();
        self.tail
    }
}

/// État d'une Completion Queue : anneau + phase tag attendu.
#[derive(Clone, Copy, Debug)]
pub struct CqRing {
    pub size: u16,
    pub head: u16,
    /// Phase attendue pour une entrée *nouvelle*. Démarre à `true` (1) car la
    /// CQ est initialisée à zéro et le contrôleur écrit P=1 au premier tour.
    pub phase: bool,
}

impl CqRing {
    pub const fn new(size: u16) -> Self {
        Self {
            size,
            head: 0,
            phase: true,
        }
    }

    #[inline]
    pub fn head(&self) -> u16 {
        self.head
    }

    /// Une entrée dont le bit de phase vaut `entry_phase` est-elle une
    /// complétion *neuve* (vs. résidu du tour précédent) ?
    #[inline]
    pub fn entry_is_new(&self, entry_phase: bool) -> bool {
        entry_phase == self.phase
    }

    /// Consomme l'entrée courante : avance le head et bascule la phase au
    /// wraparound. Retourne la nouvelle valeur de head (pour le doorbell).
    #[inline]
    pub fn advance(&mut self) -> u16 {
        if self.head + 1 >= self.size {
            self.head = 0;
            self.phase = !self.phase; // nouveau tour → phase inversée
        } else {
            self.head += 1;
        }
        self.head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sq_tail_wraps_at_size() {
        let mut sq = SqRing::new(4);
        assert_eq!(sq.tail(), 0);
        assert_eq!(sq.advance(), 1);
        assert_eq!(sq.advance(), 2);
        assert_eq!(sq.advance(), 3);
        assert_eq!(sq.advance(), 0, "le tail doit wrapper à size");
    }

    #[test]
    fn cq_phase_flips_on_wrap() {
        let mut cq = CqRing::new(2);
        assert!(cq.phase, "phase initiale = 1");
        // Une entrée neuve a phase==1 au premier tour.
        assert!(cq.entry_is_new(true));
        assert!(!cq.entry_is_new(false));
        cq.advance(); // head 0→1
        assert!(cq.phase);
        cq.advance(); // head 1→0, wrap → phase bascule
        assert_eq!(cq.head, 0);
        assert!(!cq.phase, "après un tour, phase attendue = 0");
        // Maintenant une entrée neuve a phase==0.
        assert!(cq.entry_is_new(false));
        assert!(!cq.entry_is_new(true));
    }

    #[test]
    fn cq_two_full_laps_returns_to_initial_phase() {
        let mut cq = CqRing::new(3);
        for _ in 0..3 {
            cq.advance();
        }
        assert!(!cq.phase);
        for _ in 0..3 {
            cq.advance();
        }
        assert!(cq.phase, "deux tours complets → phase initiale");
        assert_eq!(cq.head, 0);
    }

    #[test]
    fn sq_size_one_always_zero() {
        // Cas dégénéré : taille 1 → tail reste 0.
        let mut sq = SqRing::new(1);
        assert_eq!(sq.advance(), 0);
        assert_eq!(sq.advance(), 0);
    }
}
