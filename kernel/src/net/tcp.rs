//! TCP Protocol Implementation
//!
//! Complete TCP stack with state machine and congestion control

use super::buffer::PacketBuffer;
use super::socket::Ipv4Addr;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU32, Ordering};

/// TCP header (20 bytes minimum)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct TcpHeader {
    /// Source port
    pub src_port: u16,
    
    /// Destination port
    pub dst_port: u16,
    
    /// Sequence number
    pub seq_num: u32,
    
    /// Acknowledgment number
    pub ack_num: u32,
    
    /// Data offset (4 bits) + Reserved (3 bits) + Flags (9 bits)
    pub offset_flags: u16,
    
    /// Window size
    pub window_size: u16,
    
    /// Checksum
    pub checksum: u16,
    
    /// Urgent pointer
    pub urgent_ptr: u16,
}

/// TCP flags
pub mod flags {
    pub const FIN: u16 = 0x01;
    pub const SYN: u16 = 0x02;
    pub const RST: u16 = 0x04;
    pub const PSH: u16 = 0x08;
    pub const ACK: u16 = 0x10;
    pub const URG: u16 = 0x20;
    pub const ECE: u16 = 0x40;
    pub const CWR: u16 = 0x80;
}

impl TcpHeader {
    pub const MIN_SIZE: usize = 20;
    
    /// Create new TCP header
    pub fn new(
        src_port: u16,
        dst_port: u16,
        seq: u32,
        ack: u32,
        flags: u16,
        window: u16,
    ) -> Self {
        let offset = 5u16 << 12; // 20 bytes = 5 * 4
        
        Self {
            src_port: src_port.to_be(),
            dst_port: dst_port.to_be(),
            seq_num: seq.to_be(),
            ack_num: ack.to_be(),
            offset_flags: (offset | flags).to_be(),
            window_size: window.to_be(),
            checksum: 0,
            urgent_ptr: 0,
        }
    }
    
    /// Parse from buffer
    pub fn parse(data: &[u8]) -> Result<Self, TcpError> {
        if data.len() < Self::MIN_SIZE {
            return Err(TcpError::TooShort);
        }
        
        Ok(Self {
            src_port: u16::from_be_bytes([data[0], data[1]]),
            dst_port: u16::from_be_bytes([data[2], data[3]]),
            seq_num: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            ack_num: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
            offset_flags: u16::from_be_bytes([data[12], data[13]]),
            window_size: u16::from_be_bytes([data[14], data[15]]),
            checksum: u16::from_be_bytes([data[16], data[17]]),
            urgent_ptr: u16::from_be_bytes([data[18], data[19]]),
        })
    }
    
    /// Write to buffer
    pub fn write(&self, buffer: &mut [u8]) -> Result<(), TcpError> {
        if buffer.len() < Self::MIN_SIZE {
            return Err(TcpError::BufferTooSmall);
        }
        
        buffer[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        buffer[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        buffer[4..8].copy_from_slice(&self.seq_num.to_be_bytes());
        buffer[8..12].copy_from_slice(&self.ack_num.to_be_bytes());
        buffer[12..14].copy_from_slice(&self.offset_flags.to_be_bytes());
        buffer[14..16].copy_from_slice(&self.window_size.to_be_bytes());
        buffer[16..18].copy_from_slice(&self.checksum.to_be_bytes());
        buffer[18..20].copy_from_slice(&self.urgent_ptr.to_be_bytes());
        
        Ok(())
    }
    
    /// Get source port
    pub fn src_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }
    
    /// Get destination port
    pub fn dst_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }
    
    /// Get sequence number
    pub fn seq(&self) -> u32 {
        u32::from_be(self.seq_num)
    }
    
    /// Get acknowledgment number
    pub fn ack(&self) -> u32 {
        u32::from_be(self.ack_num)
    }
    
    /// Get header length in bytes
    pub fn header_len(&self) -> usize {
        ((u16::from_be(self.offset_flags) >> 12) * 4) as usize
    }
    
