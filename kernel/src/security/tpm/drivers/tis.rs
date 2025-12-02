//! TPM 2.0 TIS (TPM Interface Specification) Driver
//!
//! Complete implementation of TIS protocol for TPM communication via MMIO.
//! Standard address: 0xFED40000

use crate::security::tpm::TpmError;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

/// Standard TIS MMIO base address
const TIS_BASE_ADDR: usize = 0xFED40000;

/// TIS Register Offsets (Locality 0)
const TIS_ACCESS: usize = 0x00;
const TIS_INT_ENABLE: usize = 0x08;
const TIS_INT_VECTOR: usize = 0x0C;
const TIS_INT_STATUS: usize = 0x10;
const TIS_INTF_CAPABILITY: usize = 0x14;
const TIS_STS: usize = 0x18;
const TIS_BURST_COUNT: usize = 0x19;
const TIS_DATA_FIFO: usize = 0x24;
const TIS_DID_VID: usize = 0xF00;
const TIS_RID: usize = 0xF04;

/// Access Register Bits
const ACCESS_TPM_ESTABLISHMENT: u8 = 1 << 0;
const ACCESS_REQUEST_USE: u8 = 1 << 1;
const ACCESS_PENDING_REQUEST: u8 = 1 << 2;
const ACCESS_SEIZE: u8 = 1 << 3;
const ACCESS_BEEN_SEIZED: u8 = 1 << 4;
const ACCESS_ACTIVE_LOCALITY: u8 = 1 << 5;
const ACCESS_TPM_REG_VALID_STS: u8 = 1 << 7;

/// Status Register Bits
const STS_TPM_GO: u32 = 1 << 5;
const STS_COMMAND_READY: u32 = 1 << 6;
const STS_VALID: u32 = 1 << 7;
const STS_DATA_AVAIL: u32 = 1 << 4;
const STS_DATA_EXPECT: u32 = 1 << 3;

/// Timeout values (in loop iterations)
const TIMEOUT_SHORT: usize = 1000;
const TIMEOUT_MEDIUM: usize = 10000;
const TIMEOUT_LONG: usize = 100000;

pub struct TisDriver {
    base_addr: usize,
}

impl TisDriver {
    /// Create a new TIS driver instance
    ///
    /// # Safety
    /// Caller must ensure the base address is valid and mapped.
    pub const unsafe fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }

    /// Probe for a TPM at the standard address
    pub fn probe() -> Option<Self> {
        let driver = unsafe { Self::new(TIS_BASE_ADDR) };

        // Check DID/VID register
        let did_vid = driver.read_u32(TIS_DID_VID);

        if did_vid == 0xFFFFFFFF || did_vid == 0 {
            return None;
        }

        // Request locality 0
        if !driver.request_locality() {
            return None;
        }

        log::info!("TPM DID/VID: 0x{:08X}", did_vid);

        Some(driver)
    }

    /// Request use of locality 0
    fn request_locality(&self) -> bool {
        self.write_u8(TIS_ACCESS, ACCESS_REQUEST_USE);

        for _ in 0..TIMEOUT_SHORT {
            let access = self.read_u8(TIS_ACCESS);
            if (access & ACCESS_ACTIVE_LOCALITY) != 0 {
                return true;
            }
            core::hint::spin_loop();
        }

        false
    }

    /// Release locality
    fn release_locality(&self) {
        self.write_u8(TIS_ACCESS, ACCESS_ACTIVE_LOCALITY);
    }

    /// Wait for status bit
    fn wait_for_status(&self, bit: u32, timeout: usize) -> Result<(), TpmError> {
        for _ in 0..timeout {
            if (self.read_u32(TIS_STS) & bit) != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(TpmError::CommunicationError)
    }

    /// Get burst count from status register
    fn get_burst_count(&self) -> u16 {
        let sts = self.read_u32(TIS_STS);
        ((sts >> 8) & 0xFFFF) as u16
    }

    /// Read u8 from register
    fn read_u8(&self, offset: usize) -> u8 {
        unsafe { read_volatile((self.base_addr + offset) as *const u8) }
    }

    /// Write u8 to register
    fn write_u8(&self, offset: usize, value: u8) {
        unsafe { write_volatile((self.base_addr + offset) as *mut u8, value) }
    }

    /// Read u32 from register
    fn read_u32(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.base_addr + offset) as *const u32) }
    }

    /// Write u32 to register
    fn write_u32(&self, offset: usize, value: u32) {
        unsafe { write_volatile((self.base_addr + offset) as *mut u32, value) }
    }

    /// Send a command to the TPM
    pub fn send_command(&self, command: &[u8]) -> Result<(), TpmError> {
        // 1. Set commandReady
        self.write_u8(TIS_STS, STS_COMMAND_READY as u8);

        // Wait for commandReady
        self.wait_for_status(STS_COMMAND_READY, TIMEOUT_SHORT)?;

        // 2. Write command bytes
        let mut sent = 0;
        while sent < command.len() {
            // Wait for dataExpect
            self.wait_for_status(STS_DATA_EXPECT, TIMEOUT_SHORT)?;

            // Get burst count
            let burst = self.get_burst_count().max(1) as usize;
            let to_send = (command.len() - sent).min(burst);

            // Write burst
            for i in 0..to_send {
                self.write_u8(TIS_DATA_FIFO, command[sent + i]);
            }

            sent += to_send;
        }

        // 3. Wait for valid status (no more dataExpect)
        for _ in 0..TIMEOUT_SHORT {
            let sts = self.read_u32(TIS_STS);
            if (sts & STS_VALID) != 0 && (sts & STS_DATA_EXPECT) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 4. Execute command
        self.write_u8(TIS_STS, STS_TPM_GO as u8);

        Ok(())
    }

    /// Receive response from TPM
    pub fn receive_response(&self) -> Result<Vec<u8>, TpmError> {
        // 1. Wait for dataAvail
        self.wait_for_status(STS_DATA_AVAIL, TIMEOUT_LONG)?;

        // 2. Read response header to get size
        let mut response = Vec::new();

        // Read first 10 bytes (header)
        for _ in 0..10 {
            self.wait_for_status(STS_DATA_AVAIL, TIMEOUT_SHORT)?;
            response.push(self.read_u8(TIS_DATA_FIFO));
        }

        // Parse size from header (bytes 2-5, big-endian)
        let size =
            u32::from_be_bytes([response[2], response[3], response[4], response[5]]) as usize;

        if size < 10 || size > 4096 {
            return Err(TpmError::CommunicationError);
        }

        // Read remaining bytes
        let remaining = size - 10;
        for _ in 0..remaining {
            // Check if data still available
            let sts = self.read_u32(TIS_STS);
            if (sts & STS_DATA_AVAIL) == 0 {
                break;
            }

            response.push(self.read_u8(TIS_DATA_FIFO));
        }

        // 3. Set commandReady to finish
        self.write_u8(TIS_STS, STS_COMMAND_READY as u8);

        Ok(response)
    }

    /// Execute a complete command/response transaction
    pub fn execute(&self, command: &[u8]) -> Result<Vec<u8>, TpmError> {
        self.send_command(command)?;
        self.receive_response()
    }
}

impl Drop for TisDriver {
    fn drop(&mut self) {
        self.release_locality();
    }
}
