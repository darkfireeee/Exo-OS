//! # RDMA Verbs Implementation
//! 
//! InfiniBand verbs API with:
//! - ibv_post_send/recv
//! - ibv_poll_cq
//! - RDMA Read/Write/Atomic
//! - Zero-copy transfers

use alloc::vec::Vec;
use alloc::boxed::Box;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// Work Request opcode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WrOpcode {
    Send = 0,
    SendWithImm = 1,
    RdmaWrite = 2,
    RdmaWriteWithImm = 3,
    RdmaRead = 4,
    AtomicCmpSwap = 5,
    AtomicFetchAdd = 6,
    Recv = 7,
}

/// Work Request flags
pub const IBV_SEND_FENCE: u32 = 1 << 0;
pub const IBV_SEND_SIGNALED: u32 = 1 << 1;
pub const IBV_SEND_SOLICITED: u32 = 1 << 2;
pub const IBV_SEND_INLINE: u32 = 1 << 3;

/// Scatter-Gather Element
#[derive(Debug, Clone)]
pub struct Sge {
    pub addr: u64,
    pub length: u32,
    pub lkey: u32,
}

/// Send Work Request
#[derive(Debug, Clone)]
pub struct SendWr {
    pub wr_id: u64,
    pub next: Option<Box<SendWr>>,
    pub sg_list: Vec<Sge>,
    pub num_sge: u32,
    pub opcode: WrOpcode,
    pub send_flags: u32,
    
    // RDMA specific
    pub remote_addr: u64,
    pub rkey: u32,
    
    // Immediate data
    pub imm_data: Option<u32>,
}

impl SendWr {
    pub fn new(wr_id: u64, opcode: WrOpcode) -> Self {
        Self {
            wr_id,
            next: None,
            sg_list: Vec::new(),
            num_sge: 0,
            opcode,
            send_flags: 0,
            remote_addr: 0,
            rkey: 0,
            imm_data: None,
        }
    }
    
    pub fn add_sge(&mut self, addr: u64, length: u32, lkey: u32) {
        self.sg_list.push(Sge { addr, length, lkey });
        self.num_sge += 1;
    }
}

/// Receive Work Request
#[derive(Debug, Clone)]
pub struct RecvWr {
    pub wr_id: u64,
    pub next: Option<Box<RecvWr>>,
    pub sg_list: Vec<Sge>,
    pub num_sge: u32,
}

impl RecvWr {
    pub fn new(wr_id: u64) -> Self {
        Self {
            wr_id,
            next: None,
            sg_list: Vec::new(),
            num_sge: 0,
        }
    }
    
    pub fn add_sge(&mut self, addr: u64, length: u32, lkey: u32) {
        self.sg_list.push(Sge { addr, length, lkey });
        self.num_sge += 1;
    }
}

/// Work Completion status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WcStatus {
    Success = 0,
    LocLenErr = 1,
    LocQpOpErr = 2,
    LocProtErr = 3,
    WrFlushErr = 4,
    MwBindErr = 5,
    BadRespErr = 6,
    LocAccessErr = 7,
    RemInvReqErr = 8,
    RemAccessErr = 9,
    RemOpErr = 10,
    RetryExcErr = 11,
    RnrRetryExcErr = 12,
    LocRddViolErr = 13,
    RemInvRdReqErr = 14,
    RemAbortErr = 15,
    InvEecnErr = 16,
    InvEecStateErr = 17,
    FatalErr = 18,
    RespTimeoutErr = 19,
    GeneralErr = 20,
}

/// Work Completion
#[derive(Debug, Clone)]
pub struct WorkCompletion {
    pub wr_id: u64,
    pub status: WcStatus,
    pub opcode: WrOpcode,
    pub vendor_err: u32,
    pub byte_len: u32,
    pub imm_data: Option<u32>,
    pub qp_num: u32,
    pub src_qp: u32,
}

/// Queue Pair handle
pub struct QueuePair {
    qp_num: u32,
    send_queue: SpinLock<Vec<SendWr>>,
    recv_queue: SpinLock<Vec<RecvWr>>,
    send_cq: u32,
    recv_cq: u32,
}

