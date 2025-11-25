//! Async Channel Wrappers pour Syscalls IPC
//!
//! Fournit des wrappers asynchrones pour les canaux IPC

use crate::ipc::channel::async_channel::{async_channel, AsyncSender, AsyncReceiver};
use crate::memory::MemoryResult;
use alloc::sync::Arc;
use core::marker::PhantomData;

/// Handle asynchrone pour envoi via syscall
pub struct SyscallAsyncSender<T> {
    sender: Arc<AsyncSender<T>>,
    _phantom: PhantomData<T>,
}

impl<T: Clone> SyscallAsyncSender<T> {
    /// Envoie un message de manière asynchrone
    pub async fn send(&self, msg: T) -> MemoryResult<()> {
        self.sender.send(msg).await
    }
    
    /// Essaie d'envoyer sans bloquer
    pub fn try_send(&self, msg: T) -> MemoryResult<()> {
        self.sender.try_send(msg)
    }
    
    /// Clone le sender
    pub fn clone_sender(&self) -> Self {
        Self {
            sender: Arc::clone(&self.sender),
            _phantom: PhantomData,
        }
    }
}

/// Handle asynchrone pour réception via syscall
pub struct SyscallAsyncReceiver<T> {
    receiver: Arc<AsyncReceiver<T>>,
    _phantom: PhantomData<T>,
}

impl<T: Clone> SyscallAsyncReceiver<T> {
    /// Reçoit un message de manière asynchrone
    pub async fn recv(&self) -> MemoryResult<T> {
        self.receiver.recv().await
    }
    
    /// Essaie de recevoir sans bloquer
    pub fn try_recv(&self) -> MemoryResult<T> {
        self.receiver.try_recv()
    }
    
    /// Clone le receiver
    pub fn clone_receiver(&self) -> Self {
        Self {
            receiver: Arc::clone(&self.receiver),
            _phantom: PhantomData,
        }
    }
}

/// Crée une paire de canaux asynchrones (sender/receiver)
pub fn create_async_channel<T: Clone>() -> MemoryResult<(SyscallAsyncSender<T>, SyscallAsyncReceiver<T>)> {
    let (sender, receiver) = async_channel()?;
    
    Ok((
        SyscallAsyncSender {
            sender: Arc::new(sender),
            _phantom: PhantomData,
        },
        SyscallAsyncReceiver {
            receiver: Arc::new(receiver),
            _phantom: PhantomData,
        },
    ))
}

/// Future pour l'envoi asynchrone
pub struct SendFuture<'a, T> {
    sender: &'a SyscallAsyncSender<T>,
    msg: Option<T>,
}

impl<'a, T: Clone> core::future::Future for SendFuture<'a, T> {
    type Output = MemoryResult<()>;
    
    fn poll(mut self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> core::task::Poll<Self::Output> {
        if let Some(msg) = self.msg.take() {
            match self.sender.try_send(msg.clone()) {
                Ok(()) => core::task::Poll::Ready(Ok(())),
                Err(_) => {
                    // Message not sent, restore it and park
                    self.msg = Some(msg);
                    cx.waker().wake_by_ref();
                    core::task::Poll::Pending
                }
            }
        } else {
            core::task::Poll::Ready(Ok(()))
        }
    }
}

/// Future pour la réception asynchrone
pub struct RecvFuture<'a, T> {
    receiver: &'a SyscallAsyncReceiver<T>,
}

impl<'a, T: Clone> core::future::Future for RecvFuture<'a, T> {
    type Output = MemoryResult<T>;
    
    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> core::task::Poll<Self::Output> {
        match self.receiver.try_recv() {
            Ok(msg) => core::task::Poll::Ready(Ok(msg)),
            Err(_) => {
                // No message available, park the task
                cx.waker().wake_by_ref();
                core::task::Poll::Pending
            }
        }
    }
}

/// Canal asynchrone bidirectionnel
pub struct AsyncBidirectionalChannel<T> {
    sender: SyscallAsyncSender<T>,
    receiver: SyscallAsyncReceiver<T>,
}

impl<T: Clone> AsyncBidirectionalChannel<T> {
    /// Crée un canal bidirectionnel asynchrone
    pub fn new() -> MemoryResult<(Self, Self)> {
        let (sender1, receiver1) = create_async_channel()?;
        let (sender2, receiver2) = create_async_channel()?;
        
        Ok((
            Self {
                sender: sender1,
                receiver: receiver2,
            },
            Self {
                sender: sender2,
                receiver: receiver1,
            },
        ))
    }
    
    /// Envoie un message
    pub async fn send(&self, msg: T) -> MemoryResult<()> {
        self.sender.send(msg).await
    }
    
    /// Reçoit un message
    pub async fn recv(&self) -> MemoryResult<T> {
        self.receiver.recv().await
    }
}
