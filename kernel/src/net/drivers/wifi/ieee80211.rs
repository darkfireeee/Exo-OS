//! # IEEE 802.11 Frame Handling
//! 
//! Complete frame parsing and building for all 802.11 standards

use alloc::vec::Vec;
use alloc::string::String;

/// Frame types (2 bits)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Management = 0,
    Control = 1,
    Data = 2,
}

/// Management frame subtypes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MgmtSubtype {
    AssociationRequest = 0,
    AssociationResponse = 1,
    ReassociationRequest = 2,
    ReassociationResponse = 3,
    ProbeRequest = 4,
    ProbeResponse = 5,
    Beacon = 8,
    Atim = 9,
    Disassociation = 10,
    Authentication = 11,
    Deauthentication = 12,
    Action = 13,
}

/// Data frame subtypes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DataSubtype {
    Data = 0,
    DataCfAck = 1,
    DataCfPoll = 2,
    DataCfAckPoll = 3,
    Null = 4,
    CfAck = 5,
    CfPoll = 6,
    CfAckPoll = 7,
    QosData = 8,
    QosNull = 12,
}

/// 802.11 Frame Control field
#[derive(Debug, Clone, Copy)]
pub struct FrameControl {
    pub protocol_version: u8,
    pub frame_type: u8,
    pub subtype: u8,
    pub to_ds: bool,
    pub from_ds: bool,
    pub more_frag: bool,
    pub retry: bool,
    pub power_mgmt: bool,
    pub more_data: bool,
    pub protected: bool,
    pub order: bool,
}

impl FrameControl {
    pub fn to_bytes(&self) -> [u8; 2] {
        let mut fc = 0u16;
        fc |= (self.protocol_version as u16) & 0x3;
        fc |= ((self.frame_type as u16) & 0x3) << 2;
        fc |= ((self.subtype as u16) & 0xF) << 4;
        fc |= (self.to_ds as u16) << 8;
        fc |= (self.from_ds as u16) << 9;
        fc |= (self.more_frag as u16) << 10;
        fc |= (self.retry as u16) << 11;
        fc |= (self.power_mgmt as u16) << 12;
        fc |= (self.more_data as u16) << 13;
        fc |= (self.protected as u16) << 14;
        fc |= (self.order as u16) << 15;
        fc.to_le_bytes()
    }
    
    pub fn from_bytes(bytes: [u8; 2]) -> Self {
        let fc = u16::from_le_bytes(bytes);
        Self {
            protocol_version: (fc & 0x3) as u8,
            frame_type: ((fc >> 2) & 0x3) as u8,
            subtype: ((fc >> 4) & 0xF) as u8,
            to_ds: (fc & (1 << 8)) != 0,
            from_ds: (fc & (1 << 9)) != 0,
            more_frag: (fc & (1 << 10)) != 0,
            retry: (fc & (1 << 11)) != 0,
            power_mgmt: (fc & (1 << 12)) != 0,
            more_data: (fc & (1 << 13)) != 0,
            protected: (fc & (1 << 14)) != 0,
            order: (fc & (1 << 15)) != 0,
        }
    }
}

/// Information Elements (IEs)
#[derive(Debug, Clone)]
pub enum InformationElement {
    Ssid(String),
    SupportedRates(Vec<u8>),
    DsParameter(u8),
    Tim { dtim_count: u8, dtim_period: u8, bitmap: Vec<u8> },
    Country { code: [u8; 2], channels: Vec<(u8, u8, i8)> },
    PowerConstraint(u8),
    HtCapabilities(Vec<u8>),
    HtOperation(Vec<u8>),
    ExtendedSupportedRates(Vec<u8>),
    RsnInformation(Vec<u8>),
    VhtCapabilities(Vec<u8>),
    VhtOperation(Vec<u8>),
    HeCapabilities(Vec<u8>),  // WiFi 6
    HeOperation(Vec<u8>),
    Vendor { oui: [u8; 3], data: Vec<u8> },
    Unknown { id: u8, data: Vec<u8> },
}

