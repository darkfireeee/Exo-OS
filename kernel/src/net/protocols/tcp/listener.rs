/// TCP Listener - Accept incoming connections
/// 
/// Implements a TCP listener that can accept multiple incoming connections
/// with a backlog queue and proper state management.

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::net::tcp::{TcpConnection, TcpStateMachine, TcpState};
use crate::net::core::{NetworkInterface, SocketBuffer};

/// TCP Listener for accepting incoming connections
pub struct TcpListener {
    /// Local address to bind to
    local_addr: [u8; 16],
    /// Local port to bind to
    local_port: u16,
    /// Maximum backlog size
    backlog: usize,
    /// Queue of pending connections
    accept_queue: Mutex<Vec<Arc<TcpConnection>>>,
    /// Listener state
    state: Mutex<ListenerState>,
    /// Network interface for this listener
    interface: Option<Arc<NetworkInterface>>,
}

/// Listener state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerState {
    /// Listener is closed
    Closed,
    /// Listener is bound and listening
    Listening,
    /// Listener is accepting connections
    Accepting,
}

/// Errors that can occur during listener operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerError {
    /// Listener is not in listening state
    NotListening,
    /// Accept queue is full
    QueueFull,
    /// Accept queue is empty (no pending connections)
    QueueEmpty,
    /// Invalid address or port
    InvalidAddress,
    /// Listener already closed
    AlreadyClosed,
    /// Connection handshake failed
    HandshakeFailed,
    /// Timeout waiting for connection
    Timeout,
}

impl TcpListener {
    /// Create a new TCP listener
    pub fn new(addr: [u8; 16], port: u16, backlog: usize) -> Self {
        Self {
            local_addr: addr,
            local_port: port,
            backlog,
            accept_queue: Mutex::new(Vec::with_capacity(backlog)),
            state: Mutex::new(ListenerState::Closed),
            interface: None,
        }
    }

    /// Bind the listener to an interface
    pub fn bind(&mut self, interface: Arc<NetworkInterface>) -> Result<(), ListenerError> {
        if *self.state.lock() != ListenerState::Closed {
            return Err(ListenerError::InvalidAddress);
        }

        self.interface = Some(interface);
        Ok(())
    }

    /// Start listening for incoming connections
    pub fn listen(&self) -> Result<(), ListenerError> {
        let mut state = self.state.lock();
        if *state != ListenerState::Closed {
            return Err(ListenerError::InvalidAddress);
        }

        *state = ListenerState::Listening;
        Ok(())
    }

    /// Accept a pending connection from the queue
    pub fn accept(&self) -> Result<Arc<TcpConnection>, ListenerError> {
        let state = self.state.lock();
        if *state != ListenerState::Listening && *state != ListenerState::Accepting {
            return Err(ListenerError::NotListening);
        }
        drop(state);

        let mut queue = self.accept_queue.lock();
        if queue.is_empty() {
            return Err(ListenerError::QueueEmpty);
        }

        Ok(queue.remove(0))
    }

    /// Try to accept a connection without blocking
    pub fn try_accept(&self) -> Option<Arc<TcpConnection>> {
        self.accept().ok()
    }

    /// Handle an incoming SYN packet and add connection to accept queue
    pub fn handle_syn(&self, skb: &SocketBuffer, remote_addr: [u8; 16], remote_port: u16) -> Result<(), ListenerError> {
        let state = self.state.lock();
        if *state != ListenerState::Listening {
            return Err(ListenerError::NotListening);
        }
        drop(state);

        let mut queue = self.accept_queue.lock();
        if queue.len() >= self.backlog {
            return Err(ListenerError::QueueFull);
        }

        // Create a new connection for this SYN
        let connection = Arc::new(TcpConnection::new(
            self.local_addr,
            self.local_port,
            remote_addr,
            remote_port,
        ));

        // Initialize the connection state machine in SYN-RECEIVED
        // This would normally send SYN-ACK
        // TODO: Implement actual SYN-ACK sending through NetworkInterface

        queue.push(connection);
        Ok(())
    }

    /// Close the listener
    pub fn close(&self) -> Result<(), ListenerError> {
        let mut state = self.state.lock();
        if *state == ListenerState::Closed {
            return Err(ListenerError::AlreadyClosed);
        }

        *state = ListenerState::Closed;

        // Clear the accept queue
        let mut queue = self.accept_queue.lock();
        queue.clear();

        Ok(())
    }

    /// Get the local address
    pub fn local_addr(&self) -> [u8; 16] {
        self.local_addr
    }

    /// Get the local port
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Get the current state
    pub fn state(&self) -> ListenerState {
        *self.state.lock()
    }

    /// Get the number of pending connections
    pub fn pending_count(&self) -> usize {
        self.accept_queue.lock().len()
    }

    /// Get the backlog size
    pub fn backlog(&self) -> usize {
        self.backlog
    }

    /// Check if the accept queue is full
    pub fn is_full(&self) -> bool {
        self.accept_queue.lock().len() >= self.backlog
    }

    /// Check if the accept queue is empty
    pub fn is_empty(&self) -> bool {
        self.accept_queue.lock().is_empty()
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
