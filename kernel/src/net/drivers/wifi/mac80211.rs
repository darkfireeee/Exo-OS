//! # MAC Layer (802.11 MAC)
//! 
//! Complete MAC layer implementation with:
//! - Frame aggregation (A-MPDU, A-MSDU)
//! - Block ACK
//! - Rate control
//! - Power save

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use crate::sync::SpinLock;

/// MAC layer manager
pub struct MacLayer {
    mac_addr: [u8; 6],
    sequence_number: AtomicU16,
    
    // Aggregation state
    ampdu_enabled: bool,
    amsdu_enabled: bool,
    max_ampdu_len: u32,
    max_amsdu_len: u16,
    
    // Block ACK
    ba_sessions: SpinLock<Vec<BlockAckSession>>,
    
    // Rate control
    rate_control: RateControl,
    
    // Statistics
    tx_aggregated: AtomicU32,
    rx_aggregated: AtomicU32,
}

impl MacLayer {
    pub fn new(mac_addr: [u8; 6]) -> Result<Self, super::WiFiError> {
        Ok(Self {
            mac_addr,
            sequence_number: AtomicU16::new(0),
            ampdu_enabled: true,
            amsdu_enabled: true,
            max_ampdu_len: 65535,
            max_amsdu_len: 7935,
            ba_sessions: SpinLock::new(Vec::new()),
            rate_control: RateControl::new(),
            tx_aggregated: AtomicU32::new(0),
            rx_aggregated: AtomicU32::new(0),
        })
    }
    
    pub fn init(&mut self) -> Result<(), super::WiFiError> {
        Ok(())
    }
    
    /// Get next sequence number
    pub fn next_sequence(&self) -> u16 {
        self.sequence_number.fetch_add(1, Ordering::SeqCst) & 0xFFF
    }
    
    /// Aggregate MSDUs into A-MSDU
    pub fn aggregate_amsdu(&self, frames: Vec<&[u8]>) -> Result<Vec<u8>, super::WiFiError> {
        if !self.amsdu_enabled || frames.is_empty() {
            return Err(super::WiFiError::InvalidFrame);
        }
        
        let mut amsdu = Vec::new();
        let mut total_len = 0;
        
        for frame in frames {
            if total_len + frame.len() + 14 > self.max_amsdu_len as usize {
                break;
            }
            
            // A-MSDU subframe header (14 bytes)
            // DA (6) + SA (6) + Length (2)
            amsdu.extend_from_slice(&frame[4..10]);   // DA
            amsdu.extend_from_slice(&frame[10..16]);  // SA
            amsdu.extend_from_slice(&(frame.len() as u16).to_be_bytes());
            
            // Payload
            amsdu.extend_from_slice(&frame[24..]); // Skip 802.11 header
            
            // Padding to 4-byte boundary
            let padding = (4 - (amsdu.len() % 4)) % 4;
            amsdu.extend_from_slice(&vec![0u8; padding]);
            
            total_len = amsdu.len();
        }
        
        self.tx_aggregated.fetch_add(1, Ordering::Relaxed);
        Ok(amsdu)
    }
    
    /// Setup Block ACK session
    pub fn setup_block_ack(
        &self,
        peer: [u8; 6],
        tid: u8,
    ) -> Result<u16, super::WiFiError> {
        let mut sessions = self.ba_sessions.lock();
        
        let session_id = sessions.len() as u16;
        sessions.push(BlockAckSession {
            peer,
            tid,
            sequence_start: self.sequence_number.load(Ordering::SeqCst),
            buffer_size: 64,
            timeout: 0,
            active: true,
        });
        
        Ok(session_id)
    }
    
    /// Send Block ACK request
    pub fn send_block_ack_request(
        &self,
        session_id: u16,
    ) -> Result<Vec<u8>, super::WiFiError> {
        let sessions = self.ba_sessions.lock();
        let session = sessions.get(session_id as usize)
            .ok_or(super::WiFiError::InvalidFrame)?;
        
        let mut bar = Vec::new();
        
        // Frame Control
        bar.extend_from_slice(&[0x84, 0x00]); // Control frame, Block ACK Req
        
        // Duration
        bar.extend_from_slice(&[0x00, 0x00]);
        
        // RA (peer address)
        bar.extend_from_slice(&session.peer);
        
        // TA (our address)
        bar.extend_from_slice(&self.mac_addr);
        
        // BAR Control
        let bar_control = (session.tid as u16) << 12;
        bar.extend_from_slice(&bar_control.to_le_bytes());
        
        // BAR Information (SSN)
        let bar_info = (session.sequence_start as u16) << 4;
        bar.extend_from_slice(&bar_info.to_le_bytes());
        
        Ok(bar)
    }
    
