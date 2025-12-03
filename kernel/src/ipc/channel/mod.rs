//! # Canaux de Communication (Channels)
//!
//! Ce module fournit des abstractions de haut niveau pour la communication
//! inter-processus, construites sur les fondations ultra-performantes du
//! `FusionRing` et de la `SharedMemory`.
//!
//! ## Types de Canaux
//!
//! - **`TypedChannel<T>`** : Un canal fortement typé pour envoyer des données
//!   de n'importe quel type `T` qui est sérialisable.
//! - **`AsyncChannel<T>`** : Une version asynchrone de `TypedChannel`,
//!   intégrable avec les runtimes comme Tokio.
//! - **`BroadcastChannel<T>`** : Un canal de diffusion (1 producteur, N consommateurs).
//!
//! ## Canaux Avancés (High-Performance)
//!
//! - **`PriorityChannel`** : 5 niveaux de priorité (RealTime→Bulk)
//! - **`MulticastChannel`** : Un émetteur vers N récepteurs avec gestion lag
//! - **`AnycastChannel`** : Load balancing avec 4 politiques
//! - **`RequestReplyChannel`** : Pattern RPC avec corrélation

// --- Déclaration des sous-modules ---

pub mod typed;       // Canaux typés synchrones.

// Renommer le module async pour éviter le conflit avec le mot-clé
#[path = "async.rs"]
pub mod async_channel;   // Canaux asynchrones.

pub mod broadcast;   // Canaux de diffusion.

// --- Ré-exportation de l'API publique ---

pub use typed::{TypedChannel, TypedSender, TypedReceiver, ChannelError};
pub use async_channel::{AsyncSender, AsyncReceiver, async_channel};
pub use broadcast::{BroadcastSender, BroadcastReceiver, broadcast_channel};

// --- Advanced Channels (from core) ---
pub use crate::ipc::core::{
    PriorityChannel, PriorityChannelStats,
    MulticastChannel, MulticastReceiverState,
    AnycastChannel, AnycastReceiverState,
    RequestReplyChannel,
    PriorityClass, AnycastPolicy,
};