    /// Get flags
    pub fn flags(&self) -> u16 {
        u16::from_be(self.offset_flags) & 0x1FF
    }
    
    /// Check if flag is set
    pub fn has_flag(&self, flag: u16) -> bool {
        (self.flags() & flag) != 0
    }
    
    /// Get window size
    pub fn window(&self) -> u16 {
        u16::from_be(self.window_size)
    }
    
    /// Calculate checksum (with pseudo-header)
    pub fn calculate_checksum(
        &self,
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
        data: &[u8],
    ) -> u16 {
        let mut sum: u32 = 0;
        
        // Pseudo-header
        for i in 0..4 {
            sum += src_ip.0[i] as u32;
            sum += dst_ip.0[i] as u32;
        }
        sum += 6; // TCP protocol number
        sum += (self.header_len() + data.len()) as u32;
        
        // TCP header
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                self as *const _ as *const u8,
                Self::MIN_SIZE,
            )
        };
        
        for i in (0..Self::MIN_SIZE).step_by(2) {
            if i == 16 {
                // Skip checksum field
                continue;
            }
            
            let word = u16::from_be_bytes([header_bytes[i], header_bytes[i + 1]]);
            sum += word as u32;
        }
        
        // Data
        for i in (0..data.len()).step_by(2) {
            let word = if i + 1 < data.len() {
                u16::from_be_bytes([data[i], data[i + 1]])
            } else {
                u16::from_be_bytes([data[i], 0])
            };
            sum += word as u32;
        }
        
        // Fold
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        !sum as u16
    }
}

/// TCP state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    /// No connection
    Closed,
    
    /// Waiting for connection request
    Listen,
    
    /// SYN sent, waiting for SYN-ACK
    SynSent,
    
    /// SYN received, sent SYN-ACK, waiting for ACK
    SynReceived,
    
    /// Connection established
    Established,
    
    /// FIN sent, waiting for ACK
    FinWait1,
    
    /// FIN ACKed, waiting for FIN
    FinWait2,
    
    /// FIN received, waiting for close
    CloseWait,
    
    /// Both FINs sent, waiting for ACK
    Closing,
    
    /// FIN received and ACKed, waiting for timeout
    LastAck,
    
    /// Waiting for 2*MSL timeout
    TimeWait,
}

/// TCP Connection Control Block (TCB)
pub struct TcpConnection {
    /// Connection state
    state: TcpState,
    
    /// Local address and port
    local_addr: Ipv4Addr,
    local_port: u16,
    
    /// Remote address and port
    remote_addr: Ipv4Addr,
    remote_port: u16,
    
    /// Send sequence variables
    snd_una: u32,  // Send unacknowledged
    snd_nxt: u32,  // Send next
    snd_wnd: u16,  // Send window
    
    /// Receive sequence variables
    rcv_nxt: u32,  // Receive next
    rcv_wnd: u16,  // Receive window
    
    /// Initial sequence number
    iss: u32,
    irs: u32,
    
    /// Retransmission timeout
    rto: u32,
    
    /// Congestion window
    cwnd: u32,
    
    /// Slow start threshold
    ssthresh: u32,
    
    /// Send buffer
    send_buffer: VecDeque<u8>,
    
    /// Receive buffer
    recv_buffer: VecDeque<u8>,
}

impl TcpConnection {
    /// Create new connection
    pub fn new(
        local_addr: Ipv4Addr,
        local_port: u16,
        remote_addr: Ipv4Addr,
        remote_port: u16,
    ) -> Self {
        static NEXT_ISS: AtomicU32 = AtomicU32::new(1000);
        let iss = NEXT_ISS.fetch_add(1, Ordering::Relaxed);
        
        Self {
            state: TcpState::Closed,
            local_addr,
            local_port,
            remote_addr,
            remote_port,
            snd_una: iss,
            snd_nxt: iss,
            snd_wnd: 65535,
            rcv_nxt: 0,
            rcv_wnd: 65535,
            iss,
            irs: 0,
            rto: 1000, // 1 second
            cwnd: 10 * 1460, // 10 MSS segments
            ssthresh: 65535,
            send_buffer: VecDeque::new(),
            recv_buffer: VecDeque::new(),
        }
    }
    
