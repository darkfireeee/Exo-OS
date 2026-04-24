// kernel/src/ipc/core/transfer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MESSAGE TRANSFER ENGINE — moteur de transfert de messages IPC
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module implémente le coeur du transfert de messages entre espaces
// d'adressage. Il supporte deux modes :
//
//   1. COPY   — copie physique des données (pour les petits messages ≤ MAX_MSG_SIZE)
//   2. ZEROCOPY — partage de page physique (NO_COW, aucune copie, pour grands volumes)
//
// RÈGLES :
//   • Zéro allocation heap dans ce fichier (Zone NO-ALLOC critique)
//   • Tout unsafe documenté avec // SAFETY:
//   • Pas d'import de fs/, process/ (couche 2a)
//   • Vérification bounds systématique avant toute copie
// ═══════════════════════════════════════════════════════════════════════════════

use super::constants::{MAX_MSG_SIZE, MSG_HEADER_SIZE, RING_SLOT_SIZE};
use super::types::{alloc_message_id, IpcError, MessageId, MsgFlags};
use core::ptr;
use core::sync::atomic::{fence, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// MessageHeader — en-tête inline dans chaque slot du ring
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête de message inliné dans le ring slot.
/// 16 bytes exactement — vérifié statiquement.
#[derive(Copy, Clone, Debug)]
#[repr(C, align(8))]
pub struct MessageHeader {
    /// Identifiant unique du message.
    pub msg_id: u64,
    /// Drapeaux (RT, ZEROCOPY, BROADCAST…).
    pub flags: u32,
    /// Longueur du payload (0..=MAX_MSG_SIZE).
    pub len: u16,
    /// Padding pour aligner sur 16 bytes.
    pub _pad: u16,
}

const _: () = assert!(
    core::mem::size_of::<MessageHeader>() == MSG_HEADER_SIZE,
    "MessageHeader doit faire exactement MSG_HEADER_SIZE bytes"
);

impl MessageHeader {
    /// Crée un en-tête pour un message inline.
    #[inline(always)]
    pub fn new_inline(id: MessageId, flags: MsgFlags, len: usize) -> Self {
        debug_assert!(len <= MAX_MSG_SIZE, "payload dépasse MAX_MSG_SIZE");
        Self {
            msg_id: id.get(),
            flags: flags.0,
            len: len as u16,
            _pad: 0,
        }
    }

    /// Crée un en-tête pour un message zero-copy.
    #[inline(always)]
    pub fn new_zerocopy(id: MessageId, flags: MsgFlags) -> Self {
        let f = MsgFlags(flags.0 | MsgFlags::ZEROCOPY.0);
        Self {
            msg_id: id.get(),
            flags: f.0,
            len: 0, // payload est une référence physique externe
            _pad: 0,
        }
    }

    /// Retourne vrai si le message est zero-copy.
    #[inline(always)]
    pub fn is_zerocopy(&self) -> bool {
        (self.flags & MsgFlags::ZEROCOPY.0) != 0
    }

    /// Retourne vrai si le message est temps-réel.
    #[inline(always)]
    pub fn is_rt(&self) -> bool {
        (self.flags & MsgFlags::RT.0) != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RingSlot — slot complet dans le ring (header + payload inliné)
// ─────────────────────────────────────────────────────────────────────────────

/// Un slot complet dans le ring de communication.
/// Taille fixe RING_SLOT_SIZE = MSG_HEADER_SIZE + MAX_MSG_SIZE.
#[repr(C, align(64))]
pub struct RingSlot {
    pub header: MessageHeader,
    pub payload: [u8; MAX_MSG_SIZE],
}

const _: () = assert!(
    core::mem::size_of::<RingSlot>() == RING_SLOT_SIZE,
    "RingSlot doit faire RING_SLOT_SIZE bytes"
);

impl RingSlot {
    /// Crée un slot vide (zéro-initialisé).
    pub const fn zeroed() -> Self {
        Self {
            header: MessageHeader {
                msg_id: 0,
                flags: 0,
                len: 0,
                _pad: 0,
            },
            payload: [0u8; MAX_MSG_SIZE],
        }
    }

    /// Écrit un message inline dans ce slot.
    ///
    /// # Safety
    /// `src` doit pointer sur au moins `len` bytes valides.
    /// `len` doit être ≤ MAX_MSG_SIZE.
    #[inline]
    pub unsafe fn write_inline(
        &mut self,
        id: MessageId,
        flags: MsgFlags,
        src: *const u8,
        len: usize,
    ) -> Result<(), IpcError> {
        if len > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        self.header = MessageHeader::new_inline(id, flags, len);
        if len > 0 {
            // SAFETY: src valide (précondition), payload[..len] dans le slot.
            ptr::copy_nonoverlapping(src, self.payload.as_mut_ptr(), len);
        }
        // Barrière Release — les données sont visibles avant que le ring
        // n'incrémente le tail et rende le slot lisible.
        fence(Ordering::Release);
        Ok(())
    }

    /// Lit le payload depuis ce slot vers un buffer destination.
    ///
    /// # Safety
    /// `dst` doit pointer sur au moins `buf_len` bytes valides.
    /// Appelé uniquement après une barrière Acquire sur le ring.
    #[inline]
    pub unsafe fn read_inline(&self, dst: *mut u8, buf_len: usize) -> Result<usize, IpcError> {
        let len = self.header.len as usize;
        if len > buf_len {
            return Err(IpcError::MessageTooLarge);
        }
        if len > MAX_MSG_SIZE {
            return Err(IpcError::InternalError); // corruption
        }
        // SAFETY: dst valide (précondition), payload[..len] initialisé par write_inline.
        ptr::copy_nonoverlapping(self.payload.as_ptr(), dst, len);
        Ok(len)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZeroCopyRef — référence physique pour transfert sans copie
// ─────────────────────────────────────────────────────────────────────────────

/// Référence physique vers un buffer partagé (zero-copy IPC).
/// La page sous-jacente est mappée dans les deux espaces d'adressage
/// avec le flag NO_COW (géré par shared_memory/pool.rs).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct ZeroCopyRef {
    /// Adresse physique du buffer (alignée sur PAGE_SIZE).
    pub phys_addr: u64,
    /// Longueur du buffer en bytes.
    pub length: u32,
    /// Index dans le pool SHM (pour libération).
    pub pool_idx: u32,
}

impl ZeroCopyRef {
    /// Crée une référence zero-copy.
    #[inline(always)]
    pub fn new(phys_addr: u64, length: u32, pool_idx: u32) -> Self {
        Self {
            phys_addr,
            length,
            pool_idx,
        }
    }

    /// Retourne vrai si la référence est valide (non nulle).
    #[inline(always)]
    pub fn is_valid(&self) -> bool {
        self.phys_addr != 0 && self.length > 0
    }

    /// Référence nulle (adresse physique 0, sans buffer).
    #[inline(always)]
    pub const fn null() -> Self {
        Self {
            phys_addr: 0,
            length: 0,
            pool_idx: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TransferEngine — moteur de copie optimisé
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques du moteur de transfert.
#[derive(Default, Debug)]
pub struct TransferStats {
    /// Nombre de transferts inline effectués.
    pub inline_count: u64,
    /// Nombre de transferts zero-copy effectués.
    pub zerocopy_count: u64,
    /// Bytes totaux copiés (inline seulement).
    pub bytes_copied: u64,
    /// Nombre d'erreurs (bounds violations, etc.).
    pub errors: u64,
}

/// Moteur de transfert stateless.
/// Toutes les méthodes sont des fonctions pures — l'état est dans les slots.
pub struct TransferEngine;

impl TransferEngine {
    /// Copie `src[0..len]` dans le slot de destination.
    ///
    /// # Safety
    /// - `src` doit pointer sur au moins `len` bytes valides.
    /// - `slot` doit être exclusivement détenu par l'appelant.
    #[inline]
    pub unsafe fn copy_to_slot(
        slot: &mut RingSlot,
        src: *const u8,
        len: usize,
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        if len > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        let id = alloc_message_id();
        // SAFETY: src valide et len ≤ MAX_MSG_SIZE vérifiés ci-dessus.
        slot.write_inline(id, flags, src, len)?;
        Ok(id)
    }

    /// Copie le payload du slot vers `dst[0..buf_len]`.
    ///
    /// # Safety
    /// - `dst` doit pointer sur au moins `buf_len` bytes valides.
    /// - Une barrière Acquire doit avoir été posée avant cet appel.
    #[inline]
    pub unsafe fn copy_from_slot(
        slot: &RingSlot,
        dst: *mut u8,
        buf_len: usize,
    ) -> Result<usize, IpcError> {
        // SAFETY: dst valide et buf_len vérifiés par l'appelant.
        slot.read_inline(dst, buf_len)
    }

    /// Transfert slice → slot (version typée safe pour des types Copy).
    ///
    /// # Contrainte
    /// `T` doit être `Copy + 'static` (pas de pointeurs internes).
    #[inline]
    pub fn transfer_value<T: Copy>(
        slot: &mut RingSlot,
        value: &T,
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        let len = core::mem::size_of::<T>();
        // SAFETY: value est une référence valide vers un T de taille `len`.
        unsafe { Self::copy_to_slot(slot, value as *const T as *const u8, len, flags) }
    }

    /// Reçoit un slot vers un type `T` (version typée safe).
    ///
    /// # Safety
    /// Le slot doit avoir été écrit par `transfer_value::<T>`.
    /// L'alignement de T doit être ≤ à celui de `RingSlot::payload`.
    #[inline]
    pub fn receive_value<T: Copy>(slot: &RingSlot) -> Result<T, IpcError> {
        let len = core::mem::size_of::<T>();
        if slot.header.len as usize != len {
            return Err(IpcError::InvalidParam);
        }
        // SAFETY: payload contient exactement `len` bytes du type T.
        // L'alignement de payload (=[u8]) est 1 — on utilise read_unaligned.
        let t: T = unsafe { ptr::read_unaligned(slot.payload.as_ptr() as *const T) };
        Ok(t)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation des invariants à la compilation
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    core::mem::size_of::<ZeroCopyRef>() == 16,
    "ZeroCopyRef doit faire 16 bytes"
);
