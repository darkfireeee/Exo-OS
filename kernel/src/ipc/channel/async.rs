//! Async Channels - Asynchronous IPC channels
//!
//! Non-blocking channels with Future support

use crate::ipc::fusion_ring::{FusionRing, Ring, Slot};
use crate::memory::{MemoryResult, MemoryError};
use alloc::sync::Arc;
use alloc::collections::VecDeque;
use core::task::{Context, Poll, Waker};
use spin::Mutex;

/// Async channel state
struct AsyncChannelState {
    /// Pending send wakers
    send_wakers: VecDeque<Waker>,
    
    /// Pending recv wakers
    recv_wakers: VecDeque<Waker>,
    
    /// Channel closed flag
    closed: bool,
}

impl AsyncChannelState {
    fn new() -> Self {
        Self {
            send_wakers: VecDeque::new(),
            recv_wakers: VecDeque::new(),
            closed: false,
        }
    }
    
    fn wake_send(&mut self) {
        if let Some(waker) = self.send_wakers.pop_front() {
            waker.wake();
        }
    }
    
    fn wake_recv(&mut self) {
        if let Some(waker) = self.recv_wakers.pop_front() {
            waker.wake();
        }
    }
}

/// Async channel sender
pub struct AsyncSender<T> {
    ring: Arc<FusionRing>,
    state: Arc<Mutex<AsyncChannelState>>,
    _marker: core::marker::PhantomData<T>,
}

impl<T> AsyncSender<T> {
    /// Send message asynchronously
    pub async fn send(&self, msg: T) -> MemoryResult<()>
    where
        T: AsRef<[u8]>,
    {
        SendFuture {
            sender: self,
            msg: Some(msg),
        }
        .await
    }
    
    /// Try send (non-blocking)
    pub fn try_send(&self, msg: T) -> MemoryResult<()>
    where
        T: AsRef<[u8]>,
    {
        let result = self.ring.send(msg.as_ref());
        
        if result.is_ok() {
            // Wake up waiting receivers
            self.state.lock().wake_recv();
        }
        
        result
    }
    
    /// Close channel
    pub fn close(&self) {
        let mut state = self.state.lock();
        state.closed = true;
        
        // Wake all waiting receivers
        while !state.recv_wakers.is_empty() {
            state.wake_recv();
        }
    }
}

/// Async channel receiver
pub struct AsyncReceiver<T> {
    ring: Arc<FusionRing>,
    state: Arc<Mutex<AsyncChannelState>>,
    _marker: core::marker::PhantomData<T>,
}

impl<T> AsyncReceiver<T> {
    /// Receive message asynchronously
    pub async fn recv(&self, buffer: &mut [u8]) -> MemoryResult<usize> {
        RecvFuture {
            receiver: self,
            buffer,
        }
        .await
    }
    
    /// Try receive (non-blocking)
    pub fn try_recv(&self, buffer: &mut [u8]) -> MemoryResult<usize> {
        let result = self.ring.recv(buffer);
        
        if result.is_ok() {
            // Wake up waiting senders
            self.state.lock().wake_send();
        }
        
        result
    }
}

/// Send future
struct SendFuture<'a, T> {
    sender: &'a AsyncSender<T>,
    msg: Option<T>,
}

impl<'a, T> core::future::Future for SendFuture<'a, T>
where
    T: AsRef<[u8]>,
{
    type Output = MemoryResult<()>;
    
    fn poll(mut self: core::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.as_mut().get_unchecked_mut() };
        let msg = this.msg.take().expect("SendFuture polled after completion");
        let sender = &this.sender;
        
        match sender.ring.send(msg.as_ref()) {
            Ok(()) => {
                // Success, wake receivers
                sender.state.lock().wake_recv();
                Poll::Ready(Ok(()))
            }
            Err(MemoryError::OutOfMemory) => {
                // Ring full, register waker and return pending
                let mut state = sender.state.lock();
                
                if state.closed {
                    return Poll::Ready(Err(MemoryError::NotFound));
                }
                
                state.send_wakers.push_back(cx.waker().clone());
                this.msg = Some(msg);
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

/// Receive future
struct RecvFuture<'a, T> {
    receiver: &'a AsyncReceiver<T>,
    buffer: &'a mut [u8],
}

impl<'a, T> core::future::Future for RecvFuture<'a, T> {
    type Output = MemoryResult<usize>;
    
    fn poll(mut self: core::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.receiver.ring.recv(self.buffer) {
            Ok(size) => {
                // Success, wake senders
                self.receiver.state.lock().wake_send();
                Poll::Ready(Ok(size))
            }
            Err(MemoryError::NotFound) => {
                // Ring empty, register waker
                let mut state = self.receiver.state.lock();
                
                if state.closed {
                    return Poll::Ready(Err(MemoryError::NotFound));
                }
                
                state.recv_wakers.push_back(cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

/// Create async channel
pub fn async_channel<T>() -> MemoryResult<(AsyncSender<T>, AsyncReceiver<T>)> {
    // Allocate fusion ring properly with default capacity (returns &'static Ring)
    const DEFAULT_CAPACITY: usize = 256;
    let actual_ring = crate::ipc::fusion_ring::ring::Ring::new(DEFAULT_CAPACITY);
    let ring = Arc::new(FusionRing {
        ring: Some(actual_ring), // Now actual_ring is &'static Ring
        sync: crate::ipc::fusion_ring::sync::RingSync::new(),
    });
    let state = Arc::new(Mutex::new(AsyncChannelState::new()));
    
    let sender = AsyncSender {
        ring: ring.clone(),
        state: state.clone(),
        _marker: core::marker::PhantomData,
    };
    
    let receiver = AsyncReceiver {
        ring,
        state,
        _marker: core::marker::PhantomData,
    };
    
    Ok((sender, receiver))
}
