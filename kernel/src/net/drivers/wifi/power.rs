//! # Power Management
//! 
//! Power save modes:
//! - Legacy PS-Poll
//! - U-APSD (Unscheduled APSD)
//! - DTIM (Delivery Traffic Indication Map)
//! - TWT (Target Wake Time) for WiFi 6

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Power save modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSaveMode {
    Off,           // Always on
    Static,        // Legacy PS-Poll
    Dynamic,       // U-APSD
    Max,           // Maximum power save
    Twt,           // Target Wake Time (WiFi 6)
}

/// Power management context
pub struct PowerManager {
    mode: PowerSaveMode,
    enabled: AtomicBool,
    
    // PS-Poll
    listen_interval: u16,
    
    // U-APSD
    uapsd_enabled: AtomicBool,
    delivery_enabled: [bool; 4],  // Per AC (Access Category)
    trigger_enabled: [bool; 4],
    
    // DTIM
    dtim_period: u8,
    dtim_count: AtomicU32,
    
    // TWT (Target Wake Time)
    twt_enabled: AtomicBool,
    twt_wake_interval_us: u64,
    twt_wake_duration_us: u32,
}

impl PowerManager {
    pub fn new() -> Self {
        Self {
            mode: PowerSaveMode::Dynamic,
            enabled: AtomicBool::new(false),
            listen_interval: 10,
            uapsd_enabled: AtomicBool::new(false),
            delivery_enabled: [false; 4],
            trigger_enabled: [false; 4],
            dtim_period: 3,
            dtim_count: AtomicU32::new(0),
            twt_enabled: AtomicBool::new(false),
            twt_wake_interval_us: 100_000,  // 100ms
            twt_wake_duration_us: 10_000,   // 10ms
        }
    }
    
    /// Enable power save
    pub fn enable(&self, mode: PowerSaveMode) -> Result<(), super::WiFiError> {
        match mode {
            PowerSaveMode::Off => {
                self.enabled.store(false, Ordering::SeqCst);
            },
            PowerSaveMode::Static => {
                self.enable_ps_poll()?;
            },
            PowerSaveMode::Dynamic => {
                self.enable_uapsd()?;
            },
            PowerSaveMode::Max => {
                self.enable_max_power_save()?;
            },
            PowerSaveMode::Twt => {
                self.enable_twt()?;
            },
        }
        
        Ok(())
    }
    
    /// Disable power save
    pub fn disable(&self) -> Result<(), super::WiFiError> {
        self.enabled.store(false, Ordering::SeqCst);
        self.uapsd_enabled.store(false, Ordering::SeqCst);
        self.twt_enabled.store(false, Ordering::SeqCst);
        Ok(())
    }
    
    /// Enable PS-Poll (legacy)
    fn enable_ps_poll(&self) -> Result<(), super::WiFiError> {
        self.enabled.store(true, Ordering::SeqCst);
        // Configure hardware for PS-Poll
        Ok(())
    }
    
    /// Enable U-APSD (Unscheduled Automatic Power Save Delivery)
    fn enable_uapsd(&self) -> Result<(), super::WiFiError> {
        self.enabled.store(true, Ordering::SeqCst);
        self.uapsd_enabled.store(true, Ordering::SeqCst);
        
        // Enable all ACs for delivery and trigger
        // AC_BE, AC_BK, AC_VI, AC_VO
        for ac in 0..4 {
            self.set_uapsd_ac(ac, true, true)?;
        }
        
        Ok(())
    }
    
    /// Configure U-APSD per Access Category
    fn set_uapsd_ac(
        &self,
        ac: usize,
        delivery: bool,
        trigger: bool,
    ) -> Result<(), super::WiFiError> {
        if ac >= 4 {
            return Err(super::WiFiError::InvalidFrame);
        }
        
        // In real implementation, would configure hardware registers
        Ok(())
    }
    
    /// Enable maximum power save
    fn enable_max_power_save(&self) -> Result<(), super::WiFiError> {
        self.enabled.store(true, Ordering::SeqCst);
        self.uapsd_enabled.store(true, Ordering::SeqCst);
        
        // Set long listen interval
        // Enable all power-saving features
        
        Ok(())
    }
    
    /// Enable Target Wake Time (WiFi 6)
    fn enable_twt(&self) -> Result<(), super::WiFiError> {
        self.enabled.store(true, Ordering::SeqCst);
        self.twt_enabled.store(true, Ordering::SeqCst);
        
        // Negotiate TWT with AP
        self.negotiate_twt()?;
        
        Ok(())
    }
    