impl QueuePair {
    pub fn new(qp_num: u32, send_cq: u32, recv_cq: u32) -> Self {
        Self {
            qp_num,
            send_queue: SpinLock::new(Vec::new()),
            recv_queue: SpinLock::new(Vec::new()),
            send_cq,
            recv_cq,
        }
    }
    
    pub fn qp_num(&self) -> u32 {
        self.qp_num
    }
}

/// Post send work request
pub fn ibv_post_send(qp: &QueuePair, wr: SendWr) -> Result<(), RdmaError> {
    // Validate WR
    if wr.num_sge == 0 {
        return Err(RdmaError::InvalidWr);
    }
    
    // Check opcode validity
    match wr.opcode {
        WrOpcode::Send | WrOpcode::SendWithImm | 
        WrOpcode::RdmaWrite | WrOpcode::RdmaWriteWithImm |
        WrOpcode::RdmaRead | WrOpcode::AtomicCmpSwap | 
        WrOpcode::AtomicFetchAdd => {}
        _ => return Err(RdmaError::InvalidOpcode),
    }
    
    // Add to send queue
    let mut send_queue = qp.send_queue.lock();
    send_queue.push(wr);
    
    // Trigger hardware (mock)
    process_send_queue(qp)?;
    
    Ok(())
}

/// Post receive work request
pub fn ibv_post_recv(qp: &QueuePair, wr: RecvWr) -> Result<(), RdmaError> {
    // Validate WR
    if wr.num_sge == 0 {
        return Err(RdmaError::InvalidWr);
    }
    
    // Add to receive queue
    let mut recv_queue = qp.recv_queue.lock();
    recv_queue.push(wr);
    
    Ok(())
}

/// Poll completion queue
pub fn ibv_poll_cq(cq: &CompletionQueue, num_entries: usize) -> Result<Vec<WorkCompletion>, RdmaError> {
    let mut completions = cq.completions.lock();
    
    let count = completions.len().min(num_entries);
    let result = completions.drain(0..count).collect();
    
    Ok(result)
}

/// Process send queue (hardware emulation)
fn process_send_queue(qp: &QueuePair) -> Result<(), RdmaError> {
    let mut send_queue = qp.send_queue.lock();
    
    while let Some(wr) = send_queue.pop() {
        // Simulate processing
        match wr.opcode {
            WrOpcode::Send => {
                // Send data
            }
            WrOpcode::RdmaWrite => {
                // RDMA write to remote memory
            }
            WrOpcode::RdmaRead => {
                // RDMA read from remote memory
            }
            _ => {}
        }
        
        // Generate completion if signaled
        if wr.send_flags & IBV_SEND_SIGNALED != 0 {
            let wc = WorkCompletion {
                wr_id: wr.wr_id,
                status: WcStatus::Success,
                opcode: wr.opcode,
                vendor_err: 0,
                byte_len: wr.sg_list.iter().map(|sge| sge.length).sum(),
                imm_data: wr.imm_data,
                qp_num: qp.qp_num,
                src_qp: qp.qp_num,
            };
            
            // Add to CQ (mock)
            post_completion(qp.send_cq, wc)?;
        }
    }
    
    Ok(())
}

fn post_completion(cq_num: u32, wc: WorkCompletion) -> Result<(), RdmaError> {
    // Mock: would add to actual CQ
    Ok(())
}

/// Completion Queue
pub struct CompletionQueue {
    cq_num: u32,
    completions: SpinLock<Vec<WorkCompletion>>,
    max_cqe: u32,
}

impl CompletionQueue {
    pub fn new(cq_num: u32, max_cqe: u32) -> Self {
        Self {
            cq_num,
            completions: SpinLock::new(Vec::with_capacity(max_cqe as usize)),
            max_cqe,
        }
    }
}

/// RDMA errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdmaError {
    InvalidWr,
    InvalidOpcode,
    QueueFull,
    NoCompletions,
    ResourceExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_send_wr() {
        let mut wr = SendWr::new(1, WrOpcode::Send);
        wr.add_sge(0x1000, 4096, 0xABCD);
        wr.send_flags = IBV_SEND_SIGNALED;
        
        assert_eq!(wr.num_sge, 1);
        assert_eq!(wr.sg_list[0].length, 4096);
    }
}
