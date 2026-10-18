//! # syscall/table/ipc/mod.rs — Handlers IPC natifs, Capabilities, ExoCordon, ExoShield et services système
//!
//! Ce module organise de façon propre et modulaire l'ensemble des handlers d'appels système
//! liés aux communications inter-processus et à la sécurité système :
//! - `endpoints`     : Enregistrement, découverte et cycle de vie des mailboxes IPC.
//! - `channel`       : Envoi (send), réception (recv), appels synchrones (call) et politique ExoCordon.
//! - `remote_memory` : Opérations mémoires inter-processus (copy_from/to_pid, mmap/munmap/mprotect_pid) réservées aux serveurs racine.
//! - `security_ops`  : Gestion des capabilities (create/revoke/check), sandbox Pledge et surveillance ExoShield.
//! - `system_ops`    : Ring buffer de logs kernel, snapshot des processus PCB, métriques de performance et transitions Phoenix.

pub mod channel;
pub mod endpoints;
pub mod remote_memory;
pub mod security_ops;
pub mod system_ops;

pub use channel::*;
pub use endpoints::*;
pub use remote_memory::*;
pub use security_ops::*;
pub use system_ops::*;
