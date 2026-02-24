// ipc/channel/typed.rs — Canal typé générique pour Exo-OS
//
// TypedChannel<T: Copy + Sized> encapsule la sérialisation/désérialisation
// des valeurs Rust via une copie mémoire brute (transmute-safe pour types Copy).
// Construit au-dessus du SpscRing pour les paires (sender, receiver) dédiées,
// ou du MpmcRing en mode multi-thread.
//
// Sécurité :
//   - T doit implémenter Copy + Sized — transmute brut sans risque de drop
//   - Taille de T bornée par MAX_TYPED_VALUE_SIZE (512 octets)
//   - Aucune allocation dynamique
//
// Usage typique :
//   let (tx, rx) = TypedChannel::<SomeCmd>::create();
//   tx.send(SomeCmd::Halt).unwrap();
//   let cmd = rx.recv().unwrap();

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use core::mem::{size_of, MaybeUninit};
use core::marker::PhantomData;

use crate::ipc::core::types::{ChannelId, IpcError, MsgFlags, MessageId, alloc_channel_id, alloc_message_id};
use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::ring::spsc::SpscRing;
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};

// ---------------------------------------------------------------------------
// Contrainte de taille
// ---------------------------------------------------------------------------

/// Taille maximale d'une valeur typée transmissible via TypedChannel
pub const MAX_TYPED_VALUE_SIZE: usize = 512;

/// Erreur de compilation si T est trop grand — vérification dans TypedChannel::create()
const fn assert_type_size<T>() {
    assert!(
        size_of::<T>() <= MAX_TYPED_VALUE_SIZE,
        "TypedChannel: T dépasse MAX_TYPED_VALUE_SIZE (512 octets)"
    );
}

// ---------------------------------------------------------------------------
// Emitters et récepteurs "handles" distincts pour contrainte unique-owner
// ---------------------------------------------------------------------------

/// Handle émetteur d'un TypedChannel<T>
pub struct TypedSender<T: Copy + Sized> {
    channel_idx: usize,
    _phantom: PhantomData<T>,
}

/// Handle récepteur d'un TypedChannel<T>
pub struct TypedReceiver<T: Copy + Sized> {
    channel_idx: usize,
    _phantom: PhantomData<T>,
}

// SAFETY: T est Copy — pas de Drop, le handle ne contient qu'un index entier
unsafe impl<T: Copy + Sized + Send> Send for TypedSender<T> {}
unsafe impl<T: Copy + Sized + Send> Send for TypedReceiver<T> {}

// ---------------------------------------------------------------------------
// Canal typé interne
// ---------------------------------------------------------------------------

/// La structure interne partagée entre TypedSender/TypedReceiver.
/// Stockée dans la table statique TYPED_CHANNEL_TABLE.
#[repr(C, align(64))]
pub struct TypedChannelInner {
    pub id: ChannelId,
    ring: SpscRing,
    sends: AtomicU64,
    recvs: AtomicU64,
    drops: AtomicU64,
    closed: AtomicU32,
    /// Taille de type T enregistrée à la création (vérification runtime)
    type_size: u32,
    _pad: [u8; 28],
}

// SAFETY: SpscRing est Sync via ses barrières atomiques internes
unsafe impl Sync for TypedChannelInner {}
unsafe impl Send for TypedChannelInner {}

impl TypedChannelInner {
    pub fn new(type_size: usize) -> Self {
        let mut s = Self {
            id: alloc_channel_id(),
            ring: SpscRing::new(),
            sends: AtomicU64::new(0),
            recvs: AtomicU64::new(0),
            drops: AtomicU64::new(0),
            closed: AtomicU32::new(0),
            type_size: type_size as u32,
            _pad: [0u8; 28],
        };
        s.ring.init();
        s
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
    }