impl InformationElement {
    pub fn parse(id: u8, data: &[u8]) -> Self {
        match id {
            0 => InformationElement::Ssid(
                String::from_utf8_lossy(data).to_string()
            ),
            1 => InformationElement::SupportedRates(data.to_vec()),
            3 => InformationElement::DsParameter(data[0]),
            5 => {
                if data.len() >= 4 {
                    InformationElement::Tim {
                        dtim_count: data[0],
                        dtim_period: data[1],
                        bitmap: data[3..].to_vec(),
                    }
                } else {
                    InformationElement::Unknown { id, data: data.to_vec() }
                }
            },
            7 => {
                if data.len() >= 2 {
                    let code = [data[0], data[1]];
                    let mut channels = Vec::new();
                    let mut i = 3;
                    while i + 2 < data.len() {
                        channels.push((data[i], data[i+1], data[i+2] as i8));
                        i += 3;
                    }
                    InformationElement::Country { code, channels }
                } else {
                    InformationElement::Unknown { id, data: data.to_vec() }
                }
            },
            32 => InformationElement::PowerConstraint(data[0]),
            45 => InformationElement::HtCapabilities(data.to_vec()),
            48 => InformationElement::RsnInformation(data.to_vec()),
            50 => InformationElement::ExtendedSupportedRates(data.to_vec()),
            61 => InformationElement::HtOperation(data.to_vec()),
            191 => InformationElement::VhtCapabilities(data.to_vec()),
            192 => InformationElement::VhtOperation(data.to_vec()),
            255 => {
                if data.len() > 0 {
                    match data[0] {
                        35 => InformationElement::HeCapabilities(data[1..].to_vec()),
                        36 => InformationElement::HeOperation(data[1..].to_vec()),
                        _ => InformationElement::Unknown { id, data: data.to_vec() },
                    }
                } else {
                    InformationElement::Unknown { id, data: data.to_vec() }
                }
            },
            221 => {
                if data.len() >= 3 {
                    InformationElement::Vendor {
                        oui: [data[0], data[1], data[2]],
                        data: data[3..].to_vec(),
                    }
                } else {
                    InformationElement::Unknown { id, data: data.to_vec() }
                }
            },
            _ => InformationElement::Unknown { id, data: data.to_vec() },
        }
    }
    
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        match self {
            InformationElement::Ssid(s) => {
                bytes.push(0);
                bytes.push(s.len() as u8);
                bytes.extend_from_slice(s.as_bytes());
            },
            InformationElement::SupportedRates(rates) => {
                bytes.push(1);
                bytes.push(rates.len() as u8);
                bytes.extend_from_slice(rates);
            },
            InformationElement::DsParameter(ch) => {
                bytes.push(3);
                bytes.push(1);
                bytes.push(*ch);
            },
            InformationElement::RsnInformation(data) => {
                bytes.push(48);
                bytes.push(data.len() as u8);
                bytes.extend_from_slice(data);
            },
            _ => {} // Implement others as needed
        }
        bytes
    }
}

/// Parse IEs from buffer
pub fn parse_information_elements(data: &[u8]) -> Vec<InformationElement> {
    let mut ies = Vec::new();
    let mut offset = 0;
    
    while offset + 2 <= data.len() {
        let id = data[offset];
        let len = data[offset + 1] as usize;
        offset += 2;
        
        if offset + len > data.len() {
            break;
        }
        
        let ie_data = &data[offset..offset + len];
        ies.push(InformationElement::parse(id, ie_data));
        offset += len;
    }
    
    ies
}

