//! # TCP State Machine
//! 
//! Implémentation de la machine à états TCP (RFC 793)

use core::sync::atomic::{AtomicU8, Ordering};

/// États TCP (RFC 793)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TcpState {
    Closed = 0,
    Listen = 1,
    SynSent = 2,
    SynReceived = 3,
    Established = 4,
    FinWait1 = 5,
    FinWait2 = 6,
    CloseWait = 7,
    Closing = 8,
    LastAck = 9,
    TimeWait = 10,
}

impl TcpState {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Closed),
            1 => Some(Self::Listen),
            2 => Some(Self::SynSent),
            3 => Some(Self::SynReceived),
            4 => Some(Self::Established),
            5 => Some(Self::FinWait1),
            6 => Some(Self::FinWait2),
            7 => Some(Self::CloseWait),
            8 => Some(Self::Closing),
            9 => Some(Self::LastAck),
            10 => Some(Self::TimeWait),
            _ => None,
        }
    }
    
    /// Est-ce que la connexion est établie?
    pub fn is_established(self) -> bool {
        matches!(self, Self::Established)
    }
    
    /// Est-ce que c'est un état de fermeture?
    pub fn is_closing(self) -> bool {
        matches!(self, 
            Self::FinWait1 | Self::FinWait2 | 
            Self::CloseWait | Self::Closing | 
            Self::LastAck | Self::TimeWait
        )
    }
    
    /// Peut-on envoyer des données?
    pub fn can_send(self) -> bool {
        matches!(self, 
            Self::Established | Self::CloseWait
        )
    }
    
    /// Peut-on recevoir des données?
    pub fn can_recv(self) -> bool {
        matches!(self, 
            Self::Established | Self::FinWait1 | Self::FinWait2
        )
    }
}

/// Machine à états TCP (atomic pour concurrence)
pub struct TcpStateMachine {
    state: AtomicU8,
}

impl TcpStateMachine {
    pub fn new(initial: TcpState) -> Self {
        Self {
            state: AtomicU8::new(initial as u8),
        }
    }
    
    /// État actuel
    pub fn current(&self) -> TcpState {
        let val = self.state.load(Ordering::Acquire);
        TcpState::from_u8(val).unwrap_or(TcpState::Closed)
    }
    
    /// Transition d'état
    pub fn transition(&self, new_state: TcpState) -> Result<TcpState, StateError> {
        let old = self.current();
        
        // Vérifie transition valide
        if !self.is_valid_transition(old, new_state) {
            return Err(StateError::InvalidTransition(old, new_state));
        }
        
        self.state.store(new_state as u8, Ordering::Release);
        Ok(old)
    }
    
    /// Force un état (pour tests)
    pub fn force_state(&self, state: TcpState) {
        self.state.store(state as u8, Ordering::Release);
    }
    
    /// Vérifie si la transition est valide
    fn is_valid_transition(&self, from: TcpState, to: TcpState) -> bool {
        use TcpState::*;
        
        match (from, to) {
            // Depuis CLOSED
            (Closed, Listen) => true,
            (Closed, SynSent) => true,
            
            // Depuis LISTEN
            (Listen, SynReceived) => true,
            (Listen, SynSent) => true,
            (Listen, Closed) => true,
            
            // Depuis SYN_SENT
            (SynSent, Established) => true,
            (SynSent, SynReceived) => true,
            (SynSent, Closed) => true,
            
            // Depuis SYN_RECEIVED
            (SynReceived, Established) => true,
            (SynReceived, FinWait1) => true,
            (SynReceived, Closed) => true,
            
            // Depuis ESTABLISHED
            (Established, FinWait1) => true,
            (Established, CloseWait) => true,
            
            // Depuis FIN_WAIT_1
            (FinWait1, FinWait2) => true,
            (FinWait1, Closing) => true,
            (FinWait1, TimeWait) => true,
            
            // Depuis FIN_WAIT_2
            (FinWait2, TimeWait) => true,
            
            // Depuis CLOSE_WAIT
            (CloseWait, LastAck) => true,
            
            // Depuis CLOSING
            (Closing, TimeWait) => true,
            
            // Depuis LAST_ACK
            (LastAck, Closed) => true,
            
            // Depuis TIME_WAIT
            (TimeWait, Closed) => true,
            
            // Même état (no-op)
            (a, b) if a == b => true,
            
            _ => false,
        }
    }
}

/// Événements TCP qui causent des transitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpEvent {
    /// Application appelle listen()
    Listen,
    
    /// Application appelle connect()
    Connect,
    
    /// Application appelle close()
    Close,
    
    /// SYN reçu
    SynReceived,
    
    /// SYN+ACK reçu
    SynAckReceived,
    
    /// ACK reçu
    AckReceived,
    
    /// FIN reçu
    FinReceived,
    
    /// ACK du FIN reçu
    FinAckReceived,
    
    /// Timeout
    Timeout,
    
    /// Reset
    Reset,
}

