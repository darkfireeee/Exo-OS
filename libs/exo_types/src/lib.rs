// libs/exo-types/src/lib.rs
//
// Fichier : libs/exo_types/src/lib.rs
// Rôle    : Crate no_std de types partagés ExoOS — GI-01 Étape 1.
//
// DÉPENDANCES : aucune (types fondamentaux)
//
// INVARIANTS :
//   - SRV-02 : AUCUN import blake3 ou chacha20poly1305 dans ce crate.
//   - IPC-02 : Tous les types protocol.rs sont Sized et taille fixe.
//   - CI     : grep -r 'blake3\|chacha20' libs/exo_types/ && exit 1
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Kernel_Types_v10.md §1-2, ExoOS_Architecture_v7.md §1.3,
//   ExoOS_Arborescence_V3.md §2, GI-01_Types_TCB_SSR.md

//! Crate de types partagés ExoOS — no_std.
//!
//! Contient les types fondamentaux du noyau ExoOS (addresses physiques/virtuelles,
//! capabilities, messages IPC, identifiants d'objets…) partagés entre Ring 0, Ring 1 et Ring 3.
//!
//! **SRV-02** : Ce crate n'importe ni `blake3` ni `chacha20poly1305`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)] // Chaque unsafe fn doit justifier ses blocs unsafe
#![warn(missing_docs)]
#![allow(clippy::new_without_default)] // const fn new() ne peut pas impl Default

/// Adresses physiques et virtuelles typées.
pub mod addr;
/// Capabilities, droits et vérification de tokens.
pub mod cap;
/// Constantes globales (taille de page, identifiants réservés).
pub mod constants;
/// Structure ABI epoll et constantes événements.
pub mod epoll;
/// Codes d'erreur unifiés ExoOS/POSIX.
pub mod error;
/// Chaîne de taille fixe no_std (`FixedString<N>`).
pub mod fixed_string;
/// Vecteurs d'E/S `IoVec` compatibles readv/writev.
pub mod iovec;
/// Messages IPC (`IpcMessage`, `IpcEndpoint`).
pub mod ipc_msg;
/// Identifiants d'objets ExoFS (`ObjectId`).
pub mod object_id;
/// Descripteur poll(2) ABI Linux (`PollFd`).
pub mod pollfd;

// ─── Réexports publics ────────────────────────────────────────────────────────
pub use addr::{IoVirtAddr, PhysAddr, VirtAddr};
pub use cap::{CapToken, CapabilityType, Rights, verify_cap_token};
pub use constants::{EXOFS_PAGE_SIZE, ZERO_BLOB_ID_4K};
pub use epoll::EpollEventAbi;
pub use error::ExoError;
pub use fixed_string::{FixedString, PathBuf, ServiceName};
pub use iovec::IoVec;
pub use ipc_msg::{IpcEndpoint, IpcMessage};
pub use object_id::ObjectId;
pub use pollfd::PollFd;
