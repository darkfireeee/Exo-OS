//! TPM 2.0 Command Builder
//!
//! Constructs binary command packets for TPM 2.0.
//! Implements TCG TPM 2.0 specification Part 3 (Commands).

use alloc::vec::Vec;

/// TPM 2.0 Command Codes
#[repr(u32)]
#[allow(dead_code)]
pub enum TpmCommandCode {
    GetRandom = 0x0000017B,
    PcrExtend = 0x00000182,
    PcrRead = 0x0000017E,
    Create = 0x00000153,
    Load = 0x00000157,
    Quote = 0x00000158,
    Unseal = 0x0000015E,
    NvRead = 0x0000014E,
    NvWrite = 0x00000137,
    Startup = 0x00000144,
    GetCapability = 0x0000017A,
}

/// TPM 2.0 Structure Tags
#[repr(u16)]
pub enum TpmStructureTag {
    /// No sessions, no authorization
    NoSessions = 0x8001,
    /// With sessions/authorization
    Sessions = 0x8002,
}

/// TPM Command Builder
pub struct TpmCommand {
    buffer: Vec<u8>,
}

impl TpmCommand {
    /// Create a new command with tag and command code
    pub fn new(tag: TpmStructureTag, command_code: TpmCommandCode) -> Self {
        let mut buffer = Vec::new();

        // Tag (2 bytes)
        buffer.extend_from_slice(&(tag as u16).to_be_bytes());

        // Size placeholder (4 bytes) - will be updated in finalize()
        buffer.extend_from_slice(&[0, 0, 0, 0]);

        // Command code (4 bytes)
        buffer.extend_from_slice(&(command_code as u32).to_be_bytes());

        Self { buffer }
    }

    /// Add a u8 value
    pub fn add_u8(&mut self, value: u8) -> &mut Self {
        self.buffer.push(value);
        self
    }

    /// Add a u16 value (big-endian)
    pub fn add_u16(&mut self, value: u16) -> &mut Self {
        self.buffer.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// Add a u32 value (big-endian)
    pub fn add_u32(&mut self, value: u32) -> &mut Self {
        self.buffer.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// Add a u64 value (big-endian)
    pub fn add_u64(&mut self, value: u64) -> &mut Self {
        self.buffer.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// Add a byte slice
    pub fn add_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.buffer.extend_from_slice(bytes);
        self
    }

    /// Add a TPM2B structure (2-byte size + data)
    pub fn add_tpm2b(&mut self, data: &[u8]) -> &mut Self {
        self.add_u16(data.len() as u16);
        self.add_bytes(data);
        self
    }

    /// Finalize the command (updates size field)
    pub fn finalize(mut self) -> Vec<u8> {
        let size = self.buffer.len() as u32;
        // Update size field at offset 2
        self.buffer[2..6].copy_from_slice(&size.to_be_bytes());
        self.buffer
    }
}

/// Helper: Build TPM2_GetRandom command
pub fn build_get_random(bytes_requested: u16) -> Vec<u8> {
    TpmCommand::new(TpmStructureTag::NoSessions, TpmCommandCode::GetRandom)
        .add_u16(bytes_requested)
        .finalize()
}

/// Helper: Build TPM2_PCR_Extend command
pub fn build_pcr_extend(pcr_index: u32, hash_alg: u16, digest: &[u8]) -> Vec<u8> {
    let mut cmd = TpmCommand::new(TpmStructureTag::Sessions, TpmCommandCode::PcrExtend);

    // PCR Handle
    cmd.add_u32(pcr_index);

    // Authorization area size (for empty auth)
    cmd.add_u32(9); // Size of minimal auth session

    // Password session
    cmd.add_u32(0x40000009); // TPM_RS_PW
    cmd.add_u16(0); // Nonce size
    cmd.add_u8(0); // Session attributes
    cmd.add_u16(0); // Auth size

    // TPML_DIGEST_VALUES count
    cmd.add_u32(1);

    // TPMT_HA (hash algorithm + digest)
    cmd.add_u16(hash_alg);
    cmd.add_bytes(digest);

    cmd.finalize()
}

/// Helper: Build TPM2_PCR_Read command
pub fn build_pcr_read(pcr_selection: &[u32], hash_alg: u16) -> Vec<u8> {
    let mut cmd = TpmCommand::new(TpmStructureTag::NoSessions, TpmCommandCode::PcrRead);

    // TPML_PCR_SELECTION count
    cmd.add_u32(1);

    // TPMS_PCR_SELECTION
    cmd.add_u16(hash_alg); // Hash algorithm
    cmd.add_u8(3); // Size of select (3 bytes for 24 PCRs)

    // PCR selection bitmap (3 bytes)
    let mut select = [0u8; 3];
    for &pcr in pcr_selection {
        if pcr < 24 {
            select[(pcr / 8) as usize] |= 1 << (pcr % 8);
        }
    }
    cmd.add_bytes(&select);

    cmd.finalize()
}

/// Helper: Build TPM2_Startup command
pub fn build_startup(startup_type: u16) -> Vec<u8> {
    TpmCommand::new(TpmStructureTag::NoSessions, TpmCommandCode::Startup)
        .add_u16(startup_type)
        .finalize()
}
