// libs/exo_std/src/ipc/async_channel.rs
use core::task::{Context, Poll, Waker};
use core::pin::Pin;
use core::future::Future;
use alloc::sync::Arc;
use alloc::collections::VecDeque;
use spin::Mutex;
use crate::io::{Result as IoResult, IoError};
use super::channel::{Sender, Receiver};

/// Canal IPC asynchrone
pub struct AsyncChannel<T> {
    inner: Arc<Inner<T>>,
}

/// Sender asynchrone
pub struct AsyncSender<T> {
    inner: Arc<Inner<T>>,
}

/// Receiver asynchrone
pub struct AsyncReceiver<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    buffer: Mutex<VecDeque<T>>,
    capacity: usize,
    senders: Mutex<usize>,
    receivers: Mutex<usize>,
    send_waker: Mutex<Option<Waker>>,
    recv_waker: Mutex<Option<Waker>>,
}

struct SendFuture<T> {
    sender: AsyncSender<T>,
    message: Option<T>,
}

struct RecvFuture<T> {
    receiver: AsyncReceiver<T>,
}

impl<T> AsyncChannel<T> {
    /// Crée un nouveau canal asynchrone avec la capacité spécifiée
    pub fn new() -> IoResult<(AsyncSender<T>, AsyncReceiver<T>)> {
        let inner = Arc::new(Inner {
            buffer: Mutex::new(VecDeque::new()),
            capacity: 16, // Capacité par défaut
            senders: Mutex::new(1),
            receivers: Mutex::new(1),
            send_waker: Mutex::new(None),
            recv_waker: Mutex::new(None),
        });
        
        let sender = AsyncSender {
            inner: inner.clone(),
        };
        
        let receiver = AsyncReceiver {
            inner,
        };
        
        Ok((sender, receiver))
    }
}

impl<T> AsyncSender<T> {
    /// Envoie un message de manière asynchrone
    pub fn send(&self, message: T) -> SendFuture<T> {
        SendFuture {
            sender: self.clone(),
            message: Some(message),
        }
    }
    
    /// Tente d'envoyer un message sans bloquer
    pub fn try_send(&self, message: T) -> IoResult<()> {
        let mut buffer = self.inner.buffer.lock();
        
        if buffer.len() >= self.inner.capacity {
            return Err(IoError::WouldBlock);
        }
        
        buffer.push_back(message);
        
        // Réveiller le receiver s'il attend
        if let Some(waker) = self.inner.recv_waker.lock().take() {
            waker.wake();
        }
        
        Ok(())
    }
}

impl<T> AsyncReceiver<T> {
    /// Reçoit un message de manière asynchrone
    pub fn recv(&self) -> RecvFuture<T> {
        RecvFuture {
            receiver: self.clone(),
        }
    }
    
    /// Tente de recevoir un message sans bloquer
    pub fn try_recv(&self) -> IoResult<T> {
        let mut buffer = self.inner.buffer.lock();
        
        if let Some(msg) = buffer.pop_front() {
            // Réveiller le sender s'il attend
            if let Some(waker) = self.inner.send_waker.lock().take() {
                waker.wake();
            }
            Ok(msg)
        } else {
            Err(IoError::WouldBlock)
        }
    }
}

