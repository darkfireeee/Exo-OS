// libs/exo-types/src/pollfd.rs
//
// Fichier : libs/exo_types/src/pollfd.rs
// Rôle    : PollFd — ABI Linux pour poll(2) — GI-01 Étape 7.
//
// SOURCE DE VÉRITÉ : ExoFS_Translation_Layer_v5_FINAL.md §1.1

/// Descripteur pour `poll(2)` — ABI Linux exacte (8 bytes).
///
/// `events` et `revents` utilisent les constantes POLL* standard.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PollFd {
    /// Descripteur de fichier à surveiller.
    pub fd: u32,
    /// Événements demandés (IN, OUT, ERR...).
    pub events: u16,
    /// Événements retournés par le kernel (renseigné en retour de poll).
    pub revents: u16,
}

const _: () = assert!(core::mem::size_of::<PollFd>() == 8);

// ─── Constantes événements poll standard ─────────────────────────────────────
impl PollFd {
    /// Donnée disponible en lecture.
    pub const POLLIN: u16 = 0x0001;
    /// Donnée urgente disponible (OOB).
    pub const POLLPRI: u16 = 0x0002;
    /// Prêt à l'écriture.
    pub const POLLOUT: u16 = 0x0004;
    /// Condition d'erreur.
    pub const POLLERR: u16 = 0x0008;
    /// Connexion fermée.
    pub const POLLHUP: u16 = 0x0010;
    /// Descripteur invalide.
    pub const POLLNVAL: u16 = 0x0020;
    /// Pair a fermé le socket (lecture seule).
    pub const POLLRDHUP: u16 = 0x2000;
}
