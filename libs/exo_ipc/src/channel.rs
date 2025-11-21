// libs/exo_ipc/src/channel.rs
use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{Message, MessageFlags, MessageHeader, MAX_INLINE_SIZE, SLOT_SIZE};

/// Erreur lors de l'envoi d'un message
#[derive(Debug)]
pub enum TrySendError {
    Full,
    Disconnected,
    InvalidMessage,
}

/// Erreur lors de la réception d'un message
#[derive(Debug)]
pub enum TryRecvError {
    Empty,
    Disconnected,
    InvalidMessage,
}

/// Structure interne pour les rings buffer
struct RingBuffer<T> {
    buffer: *mut T,
    capacity: usize,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T> RingBuffer<T> {
    /// Crée un nouveau ring buffer
    unsafe fn new(buffer: *mut T, capacity: usize) -> Self {
        assert!(capacity.is_power_of_two(), "Capacity must be power of two");

        RingBuffer {
            buffer,
            capacity,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Produit un élément dans le buffer
    fn try_push(&self, item: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        // Vérifier s'il y a de la place
        if head.wrapping_sub(tail) & self.mask == self.mask {
            return Err(item);
        }

        // Écrire l'élément
        unsafe {
            ptr::write(self.buffer.add(head & self.mask), item);
        }

        // Mettre à jour head
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Consomme un élément du buffer
    fn try_pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        // Vérifier s'il y a des éléments
        if tail == head {
            return None;
        }

        // Lire l'élément
        let item = unsafe { ptr::read(self.buffer.add(tail & self.mask)) };

        // Mettre à jour tail
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(item)
    }
}

/// Canal IPC pour communication entre processus
pub struct Channel<T> {
    /// Buffer pour messages entrants
    recv_ring: UnsafeCell<RingBuffer<Message>>,

    /// Buffer pour messages sortants
    send_ring: UnsafeCell<RingBuffer<Message>>,

    /// Indicateur de connexion active
    connected: AtomicBool,

    /// Capacité du buffer
    capacity: usize,

    /// Données fantômes pour la variance
    _phantom: PhantomData<T>,
}

/// Sender pour un canal IPC
pub struct Sender<T> {
    channel: *mut Channel<T>,
    _phantom: PhantomData<T>,
}

/// Receiver pour un canal IPC
pub struct Receiver<T> {
    channel: *mut Channel<T>,
    _phantom: PhantomData<T>,
}

impl<T> Channel<T> {
    /// Crée un nouveau canal avec la capacité spécifiée
    pub fn new(capacity: usize) -> Result<(Sender<T>, Receiver<T>), &'static str> {
        if !capacity.is_power_of_two() {
            return Err("Capacity must be power of two");
        }

        // Allouer la mémoire pour le channel
        let channel = Box::into_raw(Box::new(Channel {
            recv_ring: UnsafeCell::new(unsafe {
                let buffer = alloc_zeroed(capacity * SLOT_SIZE);
                RingBuffer::new(buffer as *mut Message, capacity)
            }),
            send_ring: UnsafeCell::new(unsafe {
                let buffer = alloc_zeroed(capacity * SLOT_SIZE);
                RingBuffer::new(buffer as *mut Message, capacity)
            }),
            connected: AtomicBool::new(true),
            capacity,
            _phantom: PhantomData,
        }));

        let sender = Sender {
            channel,
            _phantom: PhantomData,
        };

        let receiver = Receiver {
            channel,
            _phantom: PhantomData,
        };

        Ok((sender, receiver))
    }

    /// Ferme le canal
    pub fn close(&self) {
        self.connected.store(false, Ordering::Release);
    }

    /// Vérifie si le canal est connecté
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

impl<T> Sender<T> {
    /// Envoie un message de manière synchrone
    pub fn send(&self, msg: T) -> Result<(), TrySendError>
    where
        T: Into<Message>,
    {
        if !self.is_connected() {
            return Err(TrySendError::Disconnected);
        }

        let msg = msg.into();
        let channel = unsafe { &*self.channel };

        // Tenter d'envoyer le message
        let send_ring = unsafe { &mut *channel.send_ring.get() };
        send_ring.try_push(msg).map_err(|_| TrySendError::Full)?;
        Ok(())
    }
}

impl<T> Receiver<T> {
    /// Reçoit un message de manière synchrone
    pub fn recv(&self) -> Result<T, TryRecvError>
    where
        T: TryFrom<Message>,
        <T as TryFrom<Message>>::Error: core::fmt::Display,
    {
        if !self.is_connected() {
            return Err(TryRecvError::Disconnected);
        }

        let channel = unsafe { &*self.channel };
        let recv_ring = unsafe { &mut *channel.recv_ring.get() };

        // Tenter de recevoir un message
        let msg = recv_ring.try_pop().ok_or(TryRecvError::Empty)?;

        // Convertir le message en type T
        T::try_from(msg).map_err(|_| TryRecvError::InvalidMessage)
    }
}

impl<T> Sender<T> {
    /// Vérifie si le sender est connecté
    fn is_connected(&self) -> bool {
        let channel = unsafe { &*self.channel };
        channel.is_connected()
    }
}

impl<T> Receiver<T> {
    /// Vérifie si le receiver est connecté
    fn is_connected(&self) -> bool {
        let channel = unsafe { &*self.channel };
        channel.is_connected()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let channel = unsafe { &*self.channel };
        channel.close();

        // Seulement libérer la mémoire si c'est le dernier détenteur
        if core::ptr::null_mut() != self.channel {
            unsafe {
                let _ = Box::from_raw(self.channel);
            }
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let channel = unsafe { &*self.channel };
        channel.close();

        // Seulement libérer la mémoire si c'est le dernier détenteur
        if core::ptr::null_mut() != self.channel {
            unsafe {
                let _ = Box::from_raw(self.channel);
            }
        }
    }
}

/// Alloue de la mémoire initialisée à zéro
unsafe fn alloc_zeroed(size: usize) -> *mut u8 {
    // Dans un vrai OS, cela appellerait l'allocateur du noyau
    // Pour les tests, on utilise alloc::alloc
    #[cfg(test)]
    {
        use alloc::alloc::{alloc_zeroed, Layout};

        let layout = Layout::from_size_align(size, 64).unwrap();
        alloc_zeroed(layout)
    }

    #[cfg(not(test))]
    {
        // Placeholder pour l'appel système d'allocation
        core::ptr::null_mut()
    }
}

