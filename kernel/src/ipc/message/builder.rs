// ipc/message/builder.rs — Constructeur fluent de messages IPC pour Exo-OS
//
// Ce module implémente un builder pattern pour assembler des messages IPC
// de manière sûre et ergonomique. Un `IpcMessageBuilder` accumule les métadonnées
// (flags, endpoint source/destination, type, priorité) et le payload inline
// dans un buffer stack-allocated, puis produit un `IpcMessage` prêt à être
// transmis.
//
// RÈGLE MSG-01 : pas d'allocation heap. Le payload est inline (max MAX_MSG_INLINE).
// RÈGLE MSG-02 : le builder est stack-allocated et jamais boxé.
// RÈGLE MSG-03 : build() produit un IpcMessage complet ou retourne une erreur.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError, MessageFlags, MessageType, ProcessId};
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};

// ---------------------------------------------------------------------------
// Constantes
// ---------------------------------------------------------------------------

/// Taille maximale du payload inline dans un IpcMessage (octets)
pub const MAX_MSG_INLINE: usize = 4096;

/// Capacité maximale des descripteurs jointes à un message
pub const MAX_MSG_DESCRIPTORS: usize = 8;

// ---------------------------------------------------------------------------
// IpcDescriptor — handle transmissible
// ---------------------------------------------------------------------------

/// Descripteur transmis avec un message (file descriptor, capability, SHM handle, etc.)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IpcDescriptor {
    pub kind: u32,
    pub handle: u32,
    pub flags: u32,
    pub _pad: u32,
}

