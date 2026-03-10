// ipc/message/serializer.rs — Sérialisation/désérialisation zero-copy IPC pour Exo-OS
//
// Ce module implémente un serializer IPC binaire à disposition fixe :
// pas de capnproto, pas de serde. Le format est défini par des structs repr(C)
// et un protocole de framing minimaliste.
//
// Format de trame :
//   [MsgFrameHeader (32 bytes)] [payload (N bytes)] [padding (0-7 bytes)]
//
// Le serializer opère sur des buffers stack-allocated ou des slices en entrée.
// La désérialisation est zero-copy (référence dans le buffer source).
//
// RÈGLE SER-01 : toute struct sérialisée doit implémenter SerdeFixed.
// RÈGLE SER-02 : pas d'allocation heap.
// RÈGLE SER-03 : les offsets sont en octets depuis le début du buffer.

use core::mem::size_of;
use core::num::NonZeroU64;

use crate::ipc::core::types::{EndpointId, MessageType, MessageFlags, IpcError};
use crate::ipc::message::builder::{IpcMessage, MAX_MSG_INLINE};

// ---------------------------------------------------------------------------
// Traits de sérialisation
// ---------------------------------------------------------------------------

/// Sérialisation/désérialisation pour types à disposition fixe.
///
/// # Sécurité
/// Les types implémentant ce trait DOIVENT être `repr(C)` et ne contenenir
/// que des types primitifs (u8, u16, u32, u64, tableaux de primitifs).
/// Pas de pointeurs, pas de références.
pub unsafe trait SerdeFixed: Sized + Copy {
    const SIZE: usize = size_of::<Self>();

    /// Sérialise en place à `buf[offset..]`. Retourne offset + SIZE.
    fn serialize_into(&self, buf: &mut [u8], offset: usize) -> Result<usize, IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        // SAFETY: Self est Copy + repr(C), pas de pointeurs internes.
        unsafe {
            let src = self as *const Self as *const u8;
            buf[offset..end].copy_from_slice(core::slice::from_raw_parts(src, Self::SIZE));
        }
        Ok(end)
    }

    /// Désérialise depuis `buf[offset..]`. Retourne (Self, offset + SIZE).
    fn deserialize_from(buf: &[u8], offset: usize) -> Result<(Self, usize), IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        let mut value = core::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: Self est Copy + repr(C), taille exacte.
        unsafe {
            let dst = value.as_mut_ptr() as *mut u8;
            dst.copy_from_nonoverlapping(buf[offset..end].as_ptr(), Self::SIZE);
            Ok((value.assume_init(), end))
        }
    }
}

// ---------------------------------------------------------------------------
// En-tête de trame
// ---------------------------------------------------------------------------

/// Magic number pour détecter les trames IPC valides
pub const FRAME_MAGIC: u32 = 0xEA04_4950; // "EA\x04IPC"

/// Version du protocole de sérialisation
pub const FRAME_VERSION: u8 = 1;

/// En-tête de trame IPC (32 bytes, repr(C))
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct MsgFrameHeader {
    /// Magic number de validation
    pub magic: u32,
    /// Version du protocole
    pub version: u8,
    /// Type de message (MessageType as u8)
    pub msg_type: u8,
    /// Flags (MessageFlags as u16)
    pub flags: u16,
    /// Taille du payload en octets
    pub payload_len: u32,
    /// Numéro de séquence
    pub seq: u64,
    /// Endpoint source
    pub src: u32,
    /// Endpoint destination
    pub dst: u32,
    /// Cookie de corrélation
    pub cookie: u64,
    // Total : 4+1+1+2+4+8+4+4+4 = 32 bytes — no _pad needed
}

// SAFETY: repr(C, packed), tous les champs sont des primitifs
unsafe impl SerdeFixed for MsgFrameHeader {}

