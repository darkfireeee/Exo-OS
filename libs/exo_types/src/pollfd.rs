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
    pub fd:      u32,
    /// Événements demandés (IN, OUT, ERR...).
    pub events:  u16,
    /// Événements retournés par le kernel (renseigné en retour de poll).
    pub revents: u16,
}

const _: () = assert!(core::mem::size_of::<PollFd>() == 8);

// ─── Constantes événements poll standard ─────────────────────────────────────
impl PollFd {
    pub const POLLIN:   u16 = 0x0001;
    pub const POLLPRI:  u16 = 0x0002;
    pub const POLLOUT:  u16 = 0x0004;
    pub const POLLERR:  u16 = 0x0008;
    pub const POLLHUP:  u16 = 0x0010;
    pub const POLLNVAL: u16 = 0x0020;
    pub const POLLRDHUP:u16 = 0x2000;
}
