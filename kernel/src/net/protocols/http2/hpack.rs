//! HPACK - Header Compression for HTTP/2
//!
//! Implements HPACK (RFC 7541) for efficient header compression.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

/// Static table (RFC 7541 Appendix A)
const STATIC_TABLE: &[(&str, &str)] = &[
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

/// HPACK encoder/decoder
pub struct HpackCodec {
    dynamic_table: Vec<(String, String)>,
    dynamic_table_size: usize,
    max_dynamic_table_size: usize,
}

impl HpackCodec {
    pub fn new(max_size: usize) -> Self {
        Self {
            dynamic_table: Vec::new(),
            dynamic_table_size: 0,
            max_dynamic_table_size: max_size,
        }
    }
    
    /// Encode headers to HPACK format
    pub fn encode(&mut self, headers: &[(String, String)]) -> Vec<u8> {
        let mut encoded = Vec::new();
        
        for (name, value) in headers {
            // Try to find in static table
            if let Some(index) = self.find_in_static_table(name, value) {
                // Indexed header field
                encoded.push(0x80 | index as u8);
            } else if let Some(index) = self.find_name_in_static_table(name) {
                // Literal with incremental indexing (indexed name)
                encoded.push(0x40 | index as u8);
                self.encode_string(value, &mut encoded);
                self.add_to_dynamic_table(name.clone(), value.clone());
            } else {
                // Literal with incremental indexing (new name)
                encoded.push(0x40);
                self.encode_string(name, &mut encoded);
                self.encode_string(value, &mut encoded);
                self.add_to_dynamic_table(name.clone(), value.clone());
            }
        }
        
        encoded
    }
    
    /// Decode HPACK format to headers
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<(String, String)>, HpackError> {
        let mut headers = Vec::new();
        let mut pos = 0;
        
        while pos < data.len() {
            let byte = data[pos];
            
            if byte & 0x80 != 0 {
                // Indexed header field
                let index = (byte & 0x7F) as usize;
                let (name, value) = self.get_indexed(index)?;
                headers.push((name, value));
                pos += 1;
            } else if byte & 0x40 != 0 {
                // Literal with incremental indexing
                let name_index = (byte & 0x3F) as usize;
                pos += 1;
                
                let name = if name_index == 0 {
                    let (n, new_pos) = self.decode_string(data, pos)?;
                    pos = new_pos;
                    n
                } else {
                    let (n, _) = self.get_indexed(name_index)?;
                    n
                };
                
                let (value, new_pos) = self.decode_string(data, pos)?;
                pos = new_pos;
                
                headers.push((name.clone(), value.clone()));
                self.add_to_dynamic_table(name, value);
            } else {
                // Literal without indexing or never indexed
                pos += 1;
                let (name, new_pos) = self.decode_string(data, pos)?;
                pos = new_pos;
                let (value, new_pos) = self.decode_string(data, pos)?;
                pos = new_pos;
                headers.push((name, value));
            }
        }
        
        Ok(headers)
    }
    
    fn find_in_static_table(&self, name: &str, value: &str) -> Option<usize> {
        STATIC_TABLE.iter()
            .position(|(n, v)| *n == name && *v == value)
            .map(|i| i + 1) // 1-indexed
    }
    
    fn find_name_in_static_table(&self, name: &str) -> Option<usize> {
        STATIC_TABLE.iter()
            .position(|(n, _)| *n == name)
            .map(|i| i + 1)
    }
    
    fn get_indexed(&self, index: usize) -> Result<(String, String), HpackError> {
        if index == 0 {
            return Err(HpackError::InvalidIndex);
        }
        
        if index <= STATIC_TABLE.len() {
            let (name, value) = STATIC_TABLE[index - 1];
            Ok((name.to_string(), value.to_string()))
        } else {
            let dynamic_index = index - STATIC_TABLE.len() - 1;
            self.dynamic_table.get(dynamic_index)
                .cloned()
                .ok_or(HpackError::InvalidIndex)
        }
    }
    
    fn add_to_dynamic_table(&mut self, name: String, value: String) {
        let entry_size = 32 + name.len() + value.len();
        
        // Evict entries if necessary
        while self.dynamic_table_size + entry_size > self.max_dynamic_table_size
            && !self.dynamic_table.is_empty()
        {
            if let Some((n, v)) = self.dynamic_table.pop() {
                self.dynamic_table_size -= 32 + n.len() + v.len();
            }
        }
        
        if entry_size <= self.max_dynamic_table_size {
            self.dynamic_table.insert(0, (name, value));
            self.dynamic_table_size += entry_size;
        }
    }
    
    fn encode_string(&self, s: &str, output: &mut Vec<u8>) {
        // Length (no Huffman encoding for now)
        let len = s.len();
        if len < 127 {
            output.push(len as u8);
        } else {
            output.push(0x7F);
            self.encode_integer(len - 127, 7, output);
        }
        output.extend_from_slice(s.as_bytes());
    }
    
    fn decode_string(&self, data: &[u8], pos: usize) -> Result<(String, usize), HpackError> {
        if pos >= data.len() {
            return Err(HpackError::Truncated);
        }
        
        let huffman = data[pos] & 0x80 != 0;
        let mut len = (data[pos] & 0x7F) as usize;
        let mut pos = pos + 1;
        
        if len == 127 {
            let (decoded_len, new_pos) = self.decode_integer(data, pos, 7)?;
            len += decoded_len;
            pos = new_pos;
        }
        
        if pos + len > data.len() {
            return Err(HpackError::Truncated);
        }
        
        let string_data = &data[pos..pos + len];
        let s = if huffman {
            // TODO: Huffman decoding
            String::from_utf8_lossy(string_data).to_string()
        } else {
            String::from_utf8_lossy(string_data).to_string()
        };
        
        Ok((s, pos + len))
    }
    
    fn encode_integer(&self, mut value: usize, prefix_bits: u8, output: &mut Vec<u8>) {
        while value >= 128 {
            output.push(((value % 128) + 128) as u8);
            value /= 128;
        }
        output.push(value as u8);
    }
    
    fn decode_integer(&self, data: &[u8], mut pos: usize, prefix_bits: u8) -> Result<(usize, usize), HpackError> {
        let mut value = 0usize;
        let mut shift = 0;
        
        loop {
            if pos >= data.len() {
                return Err(HpackError::Truncated);
            }
            
            let byte = data[pos];
            pos += 1;
            
            value += ((byte & 0x7F) as usize) << shift;
            
            if byte & 0x80 == 0 {
                break;
            }
            
            shift += 7;
        }
        
        Ok((value, pos))
    }
}

/// HPACK errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpackError {
    InvalidIndex,
    Truncated,
    InvalidEncoding,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_encode_indexed() {
        let mut codec = HpackCodec::new(4096);
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
        ];
        
        let encoded = codec.encode(&headers);
        assert_eq!(encoded[0], 0x82); // Index 2 in static table
    }
    
    #[test]
    fn test_roundtrip() {
        let mut encoder = HpackCodec::new(4096);
        let mut decoder = HpackCodec::new(4096);
        
        let headers = vec![
            (":method".to_string(), "POST".to_string()),
            (":path".to_string(), "/api/v1".to_string()),
        ];
        
        let encoded = encoder.encode(&headers);
        let decoded = decoder.decode(&encoded).unwrap();
        
        assert_eq!(decoded.len(), 2);
    }
}