impl IpcDescriptor {
    pub const fn new(kind: u32, handle: u32, flags: u32) -> Self {
        Self {
            kind,
            handle,
            flags,
            _pad: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// IpcMessage — message IPC complet
// ---------------------------------------------------------------------------

/// Message IPC complet : header + payload inline + descripteurs.
///
/// Struct packed pour faciliter le passage sur les ring buffers.
/// Total : 4096 + 192 = ~4288 bytes max.
#[repr(C, align(64))]
pub struct IpcMessage {
    // --- Header (64 bytes) ---
    /// Numéro de séquence (assigné par le builder ou l'endpoint)
    pub seq: u64,
    /// Endpoint source
    pub src: EndpointId,
    /// Endpoint destination
    pub dst: EndpointId,
    /// Type de message
    pub msg_type: MessageType,
    /// Flags de message
    pub flags: MessageFlags,
    /// Priorité (0=basse … 255=temps réel)
    pub priority: u8,
    /// Nombre de descripteurs joints
    pub desc_count: u8,
    /// Taille réelle du payload (octets)
    pub payload_len: u16,
    /// Processus émetteur
    pub sender_pid: ProcessId,
    /// Cookie d'identification correlation (pour RPC reply-matching)
    pub cookie: u64,
    _pad: [u8; 14],

    // --- Descripteurs (8 × 16 octets = 128 bytes) ---
    pub descriptors: [IpcDescriptor; MAX_MSG_DESCRIPTORS],

    // --- Payload inline (jusqu'à 4096 octets) ---
    payload: [u8; MAX_MSG_INLINE],
}

impl IpcMessage {
    /// Nouveau message vide (tous les champs à zéro / sentinelles).
    pub const fn empty() -> Self {
        Self {
            seq: 0,
            src: EndpointId::INVALID,
            dst: EndpointId::INVALID,
            msg_type: MessageType::Data,
            flags: MessageFlags::NONE,
            priority: 0,
            desc_count: 0,
            payload_len: 0,
            sender_pid: ProcessId(0),
            cookie: 0,
            _pad: [0u8; 14],
            descriptors: [IpcDescriptor {
                kind: 0,
                handle: 0,
                flags: 0,
                _pad: 0,
            }; MAX_MSG_DESCRIPTORS],
            payload: [0u8; MAX_MSG_INLINE],
        }
    }

    /// Accès en lecture au payload
    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.payload_len as usize]
    }

    /// Accès en écriture au payload (dangereux : ne pas dépasser payload_len)
    pub fn payload_mut(&mut self) -> &mut [u8] {
        let len = self.payload_len as usize;
        &mut self.payload[..len]
    }

    /// Copie le payload dans `out`. Retourne le nombre d'octets copiés.
    pub fn copy_payload_to(&self, out: &mut [u8]) -> usize {
        let len = (self.payload_len as usize).min(out.len());
        out[..len].copy_from_slice(&self.payload[..len]);
        len
    }
}

// ---------------------------------------------------------------------------
// Compteur de séquence global
// ---------------------------------------------------------------------------

static MSG_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_seq() -> u64 {
    MSG_SEQ.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// IpcMessageBuilder — API fluent
// ---------------------------------------------------------------------------

/// Builder stack-allocated pour IpcMessage.
///
/// # Exemple
/// ```rust
/// let msg = IpcMessageBuilder::new()
///     .src(my_ep)
///     .dst(target_ep)
///     .msg_type(MessageType::Data)
///     .priority(128)
///     .payload(data)?
///     .build()?;
/// ```
pub struct IpcMessageBuilder {
    src: EndpointId,
    dst: EndpointId,
    msg_type: MessageType,
    flags: MessageFlags,
    priority: u8,
    sender_pid: ProcessId,
    cookie: u64,
    descriptors: [IpcDescriptor; MAX_MSG_DESCRIPTORS],
    desc_count: usize,
    payload_buf: [u8; MAX_MSG_INLINE],
    payload_len: usize,
    error: Option<IpcError>,
}

impl IpcMessageBuilder {
    /// Crée un nouveau builder avec des valeurs par défaut saines.
    pub fn new() -> Self {
        Self {
            src: EndpointId::INVALID,
            dst: EndpointId::INVALID,
            msg_type: MessageType::Data,
            flags: MessageFlags::NONE,
            priority: 64,
            sender_pid: ProcessId(0),
            cookie: 0,
            descriptors: [IpcDescriptor {
                kind: 0,
                handle: 0,
                flags: 0,
                _pad: 0,
            }; MAX_MSG_DESCRIPTORS],
            desc_count: 0,
            payload_buf: [0u8; MAX_MSG_INLINE],
            payload_len: 0,
            error: None,
        }
    }

    // -----------------------------------------------------------------------
    // Méthodes de configuration (chainables)
    // -----------------------------------------------------------------------

    /// Endpoint source
    pub fn src(mut self, ep: EndpointId) -> Self {
        self.src = ep;
        self
    }

    /// Endpoint destination
    pub fn dst(mut self, ep: EndpointId) -> Self {
        self.dst = ep;
        self
    }

    /// Type de message
    pub fn msg_type(mut self, t: MessageType) -> Self {
        self.msg_type = t;
        self
    }

    /// Flags de message
    pub fn flags(mut self, f: MessageFlags) -> Self {
        self.flags = f;
        self
    }

    /// Priorité (0–255)
    pub fn priority(mut self, p: u8) -> Self {
        self.priority = p;
        self
    }

    /// Processus émetteur
    pub fn sender(mut self, pid: ProcessId) -> Self {
        self.sender_pid = pid;
        self
    }

    /// Cookie de corrélation (pour RPC reply-matching)
    pub fn cookie(mut self, c: u64) -> Self {
        self.cookie = c;
        self
    }

    /// Payload : copie `data` dans le buffer inline.  
    /// Tronque silencieusement si trop grand ? Non — retourne une erreur
    /// dans `build()` via `self.error`.
    pub fn payload(mut self, data: &[u8]) -> Self {
        if data.len() > MAX_MSG_INLINE {
            self.error = Some(IpcError::Invalid);
            return self;
        }
        let len = data.len();
        self.payload_buf[..len].copy_from_slice(data);
        self.payload_len = len;
        self
    }

    /// Payload depuis un slice de u32 (big-endian)
    pub fn payload_u32(mut self, values: &[u32]) -> Self {
        let byte_len = values.len() * 4;
        if byte_len > MAX_MSG_INLINE {
            self.error = Some(IpcError::Invalid);
            return self;
        }
        for (i, v) in values.iter().enumerate() {
            let off = i * 4;
            self.payload_buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.payload_len = byte_len;
        self
    }

    /// Ajoute un descripteur au message.
    pub fn add_descriptor(mut self, desc: IpcDescriptor) -> Self {
        if self.desc_count >= MAX_MSG_DESCRIPTORS {
            self.error = Some(IpcError::Invalid);
            return self;
        }
        self.descriptors[self.desc_count] = desc;
        self.desc_count += 1;
        self
    }

    // -----------------------------------------------------------------------
    // build()
    // -----------------------------------------------------------------------

    /// Construit le message IPC final.
    ///
    /// Retourne `Err` si une erreur a été enregistrée during building, ou
    /// si la destination est invalide (EndpointId(0)).
    pub fn build(self) -> Result<IpcMessage, IpcError> {
        if let Some(e) = self.error {
            IPC_STATS.record(StatEvent::InvalidParam);
            return Err(e);
        }

        if self.dst == EndpointId::INVALID {
            IPC_STATS.record(StatEvent::InvalidParam);
            return Err(IpcError::InvalidEndpoint);
        }

        let mut msg = IpcMessage::empty();
        msg.seq = next_seq();
        msg.src = self.src;
        msg.dst = self.dst;
        msg.msg_type = self.msg_type;
        msg.flags = self.flags;
        msg.priority = self.priority;
        msg.sender_pid = self.sender_pid;
        msg.cookie = self.cookie;
        msg.desc_count = self.desc_count as u8;
        msg.payload_len = self.payload_len as u16;

        // Copie du payload inline
        if self.payload_len > 0 {
            msg.payload[..self.payload_len].copy_from_slice(&self.payload_buf[..self.payload_len]);
        }

        // Copie des descripteurs
        for i in 0..self.desc_count {
            msg.descriptors[i] = self.descriptors[i];
        }

        IPC_STATS.record(StatEvent::MessageSent);
        Ok(msg)
    }
}

impl Default for IpcMessageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers pour messages courants
// ---------------------------------------------------------------------------

/// Crée un message de donnée simple (data message).
pub fn msg_data(src: EndpointId, dst: EndpointId, data: &[u8]) -> Result<IpcMessage, IpcError> {
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::Data)
        .payload(data)
        .build()
}

/// Crée un message de contrôle.
pub fn msg_control(src: EndpointId, dst: EndpointId, cmd: u32) -> Result<IpcMessage, IpcError> {
    let buf = cmd.to_le_bytes();
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::Control)
        .payload(&buf)
        .build()
}

/// Crée un message de signal (notification légère, pas de payload).
pub fn msg_signal(src: EndpointId, dst: EndpointId, signum: u32) -> Result<IpcMessage, IpcError> {
    let buf = signum.to_le_bytes();
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::Signal)
        .priority(200)
        .payload(&buf)
        .build()
}

/// Crée un message de réponse RPC (pour reply-matching via cookie).
pub fn msg_rpc_reply(
    src: EndpointId,
    dst: EndpointId,
    cookie: u64,
    result: &[u8],
) -> Result<IpcMessage, IpcError> {
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::RpcReply)
        .cookie(cookie)
        .payload(result)
        .build()
}
