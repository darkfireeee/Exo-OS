//! TPM 2.0 Response Parser
//!
//! Parses binary response packets from TPM 2.0.

use super::TpmError;
use alloc::vec::Vec;

/// TPM Response Codes
#[repr(u32)]
#[allow(dead_code)]
pub enum TpmResponseCode {
    Success = 0x00000000,
    Initialize = 0x00000100,
    Failure = 0x00000101,
    BadTag = 0x0000001E,
}

/// TPM Response Parser
pub struct TpmResponse<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> TpmResponse<'a> {
    /// Create a new response parser
    pub fn new(data: &'a [u8]) -> Result<Self, TpmError> {
        if data.len() < 10 {
            return Err(TpmError::CommunicationError);
        }

        Ok(Self { data, offset: 0 })
    }

    /// Read tag (2 bytes)
    pub fn read_tag(&mut self) -> Result<u16, TpmError> {
        self.read_u16()
    }

    /// Read response size (4 bytes)
    pub fn read_size(&mut self) -> Result<u32, TpmError> {
        self.read_u32()
    }

    /// Read response code (4 bytes)
    pub fn read_response_code(&mut self) -> Result<u32, TpmError> {
        self.read_u32()
    }

    /// Validate response header (tag, size, code)
    pub fn validate_header(&mut self) -> Result<(), TpmError> {
        let _tag = self.read_tag()?;
        let size = self.read_size()?;
        let code = self.read_response_code()?;

        if size as usize != self.data.len() {
            return Err(TpmError::CommunicationError);
        }

        if code != TpmResponseCode::Success as u32 {
            return Err(TpmError::CommandFailed(code));
        }

        Ok(())
    }

    /// Read u8
    pub fn read_u8(&mut self) -> Result<u8, TpmError> {
        if self.offset >= self.data.len() {
            return Err(TpmError::CommunicationError);
        }
        let value = self.data[self.offset];
        self.offset += 1;
        Ok(value)
    }

    /// Read u16 (big-endian)
    pub fn read_u16(&mut self) -> Result<u16, TpmError> {
        if self.offset + 2 > self.data.len() {
            return Err(TpmError::CommunicationError);
        }
        let value = u16::from_be_bytes([self.data[self.offset], self.data[self.offset + 1]]);
        self.offset += 2;
        Ok(value)
    }

    /// Read u32 (big-endian)
    pub fn read_u32(&mut self) -> Result<u32, TpmError> {
        if self.offset + 4 > self.data.len() {
            return Err(TpmError::CommunicationError);
        }
        let value = u32::from_be_bytes([
            self.data[self.offset],
            self.data[self.offset + 1],
            self.data[self.offset + 2],
            self.data[self.offset + 3],
        ]);
        self.offset += 4;
        Ok(value)
    }

    /// Read bytes
    pub fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], TpmError> {
        if self.offset + count > self.data.len() {
            return Err(TpmError::CommunicationError);
        }
        let slice = &self.data[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    /// Read TPM2B structure (2-byte size + data)
    pub fn read_tpm2b(&mut self) -> Result<Vec<u8>, TpmError> {
        let size = self.read_u16()? as usize;
        let data = self.read_bytes(size)?;
        Ok(data.to_vec())
    }

    /// Get remaining bytes
    pub fn remaining(&self) -> &'a [u8] {
        &self.data[self.offset..]
    }
}

/// Parse TPM2_GetRandom response
pub fn parse_get_random_response(data: &[u8]) -> Result<Vec<u8>, TpmError> {
    let mut resp = TpmResponse::new(data)?;
    resp.validate_header()?;
    resp.read_tpm2b()
}

/// Parse TPM2_PCR_Read response  
pub fn parse_pcr_read_response(data: &[u8]) -> Result<Vec<Vec<u8>>, TpmError> {
    let mut resp = TpmResponse::new(data)?;
    resp.validate_header()?;

    // Skip update counter
    resp.read_u32()?;

    // Read PCR selection
    let sel_count = resp.read_u32()?;
    for _ in 0..sel_count {
        resp.read_u16()?; // hash alg
        resp.read_u8()?; // size of select
        resp.read_bytes(3)?; // select
    }

    // Read digest values
    let digest_count = resp.read_u32()?;
    let mut digests = Vec::new();

    for _ in 0..digest_count {
        let digest = resp.read_tpm2b()?;
        digests.push(digest);
    }

    Ok(digests)
}