impl MsgFrameHeader {
    /// Crée un header depuis un IpcMessage
    pub fn from_message(msg: &IpcMessage) -> Self {
        Self {
            magic: FRAME_MAGIC,
            version: FRAME_VERSION,
            msg_type: msg.msg_type as u8,
            flags: msg.flags.bits(),
            payload_len: msg.payload_len as u32,
            seq: msg.seq,
            src: msg.src.0.get() as u32,
            dst: msg.dst.0.get() as u32,
            cookie: msg.cookie,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == FRAME_MAGIC && self.version == FRAME_VERSION
    }

    pub fn frame_size(&self) -> usize {
        size_of::<MsgFrameHeader>() + self.payload_len as usize
    }
}

// ---------------------------------------------------------------------------
// IpcSerializer — sérialise un IpcMessage dans un buffer
// ---------------------------------------------------------------------------

/// Sérialise un `IpcMessage` dans un buffer.
///
/// Format produit :
/// ```text
/// [MsgFrameHeader: 32 bytes] [payload: payload_len bytes]
/// ```
///
/// Retourne le nombre total d'octets écrits, ou `Err(IpcError::Invalid)` si
/// le buffer est trop petit.
pub fn serialize_message(msg: &IpcMessage, buf: &mut [u8]) -> Result<usize, IpcError> {
    let total_needed = size_of::<MsgFrameHeader>() + msg.payload_len as usize;
    if buf.len() < total_needed {
        return Err(IpcError::Invalid);
    }

    let header = MsgFrameHeader::from_message(msg);
    let mut offset = 0;
    offset = header.serialize_into(buf, offset)?;

    // Copier le payload
    let plen = msg.payload_len as usize;
    if plen > 0 {
        msg.copy_payload_to(&mut buf[offset..offset + plen]);
        offset += plen;
    }

    Ok(offset)
}

/// Calcule la taille nécessaire pour sérialiser un message sans allouer.
pub fn serialized_size(msg: &IpcMessage) -> usize {
    size_of::<MsgFrameHeader>() + msg.payload_len as usize
}

// ---------------------------------------------------------------------------
// IpcDeserializer — désérialise un buffer en IpcMessage
// ---------------------------------------------------------------------------

/// Résultat de désérialisation (référence zero-copy dans le buffer)
pub struct DeserializedMessage<'a> {
    pub header: MsgFrameHeader,
    pub payload: &'a [u8],
}

/// Désérialise un buffer IPC en un `DeserializedMessage` zero-copy.
///
/// La durée de vie du payload est liée au buffer source.
pub fn deserialize_message(buf: &[u8]) -> Result<DeserializedMessage<'_>, IpcError> {
    if buf.len() < size_of::<MsgFrameHeader>() {
        return Err(IpcError::Invalid);
    }

    let (header, off) = MsgFrameHeader::deserialize_from(buf, 0)?;

    if !header.is_valid() {
        return Err(IpcError::Invalid);
    }

    let plen = header.payload_len as usize;
    if buf.len() < off + plen {
        return Err(IpcError::Invalid);
    }

    Ok(DeserializedMessage {
        header,
        payload: &buf[off..off + plen],
    })
}

/// Désérialise et copie le résultat dans un `IpcMessage` owned (copie du payload).
pub fn deserialize_into_owned(buf: &[u8]) -> Result<IpcMessage, IpcError> {
    let dm = deserialize_message(buf)?;
    let plen = dm.payload.len();
    if plen > MAX_MSG_INLINE {
        return Err(IpcError::Invalid);
    }

    let mut msg = IpcMessage::empty();
    msg.seq = dm.header.seq;
    msg.src = NonZeroU64::new(dm.header.src as u64)
        .map(EndpointId)
        .unwrap_or(EndpointId::INVALID);
    msg.dst = NonZeroU64::new(dm.header.dst as u64)
        .map(EndpointId)
        .unwrap_or(EndpointId::INVALID);
    msg.msg_type = MessageType::from_u8(dm.header.msg_type);
    msg.flags = MessageFlags::from_bits_truncate(dm.header.flags);
    msg.cookie = dm.header.cookie;
    msg.payload_len = plen as u16;

    if plen > 0 {
        msg.payload_mut().copy_from_slice(dm.payload);
    }

    Ok(msg)
}