impl TcpStateMachine {
    /// Traite un événement et fait la transition
    pub fn handle_event(&self, event: TcpEvent) -> Result<TcpState, StateError> {
        let current = self.current();
        
        let new_state = match (current, event) {
            // Application events
            (TcpState::Closed, TcpEvent::Listen) => TcpState::Listen,
            (TcpState::Closed, TcpEvent::Connect) => TcpState::SynSent,
            (TcpState::Listen, TcpEvent::Connect) => TcpState::SynSent,
            
            // SYN received
            (TcpState::Listen, TcpEvent::SynReceived) => TcpState::SynReceived,
            (TcpState::SynSent, TcpEvent::SynReceived) => TcpState::SynReceived,
            
            // SYN+ACK received (active open)
            (TcpState::SynSent, TcpEvent::SynAckReceived) => TcpState::Established,
            
            // ACK received (passive open)
            (TcpState::SynReceived, TcpEvent::AckReceived) => TcpState::Established,
            
            // Close events
            (TcpState::Established, TcpEvent::Close) => TcpState::FinWait1,
            (TcpState::SynReceived, TcpEvent::Close) => TcpState::FinWait1,
            (TcpState::CloseWait, TcpEvent::Close) => TcpState::LastAck,
            
            // FIN received
            (TcpState::Established, TcpEvent::FinReceived) => TcpState::CloseWait,
            (TcpState::FinWait1, TcpEvent::FinReceived) => TcpState::Closing,
            (TcpState::FinWait2, TcpEvent::FinReceived) => TcpState::TimeWait,
            
            // FIN ACK received
            (TcpState::FinWait1, TcpEvent::FinAckReceived) => TcpState::FinWait2,
            (TcpState::Closing, TcpEvent::FinAckReceived) => TcpState::TimeWait,
            (TcpState::LastAck, TcpEvent::FinAckReceived) => TcpState::Closed,
            
            // TIME_WAIT timeout
            (TcpState::TimeWait, TcpEvent::Timeout) => TcpState::Closed,
            
            // Reset
            (_, TcpEvent::Reset) => TcpState::Closed,
            
            _ => return Err(StateError::InvalidEvent(current, event)),
        };
        
        self.transition(new_state)
    }
}

/// Erreurs de machine à états
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    InvalidTransition(TcpState, TcpState),
    InvalidEvent(TcpState, TcpEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_active_open() {
        let sm = TcpStateMachine::new(TcpState::Closed);
        
        // Application connect()
        sm.handle_event(TcpEvent::Connect).unwrap();
        assert_eq!(sm.current(), TcpState::SynSent);
        
        // Reçoit SYN+ACK
        sm.handle_event(TcpEvent::SynAckReceived).unwrap();
        assert_eq!(sm.current(), TcpState::Established);
    }
    
    #[test]
    fn test_passive_open() {
        let sm = TcpStateMachine::new(TcpState::Closed);
        
        // Application listen()
        sm.handle_event(TcpEvent::Listen).unwrap();
        assert_eq!(sm.current(), TcpState::Listen);
        
        // Reçoit SYN
        sm.handle_event(TcpEvent::SynReceived).unwrap();
        assert_eq!(sm.current(), TcpState::SynReceived);
        
        // Reçoit ACK
        sm.handle_event(TcpEvent::AckReceived).unwrap();
        assert_eq!(sm.current(), TcpState::Established);
    }
    
    #[test]
    fn test_active_close() {
        let sm = TcpStateMachine::new(TcpState::Established);
        
        // Application close()
        sm.handle_event(TcpEvent::Close).unwrap();
        assert_eq!(sm.current(), TcpState::FinWait1);
        
        // Reçoit ACK du FIN
        sm.handle_event(TcpEvent::FinAckReceived).unwrap();
        assert_eq!(sm.current(), TcpState::FinWait2);
        
        // Reçoit FIN
        sm.handle_event(TcpEvent::FinReceived).unwrap();
        assert_eq!(sm.current(), TcpState::TimeWait);
        
        // Timeout
        sm.handle_event(TcpEvent::Timeout).unwrap();
        assert_eq!(sm.current(), TcpState::Closed);
    }
    
    #[test]
    fn test_passive_close() {
        let sm = TcpStateMachine::new(TcpState::Established);
        
        // Reçoit FIN
        sm.handle_event(TcpEvent::FinReceived).unwrap();
        assert_eq!(sm.current(), TcpState::CloseWait);
        
        // Application close()
        sm.handle_event(TcpEvent::Close).unwrap();
        assert_eq!(sm.current(), TcpState::LastAck);
        
        // Reçoit ACK
        sm.handle_event(TcpEvent::FinAckReceived).unwrap();
        assert_eq!(sm.current(), TcpState::Closed);
    }
    
    #[test]
    fn test_simultaneous_close() {
        let sm = TcpStateMachine::new(TcpState::Established);
        
        // Application close()
        sm.handle_event(TcpEvent::Close).unwrap();
        assert_eq!(sm.current(), TcpState::FinWait1);
        
        // Reçoit FIN (simultané)
        sm.handle_event(TcpEvent::FinReceived).unwrap();
        assert_eq!(sm.current(), TcpState::Closing);
        
        // Reçoit ACK
        sm.handle_event(TcpEvent::FinAckReceived).unwrap();
        assert_eq!(sm.current(), TcpState::TimeWait);
    }
    
    #[test]
    fn test_reset() {
        let sm = TcpStateMachine::new(TcpState::Established);
        
        sm.handle_event(TcpEvent::Reset).unwrap();
        assert_eq!(sm.current(), TcpState::Closed);
    }
    
    #[test]
    fn test_invalid_transition() {
        let sm = TcpStateMachine::new(TcpState::Closed);
        
        // Ne peut pas aller directement à ESTABLISHED
        let result = sm.transition(TcpState::Established);
        assert!(result.is_err());
    }
}