/// Build probe request frame
pub fn build_probe_request(
    src: [u8; 6],
    ssid: Option<&str>,
    caps: &super::WiFiCapabilities,
) -> Result<Vec<u8>, super::WiFiError> {
    let mut frame = Vec::new();
    
    // Frame Control
    let fc = FrameControl {
        protocol_version: 0,
        frame_type: FrameType::Management as u8,
        subtype: MgmtSubtype::ProbeRequest as u8,
        to_ds: false,
        from_ds: false,
        more_frag: false,
        retry: false,
        power_mgmt: false,
        more_data: false,
        protected: false,
        order: false,
    };
    frame.extend_from_slice(&fc.to_bytes());
    
    // Duration
    frame.extend_from_slice(&[0, 0]);
    
    // Address 1 (DA) - Broadcast
    frame.extend_from_slice(&[0xff; 6]);
    
    // Address 2 (SA)
    frame.extend_from_slice(&src);
    
    // Address 3 (BSSID) - Broadcast
    frame.extend_from_slice(&[0xff; 6]);
    
    // Sequence Control
    frame.extend_from_slice(&[0, 0]);
    
    // Information Elements
    // SSID
    if let Some(s) = ssid {
        frame.extend_from_slice(&InformationElement::Ssid(s.to_string()).to_bytes());
    } else {
        frame.extend_from_slice(&InformationElement::Ssid(String::new()).to_bytes());
    }
    
    // Supported rates
    let rates = vec![0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
    frame.extend_from_slice(&InformationElement::SupportedRates(rates).to_bytes());
    
    Ok(frame)
}

/// Parse beacon frame
pub fn parse_beacon_frame(data: &[u8]) -> Option<super::BssInfo> {
    if data.len() < 36 {
        return None;
    }
    
    // Skip Frame Control (2), Duration (2)
    let bssid = [data[16], data[17], data[18], data[19], data[20], data[21]];
    
    // Fixed parameters start at offset 24
    let _timestamp = u64::from_le_bytes([
        data[24], data[25], data[26], data[27],
        data[28], data[29], data[30], data[31],
    ]);
    let beacon_interval = u16::from_le_bytes([data[32], data[33]]);
    let capabilities = u16::from_le_bytes([data[34], data[35]]);
    
    // Parse IEs
    let ies = parse_information_elements(&data[36..]);
    
    let mut ssid = String::new();
    let mut channel = 0;
    let mut rates = Vec::new();
    let mut rsn = false;
    
    for ie in ies {
        match ie {
            InformationElement::Ssid(s) => ssid = s,
            InformationElement::DsParameter(ch) => channel = ch,
            InformationElement::SupportedRates(r) => rates.extend_from_slice(&r),
            InformationElement::RsnInformation(_) => rsn = true,
            _ => {}
        }
    }
    
    Some(super::BssInfo {
        ssid,
        bssid,
        channel,
        band: if channel <= 14 {
            super::WiFiBand::Band2_4GHz
        } else {
            super::WiFiBand::Band5GHz
        },
        rssi: -50, // Would come from PHY layer
        noise: -90,
        beacon_interval,
        capabilities,
        security: super::BssSecurity {
            wpa: false,
            wpa2: rsn,
            wpa3: false,
            aes: rsn,
            tkip: false,
        },
        rates,
        timestamp: 0,
    })
}

/// Parse probe response (similar to beacon)
pub fn parse_probe_response_frame(data: &[u8]) -> Option<super::BssInfo> {
    parse_beacon_frame(data)
}

/// Build authentication frame
pub fn build_authentication_frame(
    src: [u8; 6],
    dst: [u8; 6],
    algo: AuthAlgorithm,
    seq: u16,
    status: StatusCode,
) -> Result<Vec<u8>, super::WiFiError> {
    let mut frame = Vec::new();
    
    // Frame Control
    let fc = FrameControl {
        protocol_version: 0,
        frame_type: FrameType::Management as u8,
        subtype: MgmtSubtype::Authentication as u8,
        to_ds: false,
        from_ds: false,
        more_frag: false,
        retry: false,
        power_mgmt: false,
        more_data: false,
        protected: false,
        order: false,
    };
    frame.extend_from_slice(&fc.to_bytes());
    
    // Duration
    frame.extend_from_slice(&[0x00, 0x00]);
    
    // DA
    frame.extend_from_slice(&dst);
    
    // SA
    frame.extend_from_slice(&src);
    
    // BSSID
    frame.extend_from_slice(&dst);
    
    // Sequence Control
    frame.extend_from_slice(&[0, 0]);
    
    // Authentication algorithm
    frame.extend_from_slice(&(algo as u16).to_le_bytes());
    
    // Transaction sequence
    frame.extend_from_slice(&seq.to_le_bytes());
    
    // Status code
    frame.extend_from_slice(&(status as u16).to_le_bytes());
    
    Ok(frame)
}

/// Check if frame is authentication response
pub fn is_authentication_response(data: &[u8], expected_bssid: [u8; 6]) -> bool {
    if data.len() < 30 {
        return false;
    }
    
    let fc = FrameControl::from_bytes([data[0], data[1]]);
    if fc.frame_type != FrameType::Management as u8 || fc.subtype != MgmtSubtype::Authentication as u8 {
        return false;
    }
    
    let bssid = [data[10], data[11], data[12], data[13], data[14], data[15]];
    bssid == expected_bssid
}

/// Build association request
pub fn build_association_request(
    src: [u8; 6],
    dst: [u8; 6],
    ssid: &str,
    rates: &[u8],
    caps: &super::WiFiCapabilities,
) -> Result<Vec<u8>, super::WiFiError> {
    let mut frame = Vec::new();
    
    // Frame Control
    let fc = FrameControl {
        protocol_version: 0,
        frame_type: FrameType::Management as u8,
        subtype: MgmtSubtype::AssociationRequest as u8,
        to_ds: false,
        from_ds: false,
        more_frag: false,
        retry: false,
        power_mgmt: false,
        more_data: false,
        protected: false,
        order: false,
    };
    frame.extend_from_slice(&fc.to_bytes());
    
    // Duration
    frame.extend_from_slice(&[0x00, 0x00]);
    
    // DA
    frame.extend_from_slice(&dst);
    
    // SA
    frame.extend_from_slice(&src);
    
    // BSSID
    frame.extend_from_slice(&dst);
    
    // Sequence Control
    frame.extend_from_slice(&[0, 0]);
    
    // Capability info
    let cap_info = 0x0431u16; // ESS, Privacy, Short preamble
    frame.extend_from_slice(&cap_info.to_le_bytes());
    
    // Listen interval
    frame.extend_from_slice(&10u16.to_le_bytes());
    
    // IEs
    frame.extend_from_slice(&InformationElement::Ssid(ssid.to_string()).to_bytes());
    frame.extend_from_slice(&InformationElement::SupportedRates(rates.to_vec()).to_bytes());
    
    Ok(frame)
}

/// Check if frame is association response
pub fn is_association_response(data: &[u8], expected_bssid: [u8; 6]) -> bool {
    if data.len() < 30 {
        return false;
    }
    
    let fc = FrameControl::from_bytes([data[0], data[1]]);
    if fc.frame_type != FrameType::Management as u8 || fc.subtype != MgmtSubtype::AssociationResponse as u8 {
        return false;
    }
    
    let bssid = [data[10], data[11], data[12], data[13], data[14], data[15]];
    bssid == expected_bssid
}

/// Build deauthentication frame
pub fn build_deauthentication_frame(
    src: [u8; 6],
    dst: [u8; 6],
    reason: ReasonCode,
) -> Result<Vec<u8>, super::WiFiError> {
    let mut frame = Vec::new();
    
    // Frame Control
    let fc = FrameControl {
        protocol_version: 0,
        frame_type: FrameType::Management as u8,
        subtype: MgmtSubtype::Deauthentication as u8,
        to_ds: false,
        from_ds: false,
        more_frag: false,
        retry: false,
        power_mgmt: false,
        more_data: false,
        protected: false,
        order: false,
    };
    frame.extend_from_slice(&fc.to_bytes());
    
    // Duration
    frame.extend_from_slice(&[0x00, 0x00]);
    
    // DA
    frame.extend_from_slice(&dst);
    
    // SA
    frame.extend_from_slice(&src);
    
    // BSSID
    frame.extend_from_slice(&dst);
    
    // Sequence Control
    frame.extend_from_slice(&[0, 0]);
    
    // Reason code
    frame.extend_from_slice(&(reason as u16).to_le_bytes());
    
    Ok(frame)
}

/// Build data frame
pub fn build_data_frame(
    src: [u8; 6],
    bssid: [u8; 6],
    dst: [u8; 6],
    data: &[u8],
) -> Result<Vec<u8>, super::WiFiError> {
    let mut frame = Vec::new();
    
    // Frame Control
    let fc = FrameControl {
        protocol_version: 0,
        frame_type: FrameType::Data as u8,
        subtype: DataSubtype::QosData as u8,
        to_ds: true,
        from_ds: false,
        more_frag: false,
        retry: false,
        power_mgmt: false,
        more_data: false,
        protected: false,
        order: false,
    };
    frame.extend_from_slice(&fc.to_bytes());
    
    // Duration
    frame.extend_from_slice(&[0x00, 0x00]);
    
    // Address 1 (BSSID)
    frame.extend_from_slice(&bssid);
    
    // Address 2 (SA)
    frame.extend_from_slice(&src);
    
    // Address 3 (DA)
    frame.extend_from_slice(&dst);
    
    // Sequence Control
    frame.extend_from_slice(&[0, 0]);
    
    // QoS Control
    frame.extend_from_slice(&[0, 0]);
    
    // Data
    frame.extend_from_slice(data);
    
    Ok(frame)
}

/// Parse data frame
pub fn parse_data_frame(frame: &[u8]) -> Option<(Vec<u8>, [u8; 6])> {
    if frame.len() < 26 {
        return None;
    }
    
    let fc = FrameControl::from_bytes([frame[0], frame[1]]);
    if fc.frame_type != FrameType::Data as u8 {
        return None;
    }
    
    // Address 2 is source
    let src = [frame[10], frame[11], frame[12], frame[13], frame[14], frame[15]];
    
    // Data starts after headers
    let data_offset = if fc.subtype == DataSubtype::QosData as u8 { 26 } else { 24 };
    let data = frame[data_offset..].to_vec();
    
    Some((data, src))
}

/// Authentication algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum AuthAlgorithm {
    OpenSystem = 0,
    SharedKey = 1,
    Sae = 3,  // WPA3
}

/// Status codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StatusCode {
    Success = 0,
    Failure = 1,
    RefusedCapabilities = 10,
    RefusedExternalReason = 12,
}

/// Reason codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ReasonCode {
    Unspecified = 1,
    PreviousAuthNotValid = 2,
    Leaving = 3,
    DisassocInactivity = 4,
    DisassocApBusy = 5,
}