impl<T> Clone for AsyncSender<T> {
    fn clone(&self) -> Self {
        *self.inner.senders.lock() += 1;
        AsyncSender {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Clone for AsyncReceiver<T> {
    fn clone(&self) -> Self {
        *self.inner.receivers.lock() += 1;
        AsyncReceiver {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Drop for AsyncSender<T> {
    fn drop(&mut self) {
        let mut senders = self.inner.senders.lock();
        *senders -= 1;
        
        if *senders == 0 {
            // Plus de senders - réveiller les receivers bloqués
            if let Some(waker) = self.inner.recv_waker.lock().take() {
                waker.wake();
            }
        }
    }
}

impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        let mut receivers = self.inner.receivers.lock();
        *receivers -= 1;
        
        if *receivers == 0 {
            // Plus de receivers - réveiller les senders bloqués
            if let Some(waker) = self.inner.send_waker.lock().take() {
                waker.wake();
            }
        }
    }
}

impl<T> Future for SendFuture<T>
where
    T: Send + 'static,
{
    type Output = IoResult<()>;
    
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(message) = self.message.take() {
            match self.sender.try_send(message) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(IoError::WouldBlock) => {
                    // Enregistrer le waker pour être réveillé quand de l'espace est disponible
                    *self.sender.inner.send_waker.lock() = Some(cx.waker().clone());
                    self.message = Some(message); // Remettre le message
                    Poll::Pending
                }
                Err(e) => Poll::Ready(Err(e)),
            }
        } else {
            Poll::Ready(Ok(())) // Déjà envoyé
        }
    }
}

impl<T> Future for RecvFuture<T> {
    type Output = IoResult<T>;
    
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.receiver.try_recv() {
            Ok(msg) => Poll::Ready(Ok(msg)),
            Err(IoError::WouldBlock) => {
                // Vérifier si tous les senders sont fermés
                let senders = *self.receiver.inner.senders.lock();
                if senders == 0 {
                    return Poll::Ready(Err(IoError::BrokenPipe));
                }
                
                // Enregistrer le waker pour être réveillé quand des données sont disponibles
                *self.receiver.inner.recv_waker.lock() = Some(cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread;
    use core::time::Duration;
    use futures::executor::block_on;
    
    #[test]
    fn test_async_channel_basic() {
        let (tx, rx) = AsyncChannel::<u32>::new().unwrap();
        
        // Envoyer de manière asynchrone
        block_on(tx.send(42)).unwrap();
        
        // Recevoir de manière asynchrone
        let received = block_on(rx.recv()).unwrap();
        assert_eq!(received, 42);
    }
    
    #[test]
    fn test_async_channel_multiple() {
        let (tx, rx) = AsyncChannel::<u32>::new().unwrap();
        
        // Envoyer plusieurs messages
        block_on(async {
            for i in 0..5 {
                tx.send(i).await.unwrap();
            }
        });
        
        // Recevoir dans le même ordre
        block_on(async {
            for i in 0..5 {
                let received = rx.recv().await.unwrap();
                assert_eq!(received, i);
            }
        });
    }
    
    #[test]
    fn test_async_channel_concurrent() {
        let (tx, rx) = AsyncChannel::<u32>::new().unwrap();
        
        // Créer un thread producteur
        let tx_clone = tx.clone();
        let producer = thread::spawn(move || {
            for i in 0..10 {
                block_on(tx_clone.send(i)).unwrap();
            }
        }).unwrap();
        
        // Consommer dans le thread principal
        for i in 0..10 {
            let received = block_on(rx.recv()).unwrap();
            assert_eq!(received, i);
        }
        
        producer.join().unwrap();
    }
    
    #[test]
    fn test_async_channel_disconnection() {
        let (tx, rx) = AsyncChannel::<u32>::new().unwrap();
        
        // Laisser tomber le sender
        drop(tx);
        
        // La réception devrait échouer avec BrokenPipe
        match block_on(rx.recv()) {
            Err(IoError::BrokenPipe) => {},
            _ => panic!("Expected BrokenPipe error"),
        }
    }
    
    #[test]
    fn test_async_channel_backpressure() {
        let (tx, rx) = AsyncChannel::<u32>::new().unwrap();
        
        // Remplir le buffer
        for i in 0..16 {
            tx.try_send(i).unwrap();
        }
        
        // La tentative d'envoi supplémentaire devrait échouer
        assert_eq!(tx.try_send(17).unwrap_err(), IoError::WouldBlock);
        
        // Recevoir un message
        let received = block_on(rx.recv()).unwrap();
        assert_eq!(received, 0);
        
        // Maintenant l'envoi devrait réussir
        tx.try_send(17).unwrap();
    }
}