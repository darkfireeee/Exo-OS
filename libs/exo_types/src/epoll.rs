// libs/exo-types/src/epoll.rs
//
// Fichier : libs/exo_types/src/epoll.rs
// Rôle    : EpollEventAbi — ABI Linux pour epoll — GI-01 Étape 7.
//
// INVARIANTS :
//   - CORR-06 : #[repr(C, packed)] + data stocké en [u8;8] pour éviter UB E0793.
//   - TL-36  : size_of::<EpollEventAbi>() == 12 (ABI Linux exacte).
//   - data_u64() / set_data_u64() : seules méthodes d'accès sûres au champ data.
//
// PROBLÈME UB RUST E0793 (rappel) :
//   Dans une struct `repr(packed)`, accéder à un champ `u64` non-aligné via référence
//   = Undefined Behavior depuis Rust 1.72 (erreur de compilation E0793).
//   Solution : stocker `data` comme `[u8; 8]` avec accesseurs par valeur.
//
// SOURCE DE VÉRITÉ :
//   ExoFS_Translation_Layer_v5_FINAL.md §1.1, ExoOS_Corrections_04 CORR-06,
//   GI-01_Types_TCB_SSR.md §6

/// ABI Linux exacte pour `epoll_event` — **12 bytes** (pas d'alignement implicite).
///
/// **CORR-06** : `data` est stocké comme `[u8; 8]` pour éviter l'UB E0793 (Rust 1.72+).
///
/// ❌ ERREUR COURANTE après migration :
/// ```ignore
/// let d = epoll_event.data;          // UB si repr(packed) + champ u64 non-aligné
/// let d = &epoll_event.data_bytes;   // OK — c'est un [u8;8], toujours aligné sur 1
/// let d = epoll_event.data_u64();    // ✅ CORRECT — lecture unaligned safe
/// ```
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct EpollEventAbi {
    /// Bitmask des événements surveilles/retournés (EPOLLIN, EPOLLOUT, etc.).
    pub events:    u32,
    /// Données utilisateur — **NE PAS accéder directement** (UB packed).
    /// Utiliser `data_u64()` et `set_data_u64()`.
    data_bytes:   [u8; 8],
}

// Assertions ABI Linux compile-time (TL-36)
const _: () = assert!(core::mem::size_of::<EpollEventAbi>() == 12,
    "EpollEventAbi doit faire 12B (ABI Linux epoll_event)");
const _: () = assert!(core::mem::offset_of!(EpollEventAbi, data_bytes) == 4,
    "data_bytes doit être à l'offset 4");

impl EpollEventAbi {
    /// Lit la valeur `u64` du champ `data` — **toujours safe** (lecture unaligned explicite).
    #[inline(always)]
    pub fn data_u64(&self) -> u64 {
        // SAFETY: from_ne_bytes sur [u8;8] = lecture unaligned sûre en Rust.
        // Pas de référence au champ — évite E0793.
        u64::from_ne_bytes(self.data_bytes)
    }

    /// Écrit la valeur `u64` du champ `data` — **toujours safe**.
    #[inline(always)]
    pub fn set_data_u64(&mut self, v: u64) {
        self.data_bytes = v.to_ne_bytes();
    }

    /// Constructeur complet.
    #[inline]
    pub fn new(events: u32, data: u64) -> Self {
        EpollEventAbi {
            events,
            data_bytes: data.to_ne_bytes(),
        }
    }
}

// ─── Constantes epoll ─────────────────────────────────────────────────────────
pub const EPOLLIN:      u32 = 0x0000_0001;
pub const EPOLLPRI:     u32 = 0x0000_0002;
pub const EPOLLOUT:     u32 = 0x0000_0004;
pub const EPOLLERR:     u32 = 0x0000_0008;
pub const EPOLLHUP:     u32 = 0x0000_0010;
pub const EPOLLRDHUP:   u32 = 0x0000_2000;
pub const EPOLLET:      u32 = 0x8000_0000; // Edge Triggered
pub const EPOLLONESHOT: u32 = 0x4000_0000;

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;
pub const EPOLL_CLOEXEC: i32 = 0o2000000; // O_CLOEXEC Linux

// ─── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_size_12() {
        assert_eq!(core::mem::size_of::<EpollEventAbi>(), 12);
    }

    #[test]
    fn data_roundtrip() {
        let mut e = EpollEventAbi::new(EPOLLIN, 0xDEAD_BEEF_CAFE_BABE);
        assert_eq!(e.data_u64(), 0xDEAD_BEEF_CAFE_BABE);
        e.set_data_u64(42);
        assert_eq!(e.data_u64(), 42);
    }

    #[test]
    fn data_offset_4() {
        assert_eq!(core::mem::offset_of!(EpollEventAbi, data_bytes), 4);
    }
}