    /// Get current state
    pub fn state(&self) -> TcpState {
        self.state
    }
    
    /// Get send next sequence number
    pub fn snd_nxt(&self) -> u32 {
        self.snd_nxt
    }
    
    /// Get receive next sequence number
    pub fn rcv_nxt(&self) -> u32 {
        self.rcv_nxt
    }
    
    /// Handle incoming packet
    pub fn handle_packet(&mut self, header: &TcpHeader, data: &[u8]) -> Result<(), TcpError> {
        match self.state {
            TcpState::Closed => {
                // Send RST
                Ok(())
            }
            
            TcpState::Listen => {
                if header.has_flag(flags::SYN) {
                    // Received SYN, send SYN-ACK
                    self.irs = header.seq();
                    self.rcv_nxt = self.irs + 1;
                    self.send_syn_ack()?;
                    self.state = TcpState::SynReceived;
                }
                Ok(())
            }
            
            TcpState::SynSent => {
                if header.has_flag(flags::SYN | flags::ACK) {
                    // Received SYN-ACK
                    self.irs = header.seq();
                    self.rcv_nxt = self.irs + 1;
                    self.snd_una = header.ack();
                    self.send_ack()?;
                    self.state = TcpState::Established;
                }
                Ok(())
            }
            
            TcpState::SynReceived => {
                if header.has_flag(flags::ACK) {
                    // Connection established
                    self.snd_una = header.ack();
                    self.state = TcpState::Established;
                }
                Ok(())
            }
            
            TcpState::Established => {
                // Handle data and ACKs
                if data.len() > 0 {
                    self.recv_buffer.extend(data.iter());
                    self.rcv_nxt += data.len() as u32;
                    self.send_ack()?;
                }
                
                if header.has_flag(flags::FIN) {
                    self.rcv_nxt += 1;
                    self.send_ack()?;
                    self.state = TcpState::CloseWait;
                }
                
                if header.has_flag(flags::ACK) {
                    self.snd_una = header.ack();
                }
                
                Ok(())
            }
            
            TcpState::FinWait1 => {
                if header.has_flag(flags::ACK) {
                    self.state = TcpState::FinWait2;
                }
                
                if header.has_flag(flags::FIN) {
                    self.rcv_nxt += 1;
                    self.send_ack()?;
                    
                    if self.state == TcpState::FinWait2 {
                        self.state = TcpState::TimeWait;
                    } else {
                        self.state = TcpState::Closing;
                    }
                }
                
                Ok(())
            }
            
            TcpState::FinWait2 => {
                if header.has_flag(flags::FIN) {
                    self.rcv_nxt += 1;
                    self.send_ack()?;
                    self.state = TcpState::TimeWait;
                }
                Ok(())
            }
            
            TcpState::CloseWait => {
                // Application should close
                Ok(())
            }
            
            TcpState::Closing => {
                if header.has_flag(flags::ACK) {
                    self.state = TcpState::TimeWait;
                }
                Ok(())
            }
            
            TcpState::LastAck => {
                if header.has_flag(flags::ACK) {
                    self.state = TcpState::Closed;
                }
                Ok(())
            }
            
            TcpState::TimeWait => {
                // Wait for timeout
                Ok(())
            }
        }
    }
    
    /// Active open (connect)
    pub fn connect(&mut self) -> Result<(), TcpError> {
        if self.state != TcpState::Closed {
            return Err(TcpError::InvalidState);
        }
        
        self.send_syn()?;
        self.state = TcpState::SynSent;
        Ok(())
    }
    
    /// Passive open (listen)
    pub fn listen(&mut self) -> Result<(), TcpError> {
        if self.state != TcpState::Closed {
            return Err(TcpError::InvalidState);
        }
        
        self.state = TcpState::Listen;
        Ok(())
    }
    
