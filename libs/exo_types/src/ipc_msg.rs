// libs/exo-types/src/ipc_msg.rs
//
// Fichier : libs/exo_types/src/ipc_msg.rs
// Rôle    : IpcMessage et IpcEndpoint — GI-01 Étape 5.
//
// INVARIANTS :
//   - IPC-02 : Tous les types Sized et taille fixe. Aucun &str / Vec / Box.
//   - IPC-03 : IpcMessage.sender_pid renseigné par le kernel (non falsifiable Ring 3).
//   - IPC-04 : payload max 48B inline. Données > 48B → SHM handle (ObjectId 24B).
//   - CORR-17 : reply_nonce u32 empêche les réponses stales (réutilisation de PID).
//   - CORR-31 : payload = [u8;48] (pas 56) après ajout reply_nonce. ABI verrouillée.
//   - CORR-40/45 : IpcEndpoint DOIT être Copy (assert compile-time).
//
// LAYOUT IpcMessage (64 bytes = 1 cache line) :
//   [0]  sender_pid:  u32 — renseigné par kernel
//   [4]  msg_type:    u32 — discriminant du protocole
//   [8]  reply_nonce: u32 — CORR-17 anti-reuse PID
//   [12] _pad:        u32 — alignement
//   [16] payload:    [u8;48]
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Architecture_v7.md §1.3, ExoOS_Corrections_06 CORR-17,
//   ExoOS_Corrections_07 CORR-31, GI-01_Types_TCB_SSR.md §5

/// Message IPC — exactement **64 bytes** (1 cache line).
///
/// **ABI VERROUILLÉE** — tout changement de layout nécessite la recompilation
/// de TOUS les servers en même temps (workspace unique).
///
/// # Utilisation du payload
/// - Données ≤ 48B : inline dans `payload`.
/// - Données > 48B : allouer un SHM via `memory_server`, passer l'`ObjectId` (24B) dans payload.
///   Pattern : `RequestMsg { shm_handle: ObjectId, len: u32, flags: u32, _pad: [u8;16] }`.
///
/// # Réponses directes
/// Retourner via `ipc_send(msg.sender_pid, response)` — `sender_pid` = PID réel garanti kernel.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IpcMessage {
    /// Renseigné par le kernel — PID réel de l'expéditeur (non falsifiable Ring 3). [0] 4B
    pub sender_pid:  u32,
    /// Discriminant du protocole Ring 1 (défini dans protocol.rs du server). [4] 4B
    pub msg_type:    u32,
    /// **CORR-17** : Nonce anti-reuse PID — généré par le sender, vérifié par le receiver. [8] 4B
    /// Empêche qu'un nouveau processus recevant le PID d'un processus mort
    /// intercepte une réponse destinée à l'ancien processus.
    pub reply_nonce: u32,
    /// Padding alignement. Réservé, doit être 0. [12] 4B
    pub _pad:        u32,
    /// Données inline — maximum **48 bytes** (IPC-04). [16] 48B
    pub payload:     [u8; 48],
}

// Default manuel : [u8; 48] n'implémente pas Default via derive (max = [T;32]).
impl Default for IpcMessage {
    #[inline]
    fn default() -> Self {
        Self {
            sender_pid:  0,
            msg_type:    0,
            reply_nonce: 0,
            _pad:        0,
            payload:     [0u8; 48],
        }
    }
}

// Assertions ABI compile-time obligatoires (GI-01 §5)
const _: () = assert!(core::mem::size_of::<IpcMessage>() == 64,
    "IpcMessage doit faire 64B (1 cache line)");
const _: () = assert!(core::mem::offset_of!(IpcMessage, payload) == 16,
    "payload doit être à l'offset 16");

impl IpcMessage {
    /// Construit un message de requête avec nonce aléatoire.
    ///
    /// `nonce` doit être fourni par l'appelant (depuis CSPRNG ou compteur monotone).
    #[inline]
    pub fn new_request(msg_type: u32, nonce: u32) -> Self {
        IpcMessage {
            sender_pid:  0, // renseigné par kernel à l'envoi
            msg_type,
            reply_nonce: nonce,
            _pad:        0,
            payload:     [0u8; 48],
        }
    }

    /// Construit un message de réponse avec le nonce de la requête originale.
    #[inline]
    pub fn new_reply(msg_type: u32, req_nonce: u32) -> Self {
        IpcMessage {
            sender_pid:  0,
            msg_type,
            reply_nonce: req_nonce, // écho du nonce request pour validation CORR-17
            _pad:        0,
            payload:     [0u8; 48],
        }
    }

    /// Lit un u32 depuis le payload à l'offset donné (big-endian libre, little-endian conseillé).
    ///
    /// # Panics
    /// Panique en debug si `offset + 4 > 48`.
    #[inline]
    pub fn read_u32_le(&self, offset: usize) -> u32 {
        debug_assert!(offset + 4 <= 48, "IpcMessage::read_u32_le out of bounds");
        u32::from_le_bytes(self.payload[offset..offset+4].try_into().unwrap())
    }

    /// Écrit un u32 dans le payload.
    #[inline]
    pub fn write_u32_le(&mut self, offset: usize, val: u32) {
        debug_assert!(offset + 4 <= 48, "IpcMessage::write_u32_le out of bounds");
        self.payload[offset..offset+4].copy_from_slice(&val.to_le_bytes());
    }

    /// Lit un u64 depuis le payload.
    #[inline]
    pub fn read_u64_le(&self, offset: usize) -> u64 {
        debug_assert!(offset + 8 <= 48, "IpcMessage::read_u64_le out of bounds");
        u64::from_le_bytes(self.payload[offset..offset+8].try_into().unwrap())
    }

    /// Écrit un u64 dans le payload.
    #[inline]
    pub fn write_u64_le(&mut self, offset: usize, val: u64) {
        debug_assert!(offset + 8 <= 48, "IpcMessage::write_u64_le out of bounds");
        self.payload[offset..offset+8].copy_from_slice(&val.to_le_bytes());
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
    pub pid:        u32,
    /// Index du canal dans la table IPC du processus.
    pub chan_idx:   u32,
    /// Génération anti-stale (CORR-17 complémentaire côté endpoint).
    pub generation: u32,
    /// Padding alignement.
    pub _pad:       u32,
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
        assert_eq!(core::mem::size_of::<IpcMessage>(), 64);
        assert_eq!(core::mem::offset_of!(IpcMessage, payload), 16);
    }

    #[test]
    fn ipc_endpoint_copy() {
        let ep = IpcEndpoint { pid: 1, chan_idx: 0, generation: 5, _pad: 0 };
        let ep2 = ep; // Copy
        assert_eq!(ep.pid, ep2.pid);
    }

    #[test]
    fn payload_rw() {
        let mut msg = IpcMessage::new_request(0x42, 0xDEAD);
        msg.write_u64_le(0, 0x1234_5678_9ABC_DEF0);
        assert_eq!(msg.read_u64_le(0), 0x1234_5678_9ABC_DEF0);
    }
}
