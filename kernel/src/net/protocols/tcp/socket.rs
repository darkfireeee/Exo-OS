//! # TCP Socket API
//! 
//! High-level TCP socket interface

use alloc::vec::Vec;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::SpinLock;
use super::state::{TcpState, TcpStateMachine};
use super::connection::TcpConnection;

/// TCP Socket
pub struct TcpSocket {
    /// Connexion sous-jacente (si connecté)
    connection: SpinLock<Option<Arc<TcpConnection>>>,
    
    /// État
    state_machine: TcpStateMachine,
    
    /// Port local
    local_port: u16,
    
    /// Adresse locale
    local_addr: [u8; 16],
    
    /// Socket ID
    id: u32,
}

impl TcpSocket {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU32 = AtomicU32::new(1);
        
        Self {
            connection: SpinLock::new(None),
            state_machine: TcpStateMachine::new(TcpState::Closed),
            local_port: 0,
            local_addr: [0; 16],
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
    
    pub fn id(&self) -> u32 {
        self.id
    }
    
    pub fn state(&self) -> TcpState {
        self.state_machine.current()
    }
    
    /// Bind à une adresse locale
    pub fn bind(&mut self, addr: [u8; 16], port: u16) -> Result<(), TcpSocketError> {
        if self.state() != TcpState::Closed {
            return Err(TcpSocketError::InvalidState);
        }
        
        self.local_addr = addr;
        self.local_port = port;
        
        Ok(())
    }
    
    /// Connect à une adresse distante
    pub fn connect(&mut self, remote_addr: [u8; 16], remote_port: u16) -> Result<(), TcpSocketError> {
        if self.state() != TcpState::Closed {
            return Err(TcpSocketError::InvalidState);
        }
        
        // TODO: Créer connexion et initier handshake
        
        Ok(())
    }
    
    /// Send data
    pub fn send(&self, data: &[u8]) -> Result<usize, TcpSocketError> {
        let conn_guard = self.connection.lock();
        let conn = conn_guard.as_ref().ok_or(TcpSocketError::NotConnected)?;
        
        // TODO: Envoyer via connection
        
        Ok(data.len())
    }
    
    /// Receive data
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, TcpSocketError> {
        let conn_guard = self.connection.lock();
        let conn = conn_guard.as_ref().ok_or(TcpSocketError::NotConnected)?;
        
        // TODO: Recevoir via connection
        
        Ok(0)
    }
    
    /// Close socket
    pub fn close(&mut self) -> Result<(), TcpSocketError> {
        if let Some(conn) = self.connection.lock().take() {
            // TODO: Close connection gracefully
        }
        
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpSocketError {
    InvalidState,
    NotConnected,
    AlreadyConnected,
    ConnectionRefused,
    Timeout,
    WouldBlock,
}
