//! IPC sink for remote logging

use crate::Result;

pub struct IpcSink {
    #[allow(dead_code)]
    channel_id: u32,
}

impl IpcSink {
    pub fn new(channel_id: u32) -> Self {
        Self { channel_id }
    }
    
    pub fn send(&self, _data: &[u8]) -> Result<()> {
        // TODO: Send via IPC channel
        Ok(())
    }
}
