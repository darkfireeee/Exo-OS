// libs/exo_std/src/ipc/channel.rs
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::boxed::Box;
use exo_ipc::{self, Message, Channel as IpcChannel, Sender as IpcSender, Receiver as IpcReceiver};
use crate::io::{Result as IoResult, IoError};

/// Canal IPC pour communication entre processus
pub struct Channel<T> {
    inner: Arc<InnerChannel<T>>,
}

/// Sender pour un canal IPC
pub struct Sender<T> {
    sender: IpcSender<Message>,
    _marker: PhantomData<T>,
}

/// Receiver pour un canal IPC
pub struct Receiver<T> {
    receiver: IpcReceiver<Message>,
    _marker: PhantomData<T>,
}

struct InnerChannel<T> {
    _phantom: PhantomData<T>,
}

impl<T> Channel<T> {
    /// Crée un nouveau canal IPC
    pub fn new() -> IoResult<(Sender<T>, Receiver<T>)> {
        let (ipc_sender, ipc_receiver) = IpcChannel::new(16)?; // 16 slots
        
        let sender = Sender {
            sender: ipc_sender,
            _marker: PhantomData,
        };
        
        let receiver = Receiver {
            receiver: ipc_receiver,
            _marker: PhantomData,
        };
        
        Ok((sender, receiver))
    }
}

impl<T: Serialize + Deserialize> Sender<T> {
    /// Envoie un message de manière synchrone
    pub fn send(&self, message: T) -> IoResult<()> {
        let mut serialized = Vec::new();
        message.serialize(&mut serialized)?;
    // Créer un message Exo-OS
let mut msg = Message::new();
let size = serialized.len();

if size <= exo_ipc::MAX_INLINE_SIZE {
// Inline path (rapide, ≤56B)
msg.header.size = size as u16;
let flags = exo_ipc::MessageFlags::new().set_inline();
msg.header.flags = flags.0;
msg.data[..size].copy_from_slice(&serialized);
} else {
// Zero-copy path (>56B)
// Créer une mémoire partagée pour les données grandes
let shm = SharedMemory::create(size)?;
shm.write(0, &serialized)?;

// Mettre le handle de mémoire partagée dans le message
msg.header.size = size as u16;
let flags = exo_ipc::MessageFlags::new()
.set_zero_copy()
.set_shared_memory();
msg.header.flags = flags.0;

// Copier le handle de mémoire partagée dans les données
let handle = shm.handle();
let handle_bytes = handle.to_le_bytes();
msg.data[..handle_bytes.len()].copy_from_slice(&handle_bytes);
}

// Envoyer le message via le canal sous-jacent
self.sender.send(msg).map_err(|e| match e {
exo_ipc::Error::Disconnected => IoError::BrokenPipe,
exo_ipc::Error::Full => IoError::WouldBlock,
_ => IoError::Other,
})
}
}

impl<T: Deserialize> Receiver<T> {
/// Reçoit un message de manière synchrone
pub fn recv(&self) -> IoResult<T> {
// Recevoir le message brute
let msg = self.receiver.recv().map_err(|e| match e {
exo_ipc::Error::Disconnected => IoError::BrokenPipe,
exo_ipc::Error::Empty => IoError::WouldBlock,
_ => IoError::Other,
})?;

let flags = exo_ipc::MessageFlags(msg.header.flags);

if flags.contains(exo_ipc::MessageFlags::ZERO_COPY) {
// Chemin zero-copy - récupérer les données depuis la mémoire partagée
if flags.contains(exo_ipc::MessageFlags::SHARED_MEMORY) {
// Extraire le handle de mémoire partagée
let handle_size = core::mem::size_of::<u64>();
let handle_bytes = &msg.data[..handle_size];
let handle = u64::from_le_bytes(handle_bytes.try_into().unwrap());

// Ouvrir la mémoire partagée
let shm = SharedMemory::open_from_handle(handle)?;
let size = msg.header.size as usize;
let mut data = vec![0u8; size];

// Lire les données
shm.read(0, &mut data)?;

// Désérialiser
T::deserialize(&data).map_err(|_| IoError::InvalidData)
} else {
// Zero-copy autre (à implémenter)
Err(IoError::NotSupported)
}
} else {
// Chemin inline - données directement dans le message
let size = msg.header.size as usize;
if size > exo_ipc::MAX_INLINE_SIZE {
return Err(IoError::InvalidData);
}

// Désérialiser directement depuis les données du message
T::deserialize(&msg.data[..size]).map_err(|_| IoError::InvalidData)
}
}
}