    /// Negotiate TWT parameters with AP
    fn negotiate_twt(&self) -> Result<(), super::WiFiError> {
        // Build TWT Setup frame
        let setup_frame = self.build_twt_setup()?;
        
        // Send to AP
        // (Would go through MAC layer)
        
        Ok(())
    }
    
    /// Build TWT Setup frame
    fn build_twt_setup(&self) -> Result<alloc::vec::Vec<u8>, super::WiFiError> {
        let mut frame = alloc::vec::Vec::new();
        
        // TWT Element
        frame.push(255);  // Element ID: Extension
        frame.push(18);   // Length
        frame.push(216);  // Extension ID: TWT
        
        // TWT Control field
        let control = 0x01;  // Request TWT
        frame.push(control);
        
        // TWT Parameter Info
        // Request Type, Target Wake Time, etc.
        frame.extend_from_slice(&self.twt_wake_interval_us.to_le_bytes());
        frame.extend_from_slice(&self.twt_wake_duration_us.to_le_bytes());
        
        Ok(frame)
    }
    
    /// Build PS-Poll frame
    pub fn build_ps_poll(
        &self,
        aid: u16,
        bssid: [u8; 6],
        sta_addr: [u8; 6],
    ) -> Result<alloc::vec::Vec<u8>, super::WiFiError> {
        let mut frame = alloc::vec::Vec::new();
        
        // Frame Control: PS-Poll
        frame.extend_from_slice(&[0xa4, 0x00]);
        
        // AID (with bits 14-15 set to 1 for PS-Poll)
        let aid_field = aid | 0xC000;
        frame.extend_from_slice(&aid_field.to_le_bytes());
        
        // BSSID
        frame.extend_from_slice(&bssid);
        
        // TA (Transmitter Address = STA address)
        frame.extend_from_slice(&sta_addr);
        
        Ok(frame)
    }
    
    /// Check if we should wake up for DTIM beacon
    pub fn should_wake_for_dtim(&self) -> bool {
        if !self.enabled.load(Ordering::SeqCst) {
            return true;  // Always wake if power save disabled
        }
        
        let count = self.dtim_count.load(Ordering::SeqCst);
        count == 0
    }
    
    /// Update DTIM count
    pub fn update_dtim_count(&self, count: u8) {
        self.dtim_count.store(count as u32, Ordering::SeqCst);
    }
    
    /// Check if in power save mode
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    
    /// Get current mode
    pub fn get_mode(&self) -> PowerSaveMode {
        self.mode
    }
    
    /// Set listen interval
    pub fn set_listen_interval(&mut self, interval: u16) {
        self.listen_interval = interval;
    }
    
    /// Get listen interval
    pub fn get_listen_interval(&self) -> u16 {
        self.listen_interval
    }
}

/// Access Categories for QoS
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum AccessCategory {
    Background = 0,  // AC_BK
    BestEffort = 1,  // AC_BE
    Video = 2,       // AC_VI
    Voice = 3,       // AC_VO
}

impl AccessCategory {
    /// Get EDCA parameters for AC
    pub fn edca_params(&self) -> EdcaParams {
        match self {
            AccessCategory::Background => EdcaParams {
                aifsn: 7,
                cw_min: 15,
                cw_max: 1023,
                txop_limit: 0,
            },
            AccessCategory::BestEffort => EdcaParams {
                aifsn: 3,
                cw_min: 15,
                cw_max: 1023,
                txop_limit: 0,
            },
            AccessCategory::Video => EdcaParams {
                aifsn: 2,
                cw_min: 7,
                cw_max: 15,
                txop_limit: 3008,  // 3.008ms
            },
            AccessCategory::Voice => EdcaParams {
                aifsn: 2,
                cw_min: 3,
                cw_max: 7,
                txop_limit: 1504,  // 1.504ms
            },
        }
    }
}

/// EDCA (Enhanced Distributed Channel Access) parameters
#[derive(Debug, Clone, Copy)]
pub struct EdcaParams {
    pub aifsn: u8,       // Arbitration IFS Number
    pub cw_min: u16,     // Minimum Contention Window
    pub cw_max: u16,     // Maximum Contention Window
    pub txop_limit: u16, // TXOP Limit (μs)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_power_manager() {
        let pm = PowerManager::new();
        assert!(!pm.is_enabled());
        assert_eq!(pm.get_mode(), PowerSaveMode::Dynamic);
    }
    
    #[test]
    fn test_access_category() {
        let ac = AccessCategory::Voice;
        let params = ac.edca_params();
        assert_eq!(params.aifsn, 2);
        assert_eq!(params.cw_min, 3);
    }
}