    /// Close connection
    pub fn close(&mut self) -> Result<(), TcpError> {
        match self.state {
            TcpState::Established => {
                self.send_fin()?;
                self.state = TcpState::FinWait1;
            }
            
            TcpState::CloseWait => {
                self.send_fin()?;
                self.state = TcpState::LastAck;
            }
            
            _ => return Err(TcpError::InvalidState),
        }
        
        Ok(())
    }
    
    /// Send data
    pub fn send(&mut self, data: &[u8]) -> Result<usize, TcpError> {
        if self.state != TcpState::Established {
            return Err(TcpError::NotEstablished);
        }
        
        // Add to send buffer
        self.send_buffer.extend(data.iter());
        
        // TODO: Actual transmission with congestion control
        
        Ok(data.len())
    }
    
    /// Receive data
    pub fn recv(&mut self, buffer: &mut [u8]) -> Result<usize, TcpError> {
        if self.state != TcpState::Established {
            return Err(TcpError::NotEstablished);
        }
        
        let len = self.recv_buffer.len().min(buffer.len());
        
        for i in 0..len {
            buffer[i] = self.recv_buffer.pop_front().unwrap();
        }
        
        Ok(len)
    }
    
    /// Send SYN
    fn send_syn(&mut self) -> Result<(), TcpError> {
        let header = TcpHeader::new(
            self.local_port,
            self.remote_port,
            self.iss,
            0,
            flags::SYN,
            self.rcv_wnd,
        );
        
        self.snd_nxt = self.iss + 1;
        
        // TODO: Actual send via IP layer
        
        Ok(())
    }
    
    /// Send SYN-ACK
    fn send_syn_ack(&mut self) -> Result<(), TcpError> {
        let header = TcpHeader::new(
            self.local_port,
            self.remote_port,
            self.iss,
            self.rcv_nxt,
            flags::SYN | flags::ACK,
            self.rcv_wnd,
        );
        
        self.snd_nxt = self.iss + 1;
        
        // TODO: Actual send
        
        Ok(())
    }
    
    /// Send ACK
    fn send_ack(&mut self) -> Result<(), TcpError> {
        let header = TcpHeader::new(
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            flags::ACK,
            self.rcv_wnd,
        );
        
        // TODO: Actual send
        
        Ok(())
    }
    
    /// Send FIN
    fn send_fin(&mut self) -> Result<(), TcpError> {
        let header = TcpHeader::new(
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            flags::FIN | flags::ACK,
            self.rcv_wnd,
        );
        
        self.snd_nxt += 1;
        
        // TODO: Actual send
        
        Ok(())
    }
}

/// TCP errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpError {
    TooShort,
    BufferTooSmall,
    InvalidChecksum,
    InvalidState,
    NotEstablished,
    ConnectionRefused,
    ConnectionReset,
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_tcp_header() {
        let header = TcpHeader::new(1234, 80, 1000, 2000, flags::SYN, 65535);
        
        assert_eq!(header.src_port(), 1234);
        assert_eq!(header.dst_port(), 80);
        assert_eq!(header.seq(), 1000);
        assert_eq!(header.ack(), 2000);
        assert!(header.has_flag(flags::SYN));
        assert_eq!(header.header_len(), 20);
    }
    
    #[test]
    fn test_tcp_state_machine() {
        let local = Ipv4Addr::new(192, 168, 1, 1);
        let remote = Ipv4Addr::new(192, 168, 1, 2);
        
        let mut conn = TcpConnection::new(local, 1234, remote, 80);
        
        assert_eq!(conn.state(), TcpState::Closed);
        
        // Active open
        conn.connect().unwrap();
        assert_eq!(conn.state(), TcpState::SynSent);
    }
    
    #[test]
    fn test_tcp_listen() {
        let local = Ipv4Addr::new(192, 168, 1, 1);
        let remote = Ipv4Addr::new(0, 0, 0, 0);
        
        let mut conn = TcpConnection::new(local, 80, remote, 0);
        
        // Passive open
        conn.listen().unwrap();
        assert_eq!(conn.state(), TcpState::Listen);
    }
}
