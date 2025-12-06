//! # TCP Options - RFC Support
//! 
//! Support complet des options TCP (RFC 793, 1323, 2018, 7323).

use alloc::vec::Vec;

/// TCP Option kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TcpOptionKind {
    EndOfOptions = 0,
    NoOp = 1,
    Mss = 2,
    WindowScale = 3,
    SackPermitted = 4,
    Sack = 5,
    Timestamp = 8,
}

/// TCP Options
#[derive(Debug, Clone, Default)]
pub struct TcpOptions {
    pub mss: Option<u16>,
    pub window_scale: Option<u8>,
    pub sack_permitted: bool,
    pub sack_blocks: Vec<SackBlock>,
    pub timestamp: Option<(u32, u32)>, // (TSval, TSecr)
}

/// SACK block (RFC 2018)
#[derive(Debug, Clone, Copy)]
pub struct SackBlock {
    pub left_edge: u32,
    pub right_edge: u32,
}

impl TcpOptions {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Parse options depuis bytes
    pub fn parse(data: &[u8]) -> Self {
        let mut options = Self::new();
        let mut i = 0;
        
        while i < data.len() {
            let kind = data[i];
            
            match kind {
                0 => break, // End of options
                1 => i += 1, // NoOp
                2 => { // MSS
                    if i + 3 < data.len() && data[i + 1] == 4 {
                        options.mss = Some(u16::from_be_bytes([data[i + 2], data[i + 3]]));
                        i += 4;
                    } else {
                        break;
                    }
                }
                3 => { // Window Scale
                    if i + 2 < data.len() && data[i + 1] == 3 {
                        options.window_scale = Some(data[i + 2]);
                        i += 3;
                    } else {
                        break;
                    }
                }
                4 => { // SACK Permitted
                    if i + 1 < data.len() && data[i + 1] == 2 {
                        options.sack_permitted = true;
                        i += 2;
                    } else {
                        break;
                    }
                }
                5 => { // SACK
                    if i + 1 < data.len() {
                        let len = data[i + 1] as usize;
                        if i + len <= data.len() && (len - 2) % 8 == 0 {
                            let num_blocks = (len - 2) / 8;
                            for b in 0..num_blocks {
                                let offset = i + 2 + b * 8;
                                if offset + 8 <= data.len() {
                                    let left = u32::from_be_bytes([
                                        data[offset], data[offset + 1],
                                        data[offset + 2], data[offset + 3]
                                    ]);
                                    let right = u32::from_be_bytes([
                                        data[offset + 4], data[offset + 5],
                                        data[offset + 6], data[offset + 7]
                                    ]);
                                    options.sack_blocks.push(SackBlock {
                                        left_edge: left,
                                        right_edge: right,
                                    });
                                }
                            }
                            i += len;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                8 => { // Timestamp
                    if i + 9 < data.len() && data[i + 1] == 10 {
                        let tsval = u32::from_be_bytes([
                            data[i + 2], data[i + 3], data[i + 4], data[i + 5]
                        ]);
                        let tsecr = u32::from_be_bytes([
                            data[i + 6], data[i + 7], data[i + 8], data[i + 9]
                        ]);
                        options.timestamp = Some((tsval, tsecr));
                        i += 10;
                    } else {
                        break;
                    }
                }
                _ => {
                    // Unknown option: skip
                    if i + 1 < data.len() {
                        let len = data[i + 1] as usize;
                        if len >= 2 {
                            i += len;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        
        options
    }
    
    /// Encode options to bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut data = Vec::new();
        
        // MSS
        if let Some(mss) = self.mss {
            data.push(TcpOptionKind::Mss as u8);
            data.push(4);
            data.extend_from_slice(&mss.to_be_bytes());
        }
        
        // Window Scale
        if let Some(wscale) = self.window_scale {
            data.push(TcpOptionKind::WindowScale as u8);
            data.push(3);
            data.push(wscale);
        }
        
        // SACK Permitted
        if self.sack_permitted {
            data.push(TcpOptionKind::SackPermitted as u8);
            data.push(2);
        }
        
        // SACK blocks
        if !self.sack_blocks.is_empty() {
            data.push(TcpOptionKind::Sack as u8);
            data.push(2 + (self.sack_blocks.len() * 8) as u8);
            for block in &self.sack_blocks {
                data.extend_from_slice(&block.left_edge.to_be_bytes());
                data.extend_from_slice(&block.right_edge.to_be_bytes());
            }
        }
        
        // Timestamp
        if let Some((tsval, tsecr)) = self.timestamp {
            data.push(TcpOptionKind::Timestamp as u8);
            data.push(10);
            data.extend_from_slice(&tsval.to_be_bytes());
            data.extend_from_slice(&tsecr.to_be_bytes());
        }
        
        // Pad to 4-byte boundary
        while data.len() % 4 != 0 {
            data.push(TcpOptionKind::NoOp as u8);
        }
        
        data
    }
    
    /// Taille totale des options
    pub fn len(&self) -> usize {
        let mut len = 0;
        
        if self.mss.is_some() { len += 4; }
        if self.window_scale.is_some() { len += 3; }
        if self.sack_permitted { len += 2; }
        if !self.sack_blocks.is_empty() {
            len += 2 + self.sack_blocks.len() * 8;
        }
        if self.timestamp.is_some() { len += 10; }
        
        // Pad to 4-byte boundary
        (len + 3) / 4 * 4
    }
}

/// Builder pour options SYN
pub struct SynOptionsBuilder {
    options: TcpOptions,
}

impl SynOptionsBuilder {
    pub fn new() -> Self {
        Self {
            options: TcpOptions::new(),
        }
    }
    
    pub fn mss(mut self, mss: u16) -> Self {
        self.options.mss = Some(mss);
        self
    }
    
    pub fn window_scale(mut self, scale: u8) -> Self {
        self.options.window_scale = Some(scale);
        self
    }
    
    pub fn sack_permitted(mut self) -> Self {
        self.options.sack_permitted = true;
        self
    }
    
    pub fn timestamp(mut self, tsval: u32) -> Self {
        self.options.timestamp = Some((tsval, 0));
        self
    }
    
    pub fn build(self) -> TcpOptions {
        self.options
    }
}

/// Calcule window scale optimal
pub fn calculate_window_scale(buffer_size: u32) -> u8 {
    if buffer_size <= 65535 {
        return 0;
    }
    
    let mut scale = 0u8;
    let mut size = 65535u32;
    
    while size < buffer_size && scale < 14 {
        scale += 1;
        size <<= 1;
    }
    
    scale
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_mss() {
        let data = [2, 4, 0x05, 0xb4]; // MSS = 1460
        let opts = TcpOptions::parse(&data);
        assert_eq!(opts.mss, Some(1460));
    }
    
    #[test]
    fn test_encode_options() {
        let opts = SynOptionsBuilder::new()
            .mss(1460)
            .window_scale(7)
            .sack_permitted()
            .build();
        
        let encoded = opts.encode();
        assert!(encoded.len() % 4 == 0);
        
        let decoded = TcpOptions::parse(&encoded);
        assert_eq!(decoded.mss, Some(1460));
        assert_eq!(decoded.window_scale, Some(7));
        assert!(decoded.sack_permitted);
    }
    
    #[test]
    fn test_window_scale() {
        assert_eq!(calculate_window_scale(65535), 0);
        assert_eq!(calculate_window_scale(131070), 1);
        assert_eq!(calculate_window_scale(1_000_000), 4);
    }
}