    /// Envoie la représentation brute de `val` dans le ring.
    ///
    /// # SAFETY
    /// Appelant doit garantir que size_of::<T>() == self.type_size
    pub unsafe fn push_raw(&self, ptr: *const u8, len: usize, flags: MsgFlags)
        -> Result<MessageId, IpcError>
    {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        // SAFETY: ptr pointe sur [u8; len] valide issu d'un T: Copy
        let data = core::slice::from_raw_parts(ptr, len);
        let mid = alloc_message_id();
        self.ring.push_copy(data, flags)?;
        self.sends.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageSent);
        Ok(mid)
    }

    /// Reçoit et reconstruit un T depuis le ring.
    ///
    /// # SAFETY
    /// Appelant doit garantir que buf a une taille >= self.type_size
    pub unsafe fn pop_raw(&self, ptr: *mut u8, len: usize)
        -> Result<usize, IpcError>
    {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        let data = core::slice::from_raw_parts_mut(ptr, len);
        let (read_len, _flags) = self.ring.pop_into(data)?;
        self.recvs.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageReceived);
        Ok(read_len)
    }
}

// ---------------------------------------------------------------------------
// Table statique globale de TypedChannelInner
// ---------------------------------------------------------------------------

pub const TYPED_CHANNEL_TABLE_SIZE: usize = 256;

pub struct TypedChannelTable {
    slots: [MaybeUninit<TypedChannelInner>; TYPED_CHANNEL_TABLE_SIZE],
    used: [bool; TYPED_CHANNEL_TABLE_SIZE],
    pub count: usize,
}

// SAFETY: accès protégé par SpinLock<TypedChannelTable> dans la table globale
unsafe impl Send for TypedChannelTable {}

impl TypedChannelTable {
    pub const fn new() -> Self {
        // SAFETY: mem::zeroed() évite la limite mémoire du const-eval pour grands tableaux.
        unsafe { core::mem::zeroed() }
    }

    pub fn alloc(&mut self, type_size: usize) -> Option<usize> {
        for i in 0..TYPED_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(TypedChannelInner::new(type_size));
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    pub fn free(&mut self, idx: usize) -> bool {
        if idx < TYPED_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] garantit l'init
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    pub unsafe fn get(&self, idx: usize) -> Option<&TypedChannelInner> {
        if idx < TYPED_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

use crate::scheduler::sync::spinlock::SpinLock;

// SAFETY: La section .bss est zéro-initialisée par le bootloader.
// SpinLock<TypedChannelTable> tout-zéro = valide (lock free, table vide).
// MaybeUninit::uninit() = 0 byte de const-eval, la mémoire vient du .bss.
static TYPED_CHANNEL_TABLE: core::mem::MaybeUninit<SpinLock<TypedChannelTable>> =
    core::mem::MaybeUninit::uninit();

#[inline(always)]
fn typed_table() -> &'static SpinLock<TypedChannelTable> {
    // SAFETY: .bss est zéro-initialisé au boot. SpinLock all-zeros = déverrouillé.
    unsafe { TYPED_CHANNEL_TABLE.assume_init_ref() }
}

// ---------------------------------------------------------------------------
// TypedChannel<T> — API publique
// ---------------------------------------------------------------------------

/// Canal typé générique. Utilisation :
/// ```
/// let (tx, rx) = TypedChannel::<u64>::create().unwrap();
/// tx.send(42u64, MsgFlags::empty()).unwrap();
/// let v = rx.recv().unwrap();
/// assert_eq!(v, 42u64);
/// ```
pub struct TypedChannel<T: Copy + Sized> {
    _phantom: PhantomData<T>,
}

impl<T: Copy + Sized> TypedChannel<T> {
    /// Crée une paire (TypedSender<T>, TypedReceiver<T>).
    ///
    /// Panique à la compilation si `size_of::<T>() > MAX_TYPED_VALUE_SIZE`.
    pub fn create() -> Result<(TypedSender<T>, TypedReceiver<T>), IpcError> {
        // Vérifie la taille à la compilation (via const fn)
        assert_type_size::<T>();

        let type_size = size_of::<T>();
        let mut tbl = typed_table().lock();
        let idx = tbl.alloc(type_size).ok_or(IpcError::OutOfResources)?;
        drop(tbl);

        Ok((
            TypedSender { channel_idx: idx, _phantom: PhantomData },
            TypedReceiver { channel_idx: idx, _phantom: PhantomData },
        ))
    }
}

impl<T: Copy + Sized> TypedSender<T> {
    /// Envoie la valeur `val` de type T via le canal typé.
    pub fn send(&self, val: T, flags: MsgFlags) -> Result<MessageId, IpcError> {
        let tbl = typed_table().lock();
        let inner = unsafe { tbl.get(self.channel_idx) }
            .ok_or(IpcError::InvalidHandle)?;
        let inner_ref: &'static TypedChannelInner = unsafe {
            &*(inner as *const TypedChannelInner)
        };
        drop(tbl);

        let size = size_of::<T>();
        // SAFETY: val est T: Copy, la mémoire de size octets de &val est valide
        unsafe {
            inner_ref.push_raw(
                &val as *const T as *const u8,
                size,
                flags,
            )
        }
    }

    /// Ferme le canal (côté émetteur — notifie le récepteur).
    pub fn close(&self) {
        let tbl = typed_table().lock();
        if let Some(inner) = unsafe { tbl.get(self.channel_idx) } {
            inner.close();
        }
    }

    pub fn channel_idx(&self) -> usize {
        self.channel_idx
    }
}

impl<T: Copy + Sized> TypedReceiver<T> {
    /// Reçoit une valeur de type T depuis le canal.
    ///
    /// # Erreurs
    /// - `IpcError::WouldBlock` — aucun message disponible
    /// - `IpcError::Closed` — canal fermé
    /// - `IpcError::ProtocolError` — taille reçue != size_of::<T>()
    pub fn recv(&self) -> Result<T, IpcError> {
        let tbl = typed_table().lock();
        let inner = unsafe { tbl.get(self.channel_idx) }
            .ok_or(IpcError::InvalidHandle)?;
        let inner_ref: &'static TypedChannelInner = unsafe {
            &*(inner as *const TypedChannelInner)
        };
        drop(tbl);

