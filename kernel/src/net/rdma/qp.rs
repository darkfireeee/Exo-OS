//! # RDMA Queue Pairs
//! 
//! Queue Pair (QP) management with:
//! - QP types (RC, UC, UD)
//! - State machine (RESET → INIT → RTR → RTS)
//! - Attributes modification

use alloc::vec::Vec;
use crate::sync::SpinLock;

/// QP type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QpType {
    Rc = 2,  // Reliable Connection
    Uc = 3,  // Unreliable Connection
    Ud = 4,  // Unreliable Datagram
}

/// QP state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QpState {
    Reset = 0,
    Init = 1,
    Rtr = 2,   // Ready to Receive
    Rts = 3,   // Ready to Send
    Sqd = 4,   // Send Queue Drained
    Sqe = 5,   // Send Queue Error
    Err = 6,
}

/// QP capabilities
#[derive(Debug, Clone)]
pub struct QpCap {
    pub max_send_wr: u32,
    pub max_recv_wr: u32,
    pub max_send_sge: u32,
    pub max_recv_sge: u32,
    pub max_inline_data: u32,
}

impl Default for QpCap {
    fn default() -> Self {
        Self {
            max_send_wr: 256,
            max_recv_wr: 256,
            max_send_sge: 1,
            max_recv_sge: 1,
            max_inline_data: 64,
        }
    }
}

/// QP attributes
#[derive(Debug, Clone)]
pub struct QpAttr {
    pub qp_state: QpState,
    pub qp_type: QpType,
    pub port_num: u8,
    pub pkey_index: u16,
    pub qkey: u32,
    
    // RC/UC specific
    pub dest_qp_num: u32,
    pub rq_psn: u32,  // Receive Packet Sequence Number
    pub sq_psn: u32,  // Send Packet Sequence Number
    
    // Timeouts
    pub timeout: u8,
    pub retry_cnt: u8,
    pub rnr_retry: u8,
    
    // Path MTU
    pub path_mtu: PathMtu,
}

/// Path MTU
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PathMtu {
    Mtu256 = 1,
    Mtu512 = 2,
    Mtu1024 = 3,
    Mtu2048 = 4,
    Mtu4096 = 5,
}

/// Queue Pair
pub struct QpFull {
    qp_num: u32,
    attr: SpinLock<QpAttr>,
    cap: QpCap,
    send_cq: u32,
    recv_cq: u32,
}

impl QpFull {
    pub fn new(qp_num: u32, qp_type: QpType, cap: QpCap, send_cq: u32, recv_cq: u32) -> Self {
        let attr = QpAttr {
            qp_state: QpState::Reset,
            qp_type,
            port_num: 1,
            pkey_index: 0,
            qkey: 0,
            dest_qp_num: 0,
            rq_psn: 0,
            sq_psn: 0,
            timeout: 14,
            retry_cnt: 7,
            rnr_retry: 7,
            path_mtu: PathMtu::Mtu4096,
        };
        
        Self {
            qp_num,
            attr: SpinLock::new(attr),
            cap,
            send_cq,
            recv_cq,
        }
    }
    
    /// Modify QP attributes
    pub fn modify(&self, new_attr: QpAttr) -> Result<(), QpError> {
        let mut attr = self.attr.lock();
        
        // Validate state transition
        let valid = match (attr.qp_state, new_attr.qp_state) {
            (QpState::Reset, QpState::Init) => true,
            (QpState::Init, QpState::Rtr) => true,
            (QpState::Rtr, QpState::Rts) => true,
            (_, QpState::Reset) => true, // Can always reset
            (_, QpState::Err) => true,   // Can always error
            (current, target) if current == target => true,
            _ => false,
        };
        
        if !valid {
            return Err(QpError::InvalidStateTransition);
        }
        
        *attr = new_attr;
        Ok(())
    }
    
    /// Get QP number
    pub fn qp_num(&self) -> u32 {
        self.qp_num
    }
    
    /// Get QP state
    pub fn state(&self) -> QpState {
        self.attr.lock().qp_state
    }
}

/// QP errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QpError {
    InvalidStateTransition,
    InvalidAttribute,
    ResourceExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_qp_state_machine() {
        let qp = QpFull::new(1, QpType::Rc, QpCap::default(), 1, 1);
        
        // RESET → INIT
        let mut attr = QpAttr {
            qp_state: QpState::Init,
            qp_type: QpType::Rc,
            port_num: 1,
            pkey_index: 0,
            qkey: 0,
            dest_qp_num: 0,
            rq_psn: 0,
            sq_psn: 0,
            timeout: 14,
            retry_cnt: 7,
            rnr_retry: 7,
            path_mtu: PathMtu::Mtu4096,
        };
        assert!(qp.modify(attr.clone()).is_ok());
        
        // INIT → RTR
        attr.qp_state = QpState::Rtr;
        attr.dest_qp_num = 2;
        assert!(qp.modify(attr.clone()).is_ok());
        
        // RTR → RTS
        attr.qp_state = QpState::Rts;
        assert!(qp.modify(attr).is_ok());
        
        assert_eq!(qp.state(), QpState::Rts);
    }
}
