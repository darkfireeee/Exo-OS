// libs/exo-types/src/ipc_msg.rs
//
// Fichier : libs/exo_types/src/ipc_msg.rs
// Rôle    : IpcMessage et IpcEndpoint — GI-01 Étape 5.
//
// INVARIANTS :
//   - IPC-02 : Tous les types Sized et taille fixe. Aucun &str / Vec / Box.
//   - IPC-03 : IpcMessage.sender_pid renseigné par le kernel (non falsifiable Ring 3).
//   - IPC-04 : payload max 120B inline. Données plus grandes → SHM handle.
//   - IPC-04 : payload max 120B inline. Données plus grandes -> SHM handle.
//   - CORR-40/45 : IpcEndpoint DOIT être Copy (assert compile-time).
//
// LAYOUT IpcMessage (128 bytes = enveloppe userspace canonique) :
//   [0]  sender_pid:  u32 — renseigné par kernel
//   [4]  msg_type:    u32 — discriminant du protocole
//   [8]  payload:    [u8;120]
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Architecture_v7.md §1.3, ExoOS_Corrections_06 CORR-17,
//   ExoOS_Corrections_07 CORR-31, GI-01_Types_TCB_SSR.md §5

/// Taille fixe de l'en-tête commun `sender_pid + msg_type`.
pub const IPC_HEADER_SIZE: usize = 8;
/// Nombre d'octets inline disponibles dans l'enveloppe IPC canonique.
pub const IPC_INLINE_PAYLOAD_SIZE: usize = 120;
/// Taille totale de l'enveloppe IPC canonique.
pub const IPC_ENVELOPE_SIZE: usize = IPC_HEADER_SIZE + IPC_INLINE_PAYLOAD_SIZE;

/// Message IPC — exactement **128 bytes**.
///
/// **ABI VERROUILLÉE** — tout changement de layout nécessite la recompilation
/// de TOUS les servers en même temps (workspace unique).
///
/// # Utilisation du payload
/// - Données ≤ 120B : inline dans `payload`.
/// - Données plus grandes : allouer un SHM via `memory_server` et passer un handle dans payload.
///
/// # Réponses directes
/// Retourner via `ipc_send(msg.sender_pid, response)` — `sender_pid` = PID réel garanti kernel.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IpcMessage {
    /// Renseigné par le kernel — PID réel de l'expéditeur (non falsifiable Ring 3). [0] 4B
    pub sender_pid: u32,
    /// Discriminant du protocole Ring 1 (défini dans protocol.rs du server). [4] 4B
    pub msg_type: u32,
    /// Données inline — maximum **120 bytes** (IPC-04). [8] 120B
    pub payload: [u8; IPC_INLINE_PAYLOAD_SIZE],
}

impl Default for IpcMessage {
    #[inline]
    fn default() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0u8; IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

// Assertions ABI compile-time obligatoires (GI-01 §5)
const _: () = assert!(
    core::mem::size_of::<IpcMessage>() == IPC_ENVELOPE_SIZE,
    "IpcMessage doit faire 128B"
);
const _: () = assert!(
    core::mem::offset_of!(IpcMessage, payload) == IPC_HEADER_SIZE,
    "payload doit être à l'offset 8"
);

impl IpcMessage {
    /// Construit une enveloppe IPC entièrement nulle.
    #[inline]
    pub const fn zeroed() -> Self {
        IpcMessage {
            sender_pid: 0,
            msg_type: 0,
            payload: [0u8; IPC_INLINE_PAYLOAD_SIZE],
        }
    }

    /// Construit un message de requête.
    #[inline]
    pub fn new_request(msg_type: u32) -> Self {
        IpcMessage {
            sender_pid: 0,
            msg_type,
            payload: [0u8; IPC_INLINE_PAYLOAD_SIZE],
        }
    }

    /// Lit un u32 depuis le payload à l'offset donné (big-endian libre, little-endian conseillé).
    ///
    /// # Panics
    /// Panique en debug si `offset + 4 > IPC_INLINE_PAYLOAD_SIZE`.
    #[inline]
    pub fn read_u32_le(&self, offset: usize) -> u32 {
        debug_assert!(
            offset + 4 <= IPC_INLINE_PAYLOAD_SIZE,
            "IpcMessage::read_u32_le out of bounds"
        );
        u32::from_le_bytes(self.payload[offset..offset + 4].try_into().unwrap())
    }

    /// Écrit un u32 dans le payload.
    #[inline]
    pub fn write_u32_le(&mut self, offset: usize, val: u32) {
        debug_assert!(
            offset + 4 <= IPC_INLINE_PAYLOAD_SIZE,
            "IpcMessage::write_u32_le out of bounds"
        );
        self.payload[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
    }

    /// Lit un u64 depuis le payload.
    #[inline]
    pub fn read_u64_le(&self, offset: usize) -> u64 {
        debug_assert!(
            offset + 8 <= IPC_INLINE_PAYLOAD_SIZE,
            "IpcMessage::read_u64_le out of bounds"
        );
        u64::from_le_bytes(self.payload[offset..offset + 8].try_into().unwrap())
    }

    /// Écrit un u64 dans le payload.
    #[inline]
    pub fn write_u64_le(&mut self, offset: usize, val: u64) {
        debug_assert!(
            offset + 8 <= IPC_INLINE_PAYLOAD_SIZE,
            "IpcMessage::write_u64_le out of bounds"
        );
        self.payload[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
    }
}

// ─── IpcEndpoint — Identifiant canal IPC ─────────────────────────────────────

/// Endpoint IPC — identifiant d'un canal de réception.
///
/// **CORR-40/45** : Doit rester `Copy` pour utilisation dans les tableaux ISR
/// sans allocation heap. L'assert compile-time ci-dessous garantit cette propriété.
///
/// ❌ INTERDIT : Ajouter `Arc<T>`, `Box<T>` ou tout champ non-`Copy`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IpcEndpoint {
    /// PID du processus owner du canal.
    pub pid: u32,
    /// Index du canal dans la table IPC du processus.
    pub chan_idx: u32,
    /// Génération anti-stale (CORR-17 complémentaire côté endpoint).
    pub generation: u32,
    /// Padding alignement.
    pub _pad: u32,
}

// Garantie Copy — assertion compile-time (CORR-40)
const _: () = assert!(core::mem::size_of::<IpcEndpoint>() == 16);
const fn _assert_copy<T: Copy>() {}
const _: () = _assert_copy::<IpcEndpoint>();
const _: () = _assert_copy::<IpcMessage>();

// ─── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_message_layout() {
        assert_eq!(core::mem::size_of::<IpcMessage>(), IPC_ENVELOPE_SIZE);
        assert_eq!(core::mem::offset_of!(IpcMessage, payload), IPC_HEADER_SIZE);
    }

    #[test]
    fn ipc_endpoint_copy() {
        let ep = IpcEndpoint {
            pid: 1,
            chan_idx: 0,
            generation: 5,
            _pad: 0,
        };
        let ep2 = ep; // Copy
        assert_eq!(ep.pid, ep2.pid);
    }

    #[test]
    fn payload_rw() {
        let mut msg = IpcMessage::new_request(0x42);
        msg.write_u64_le(0, 0x1234_5678_9ABC_DEF0);
        assert_eq!(msg.read_u64_le(0), 0x1234_5678_9ABC_DEF0);
    }
}