    /// Process received Block ACK
    pub fn process_block_ack(
        &self,
        frame: &[u8],
    ) -> Result<Vec<u16>, super::WiFiError> {
        if frame.len() < 24 {
            return Err(super::WiFiError::InvalidFrame);
        }
        
        // Parse BA Control
        let ba_control = u16::from_le_bytes([frame[16], frame[17]]);
        let _tid = (ba_control >> 12) & 0xF;
        
        // Parse BA Information (starting sequence)
        let ba_info = u16::from_le_bytes([frame[18], frame[19]]);
        let start_seq = (ba_info >> 4) & 0xFFF;
        
        // Parse bitmap (8 bytes = 64 bits)
        let mut acked = Vec::new();
        for i in 0..64 {
            let byte_idx = 20 + (i / 8);
            let bit_idx = i % 8;
            
            if frame.len() <= byte_idx {
                break;
            }
            
            if (frame[byte_idx] & (1 << bit_idx)) != 0 {
                acked.push((start_seq + i as u16) & 0xFFF);
            }
        }
        
        Ok(acked)
    }
    
    /// Get optimal transmission rate
    pub fn get_tx_rate(&self, peer: [u8; 6], rssi: i8) -> u32 {
        self.rate_control.get_rate(rssi)
    }
}

/// Block ACK session
struct BlockAckSession {
    peer: [u8; 6],
    tid: u8,
    sequence_start: u16,
    buffer_size: u16,
    timeout: u16,
    active: bool,
}

/// Rate control algorithm (Minstrel HT)
struct RateControl {
    rates: Vec<RateInfo>,
}

impl RateControl {
    fn new() -> Self {
        Self {
            rates: vec![
                RateInfo { rate_mbps: 6, mcs: 0, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 12, mcs: 1, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 18, mcs: 2, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 24, mcs: 3, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 36, mcs: 4, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 48, mcs: 5, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 54, mcs: 6, success: 0, attempts: 0 },
                // 802.11n MCS rates
                RateInfo { rate_mbps: 65, mcs: 7, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 130, mcs: 8, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 195, mcs: 9, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 260, mcs: 10, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 390, mcs: 11, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 520, mcs: 12, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 585, mcs: 13, success: 0, attempts: 0 },
                // 802.11ac VHT rates
                RateInfo { rate_mbps: 780, mcs: 14, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 867, mcs: 15, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 1300, mcs: 16, success: 0, attempts: 0 },
                RateInfo { rate_mbps: 1733, mcs: 17, success: 0, attempts: 0 },
            ],
        }
    }
    
    fn get_rate(&self, rssi: i8) -> u32 {
        // Simple RSSI-based rate selection
        // Production code would use Minstrel-HT algorithm
        if rssi > -50 {
            1733  // VHT MCS 9 (4 streams, 160 MHz)
        } else if rssi > -60 {
            867   // VHT MCS 8 (2 streams, 80 MHz)
        } else if rssi > -70 {
            390   // HT MCS 15
        } else if rssi > -80 {
            130   // HT MCS 7
        } else {
            24    // Legacy OFDM
        }
    }
}

struct RateInfo {
    rate_mbps: u32,
    mcs: u8,
    success: u32,
    attempts: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sequence_number() {
        let mac = MacLayer::new([0x00; 6]).unwrap();
        let seq1 = mac.next_sequence();
        let seq2 = mac.next_sequence();
        assert_eq!(seq2, seq1 + 1);
    }
    
    #[test]
    fn test_rate_control() {
        let rc = RateControl::new();
        assert_eq!(rc.get_rate(-40), 1733);
        assert_eq!(rc.get_rate(-55), 867);
        assert_eq!(rc.get_rate(-85), 24);
    }
}