// Traits utilitaires pour la sérialisation
trait Serialize {
fn serialize(&self, writer: &mut impl core::fmt::Write) -> IoResult<()>;
}

trait Deserialize: Sized {
fn deserialize(reader: &mut impl core::fmt::Read) -> IoResult<Self>;
}

impl Serialize for u32 {
fn serialize(&self, writer: &mut impl core::fmt::Write) -> IoResult<()> {
write!(writer, "{}", self).map_err(|_| IoError::Other)
}
}

impl Deserialize for u32 {
fn deserialize(reader: &mut impl core::fmt::Read) -> IoResult<Self> {
let mut buf = String::new();
reader.read_to_string(&mut buf).map_err(|| IoError::InvalidData)?;
buf.parse().map_err(|| IoError::InvalidData)
}
}

// MessageFlags extension
trait MessageFlagsExt {
fn set_inline(&self) -> Self;
fn set_zero_copy(&self) -> Self;
fn set_shared_memory(&self) -> Self;
}

impl MessageFlagsExt for exo_ipc::MessageFlags {
fn set_inline(&self) -> Self {
let mut flags = *self;
flags.set(exo_ipc::MessageFlags::INLINE);
flags
}

fn set_zero_copy(&self) -> Self {
let mut flags = *self;
flags.set(exo_ipc::MessageFlags::ZERO_COPY);
flags
}

fn set_shared_memory(&self) -> Self {
let mut flags = *self;
flags.set(exo_ipc::MessageFlags::SHARED_MEMORY);
flags
}
}

#[cfg(test)]
mod tests {
use super::*;
use alloc::string::String;

#[derive(Debug, PartialEq)]
struct TestMessage {
id: u32,
text: String,
}

impl Serialize for TestMessage {
fn serialize(&self, writer: &mut impl core::fmt::Write) -> IoResult<()> {
writeln!(writer, "{}", self.id)?;
writeln!(writer, "{}", self.text)?;
Ok(())
}
}

impl Deserialize for TestMessage {
fn deserialize(reader: &mut impl core::fmt::Read) -> IoResult<Self> {
let mut buf = String::new();
reader.read_line(&mut buf).map_err(|_| IoError::InvalidData)?;
let id = buf.trim().parse().map_err(|_| IoError::InvalidData)?;

let mut text = String::new();
reader.read_to_string(&mut text).map_err(|_| IoError::InvalidData)?;

Ok(TestMessage {
id,
text: text.trim().to_string(),
})
}
}

#[test]
fn test_channel_inline_message() {
let (tx, rx) = Channel::<u32>::new().unwrap();

// Envoyer un petit message (inline path)
tx.send(42).unwrap();

// Recevoir
let received = rx.recv().unwrap();
assert_eq!(received, 42);
}

#[test]
fn test_channel_zero_copy() {
let (tx, rx) = Channel::<TestMessage>::new().unwrap();

// Créer un grand message (zero-copy path)
let msg = TestMessage {
id: 1337,
text: "a".repeat(100), // 100 caractères > 56B
};

// Envoyer
tx.send(msg.clone()).unwrap();

// Recevoir
let received = rx.recv().unwrap();
assert_eq!(received, msg);
}

#[test]
fn test_channel_multiple_messages() {
let (tx, rx) = Channel::<u32>::new().unwrap();

// Envoyer plusieurs messages
for i in 0..10 {
tx.send(i).unwrap();
}

// Recevoir dans le même ordre
for i in 0..10 {
let received = rx.recv().unwrap();
assert_eq!(received, i);
}
}

#[test]
fn test_channel_disconnection() {
let (tx, rx) = Channel::<u32>::new().unwrap();

// Laisser tomber le sender
drop(tx);

// La réception devrait échouer avec BrokenPipe
assert_eq!(rx.recv().unwrap_err(), IoError::BrokenPipe);
}
}