// ---------------------------------------------------------------------------
// Multi-messages : framing de plusieurs messages dans un seul buffer
// ---------------------------------------------------------------------------

/// Sérialise une séquence de messages en les enchaînant dans `buf`.
/// Retourne le total d'octets écrits.
pub fn serialize_batch(messages: &[IpcMessage], buf: &mut [u8]) -> Result<usize, IpcError> {
    let mut offset = 0;
    for msg in messages {
        let written = serialize_message(msg, &mut buf[offset..])?;
        offset += written;
    }
    Ok(offset)
}

/// Itérateur zero-copy sur un buffer de messages sérialisés.
pub struct MessageFrameIter<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> MessageFrameIter<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }
}

impl<'a> Iterator for MessageFrameIter<'a> {
    type Item = Result<DeserializedMessage<'a>, IpcError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.buf.len() {
            return None;
        }
        match deserialize_message(&self.buf[self.offset..]) {
            Err(e) => Some(Err(e)),
            Ok(dm) => {
                let frame_size = dm.header.frame_size();
                self.offset += frame_size;
                Some(Ok(dm))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers de sérialisation ad-hoc pour primitifs
// ---------------------------------------------------------------------------

/// Écrit un u32 LE à `buf[offset]`. Retourne offset + 4.
pub fn write_u32(buf: &mut [u8], offset: usize, v: u32) -> Result<usize, IpcError> {
    let end = offset + 4;
    if end > buf.len() { return Err(IpcError::Invalid); }
    buf[offset..end].copy_from_slice(&v.to_le_bytes());
    Ok(end)
}

/// Lit un u32 LE à `buf[offset]`. Retourne (valeur, offset + 4).
pub fn read_u32(buf: &[u8], offset: usize) -> Result<(u32, usize), IpcError> {
    let end = offset + 4;
    if end > buf.len() { return Err(IpcError::Invalid); }
    let v = u32::from_le_bytes([buf[offset], buf[offset+1], buf[offset+2], buf[offset+3]]);
    Ok((v, end))
}

/// Écrit un u64 LE à `buf[offset]`.
pub fn write_u64(buf: &mut [u8], offset: usize, v: u64) -> Result<usize, IpcError> {
    let end = offset + 8;
    if end > buf.len() { return Err(IpcError::Invalid); }
    buf[offset..end].copy_from_slice(&v.to_le_bytes());
    Ok(end)
}

/// Lit un u64 LE à `buf[offset]`.
pub fn read_u64(buf: &[u8], offset: usize) -> Result<(u64, usize), IpcError> {
    let end = offset + 8;
    if end > buf.len() { return Err(IpcError::Invalid); }
    let b: [u8; 8] = [
        buf[offset], buf[offset+1], buf[offset+2], buf[offset+3],
        buf[offset+4], buf[offset+5], buf[offset+6], buf[offset+7],
    ];
    Ok((u64::from_le_bytes(b), end))
}

/// Écrit un slice de bytes dans le buffer, précédé de sa longueur (u32 LE).
pub fn write_bytes(buf: &mut [u8], offset: usize, data: &[u8]) -> Result<usize, IpcError> {
    let off = write_u32(buf, offset, data.len() as u32)?;
    let end = off + data.len();
    if end > buf.len() { return Err(IpcError::Invalid); }
    buf[off..end].copy_from_slice(data);
    Ok(end)
}

/// Lit un slice de bytes depuis le buffer (lu avec sa longueur u32 LE).
pub fn read_bytes<'a>(buf: &'a [u8], offset: usize) -> Result<(&'a [u8], usize), IpcError> {
    let (len, off) = read_u32(buf, offset)?;
    let end = off + len as usize;
    if end > buf.len() { return Err(IpcError::Invalid); }
    Ok((&buf[off..end], end))
}