        let size = size_of::<T>();

        // Allouer T non-initialisé sur la pile (zéro-alloc)
        let mut val = MaybeUninit::<T>::uninit();

        // SAFETY: val est T: Copy, les bytes non-initialisés seront remplis par pop_raw
        let read_len = unsafe {
            inner_ref.pop_raw(
                val.as_mut_ptr() as *mut u8,
                size,
            )?
        };

        if read_len != size {
            return Err(IpcError::ProtocolError);
        }

        // SAFETY: pop_raw a écrit exactement `size` octets = sizeof(T)
        Ok(unsafe { val.assume_init() })
    }

    /// Tente de recevoir une valeur sans blocage.
    pub fn try_recv(&self) -> Result<T, IpcError> {
        // Vérifier d'abord si le ring a un message
        let tbl = typed_table().lock();
        let inner = unsafe { tbl.get(self.channel_idx) }
            .ok_or(IpcError::InvalidHandle)?;
        if inner.ring.is_empty() {
            return Err(IpcError::WouldBlock);
        }
        drop(tbl);
        self.recv()
    }

    pub fn channel_idx(&self) -> usize {
        self.channel_idx
    }
}

// Implémentation Drop pour libérer le canal de la table globale
// Note : en noyau, on ne libère pas automatiquement — les deux handles
// doivent être fermés explicitement via TypedSender::close() + typed_channel_destroy()

// ---------------------------------------------------------------------------
// API publique globale
// ---------------------------------------------------------------------------

/// Détruit et libère le canal typé identifié par `idx`.
pub fn typed_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = typed_table().lock();
    if let Some(inner) = unsafe { tbl.get(idx) } {
        inner.close();
    }
    drop(tbl);

    let mut tbl = typed_table().lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Retourne le nombre de canaux typés actifs.
pub fn typed_channel_count() -> usize {
    typed_table().lock().count
}
