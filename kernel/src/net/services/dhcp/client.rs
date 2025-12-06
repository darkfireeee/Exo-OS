// kernel/src/net/dhcp.rs - DHCP Client Implementation
// Dynamic Host Configuration Protocol (RFC 2131)

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::RwLock;

// ============================================================================
// DHCP Constants
// ============================================================================

const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;

const DHCP_MAGIC_COOKIE: u32 = 0x63825363;

// DHCP Message Types
const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_DECLINE: u8 = 4;
const DHCP_ACK: u8 = 5;
const DHCP_NAK: u8 = 6;
const DHCP_RELEASE: u8 = 7;
const DHCP_INFORM: u8 = 8;

// DHCP Options
const DHCP_OPT_PAD: u8 = 0;
const DHCP_OPT_SUBNET_MASK: u8 = 1;
const DHCP_OPT_ROUTER: u8 = 3;
const DHCP_OPT_DNS_SERVER: u8 = 6;
const DHCP_OPT_HOSTNAME: u8 = 12;
const DHCP_OPT_REQUESTED_IP: u8 = 50;
const DHCP_OPT_LEASE_TIME: u8 = 51;
const DHCP_OPT_MESSAGE_TYPE: u8 = 53;
const DHCP_OPT_SERVER_ID: u8 = 54;
const DHCP_OPT_PARAM_REQUEST: u8 = 55;
const DHCP_OPT_RENEWAL_TIME: u8 = 58;
const DHCP_OPT_REBINDING_TIME: u8 = 59;
const DHCP_OPT_END: u8 = 255;

// ============================================================================
// DHCP Packet Structure
// ============================================================================

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DhcpPacket {
    pub op: u8,               // Message opcode: 1=request, 2=reply
    pub htype: u8,            // Hardware type: 1=Ethernet
    pub hlen: u8,             // Hardware address length: 6 for MAC
    pub hops: u8,             // Hops (0 for client)
    pub xid: u32,             // Transaction ID
    pub secs: u16,            // Seconds elapsed
    pub flags: u16,           // Flags (0x8000 = broadcast)
    pub ciaddr: [u8; 4],      // Client IP address
    pub yiaddr: [u8; 4],      // Your (client) IP address
    pub siaddr: [u8; 4],      // Server IP address
    pub giaddr: [u8; 4],      // Gateway IP address
    pub chaddr: [u8; 16],     // Client hardware address
    pub sname: [u8; 64],      // Server name
    pub file: [u8; 128],      // Boot file name
    pub magic: u32,           // Magic cookie: 0x63825363
    // Options follow
}

impl DhcpPacket {
    pub fn new_discover(mac: [u8; 6], xid: u32) -> Self {
        let mut chaddr = [0u8; 16];
        chaddr[..6].copy_from_slice(&mac);

        Self {
            op: 1, // BOOTREQUEST
            htype: 1, // Ethernet
            hlen: 6,
            hops: 0,
            xid: xid.to_be(),
            secs: 0,
            flags: 0x8000u16.to_be(), // Broadcast flag
            ciaddr: [0; 4],
            yiaddr: [0; 4],
            siaddr: [0; 4],
            giaddr: [0; 4],
            chaddr,
            sname: [0; 64],
            file: [0; 128],
            magic: DHCP_MAGIC_COOKIE.to_be(),
        }
    }

    pub fn new_request(mac: [u8; 6], xid: u32, requested_ip: [u8; 4], server_ip: [u8; 4]) -> Self {
        let mut packet = Self::new_discover(mac, xid);
        packet.siaddr = server_ip;
        packet
    }
}

// ============================================================================
// DHCP Client State
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpState {
    Init,
    Selecting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
}

pub struct DhcpClient {
    mac_address: [u8; 6],
    state: RwLock<DhcpState>,
    transaction_id: AtomicU32,
    
    // Leased configuration
    assigned_ip: RwLock<Option<[u8; 4]>>,
    server_ip: RwLock<Option<[u8; 4]>>,
    subnet_mask: RwLock<Option<[u8; 4]>>,
    router: RwLock<Option<[u8; 4]>>,
    dns_servers: RwLock<Vec<[u8; 4]>>,
    lease_time: AtomicU64,
    renewal_time: AtomicU64,
    rebind_time: AtomicU64,
    lease_start: AtomicU64,
    
    // Statistics
    stats: DhcpStats,
}

#[derive(Debug, Default)]
struct DhcpStats {
    discovers_sent: AtomicU64,
    offers_received: AtomicU64,
    requests_sent: AtomicU64,
    acks_received: AtomicU64,
    naks_received: AtomicU64,
}

impl DhcpClient {
    pub fn new(mac_address: [u8; 6]) -> Self {
        Self {
            mac_address,
            state: RwLock::new(DhcpState::Init),
            transaction_id: AtomicU32::new(0x12345678), // Random
            assigned_ip: RwLock::new(None),
            server_ip: RwLock::new(None),
            subnet_mask: RwLock::new(None),
            router: RwLock::new(None),
            dns_servers: RwLock::new(Vec::new()),
            lease_time: AtomicU64::new(0),
            renewal_time: AtomicU64::new(0),
            rebind_time: AtomicU64::new(0),
            lease_start: AtomicU64::new(0),
            stats: DhcpStats::default(),
        }
    }

    // ========================================================================
    // DHCPDISCOVER - Request configuration
    // ========================================================================

    pub fn send_discover(&self) -> Vec<u8> {
        let xid = self.transaction_id.fetch_add(1, Ordering::Relaxed);
        let packet = DhcpPacket::new_discover(self.mac_address, xid);

        let mut data = Vec::new();
        
        // DHCP packet
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &packet as *const _ as *const u8,
                core::mem::size_of::<DhcpPacket>()
            )
        });

        // Options
        self.add_option(&mut data, DHCP_OPT_MESSAGE_TYPE, &[DHCP_DISCOVER]);
        
        // Parameter request list
        self.add_option(&mut data, DHCP_OPT_PARAM_REQUEST, &[
            DHCP_OPT_SUBNET_MASK,
            DHCP_OPT_ROUTER,
            DHCP_OPT_DNS_SERVER,
            DHCP_OPT_LEASE_TIME,
        ]);
        
        self.add_option(&mut data, DHCP_OPT_END, &[]);

        *self.state.write() = DhcpState::Selecting;
        self.stats.discovers_sent.fetch_add(1, Ordering::Relaxed);

        log::info!("[DHCP] Sent DISCOVER (xid: 0x{:08x})", xid);
        data
    }

    // ========================================================================
    // DHCPREQUEST - Accept offer
    // ========================================================================

    pub fn send_request(&self, offered_ip: [u8; 4], server_ip: [u8; 4]) -> Vec<u8> {
        let xid = self.transaction_id.load(Ordering::Relaxed);
        let packet = DhcpPacket::new_request(self.mac_address, xid, offered_ip, server_ip);

        let mut data = Vec::new();
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &packet as *const _ as *const u8,
                core::mem::size_of::<DhcpPacket>()
            )
        });

        // Options
        self.add_option(&mut data, DHCP_OPT_MESSAGE_TYPE, &[DHCP_REQUEST]);
        self.add_option(&mut data, DHCP_OPT_REQUESTED_IP, &offered_ip);
        self.add_option(&mut data, DHCP_OPT_SERVER_ID, &server_ip);
        self.add_option(&mut data, DHCP_OPT_END, &[]);

        *self.state.write() = DhcpState::Requesting;
        self.stats.requests_sent.fetch_add(1, Ordering::Relaxed);

        log::info!("[DHCP] Sent REQUEST for {} from server {}",
            format_ip(offered_ip), format_ip(server_ip));
        data
    }

    // ========================================================================
    // Handle incoming DHCP packet
    // ========================================================================

    pub fn handle_packet(&self, data: &[u8]) -> Result<DhcpAction, DhcpError> {
        if data.len() < core::mem::size_of::<DhcpPacket>() {
            return Err(DhcpError::PacketTooSmall);
        }

        let packet = unsafe {
            &*(data.as_ptr() as *const DhcpPacket)
        };

        // Vérifier le magic cookie
        if u32::from_be(packet.magic) != DHCP_MAGIC_COOKIE {
            return Err(DhcpError::InvalidMagic);
        }

        // Vérifier le XID
        let xid = u32::from_be(packet.xid);
        if xid != self.transaction_id.load(Ordering::Relaxed) {
            return Err(DhcpError::InvalidTransactionId);
        }

        // Parser les options
        let options_start = core::mem::size_of::<DhcpPacket>();
        let options = &data[options_start..];
        
        let (message_type, parsed_options) = self.parse_options(options)?;

        match message_type {
            DHCP_OFFER => {
                self.stats.offers_received.fetch_add(1, Ordering::Relaxed);
                
                let offered_ip = packet.yiaddr;
                let server_ip = parsed_options.server_id.ok_or(DhcpError::MissingServerID)?;

                log::info!("[DHCP] Received OFFER: IP={}, Server={}",
                    format_ip(offered_ip), format_ip(server_ip));

                Ok(DhcpAction::SendRequest { offered_ip, server_ip })
            }
            DHCP_ACK => {
                self.stats.acks_received.fetch_add(1, Ordering::Relaxed);
                
                let assigned_ip = packet.yiaddr;
                *self.assigned_ip.write() = Some(assigned_ip);
                *self.server_ip.write() = parsed_options.server_id;
                *self.subnet_mask.write() = parsed_options.subnet_mask;
                *self.router.write() = parsed_options.router;
                *self.dns_servers.write() = parsed_options.dns_servers;

                let lease_time = parsed_options.lease_time.unwrap_or(3600);
                self.lease_time.store(lease_time, Ordering::Relaxed);
                self.lease_start.store(crate::time::monotonic_time(), Ordering::Relaxed);
                
                // T1 = 0.5 * lease_time, T2 = 0.875 * lease_time
                self.renewal_time.store(lease_time / 2, Ordering::Relaxed);
                self.rebind_time.store(lease_time * 7 / 8, Ordering::Relaxed);

                *self.state.write() = DhcpState::Bound;

                log::info!("[DHCP] Received ACK: IP={}, Lease={}s",
                    format_ip(assigned_ip), lease_time);

                Ok(DhcpAction::Configured {
                    ip: assigned_ip,
                    subnet_mask: parsed_options.subnet_mask.unwrap_or([255, 255, 255, 0]),
                    router: parsed_options.router,
                    dns_servers: parsed_options.dns_servers,
                })
            }
            DHCP_NAK => {
                self.stats.naks_received.fetch_add(1, Ordering::Relaxed);
                *self.state.write() = DhcpState::Init;
                
                log::warn!("[DHCP] Received NAK, restarting discovery");
                Ok(DhcpAction::Restart)
            }
            _ => Err(DhcpError::UnknownMessageType),
        }
    }

    // ========================================================================
    // Parse DHCP Options
    // ========================================================================

    fn parse_options(&self, data: &[u8]) -> Result<(u8, ParsedOptions), DhcpError> {
        let mut message_type = None;
        let mut options = ParsedOptions::default();
        
        let mut i = 0;
        while i < data.len() {
            let opt = data[i];
            
            if opt == DHCP_OPT_END {
                break;
            }
            
            if opt == DHCP_OPT_PAD {
                i += 1;
                continue;
            }
            
            if i + 1 >= data.len() {
                break;
            }
            
            let len = data[i + 1] as usize;
            if i + 2 + len > data.len() {
                break;
            }
            
            let value = &data[i + 2..i + 2 + len];
            
            match opt {
                DHCP_OPT_MESSAGE_TYPE if len == 1 => message_type = Some(value[0]),
                DHCP_OPT_SUBNET_MASK if len == 4 => {
                    options.subnet_mask = Some([value[0], value[1], value[2], value[3]]);
                }
                DHCP_OPT_ROUTER if len >= 4 => {
                    options.router = Some([value[0], value[1], value[2], value[3]]);
                }
                DHCP_OPT_DNS_SERVER if len >= 4 => {
                    for chunk in value.chunks(4) {
                        if chunk.len() == 4 {
                            options.dns_servers.push([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        }
                    }
                }
                DHCP_OPT_SERVER_ID if len == 4 => {
                    options.server_id = Some([value[0], value[1], value[2], value[3]]);
                }
                DHCP_OPT_LEASE_TIME if len == 4 => {
                    options.lease_time = Some(u32::from_be_bytes([value[0], value[1], value[2], value[3]]) as u64);
                }
                _ => {}
            }
            
            i += 2 + len;
        }
        
        Ok((message_type.ok_or(DhcpError::MissingMessageType)?, options))
    }

    fn add_option(&self, data: &mut Vec<u8>, option: u8, value: &[u8]) {
        if option == DHCP_OPT_END {
            data.push(DHCP_OPT_END);
        } else {
            data.push(option);
            data.push(value.len() as u8);
            data.extend_from_slice(value);
        }
    }

    // ========================================================================
    // Getters
    // ========================================================================

    pub fn assigned_ip(&self) -> Option<[u8; 4]> {
        *self.assigned_ip.read()
    }

    pub fn subnet_mask(&self) -> Option<[u8; 4]> {
        *self.subnet_mask.read()
    }

    pub fn router(&self) -> Option<[u8; 4]> {
        *self.router.read()
    }

    pub fn dns_servers(&self) -> Vec<[u8; 4]> {
        self.dns_servers.read().clone()
    }

    pub fn state(&self) -> DhcpState {
        *self.state.read()
    }

    pub fn is_bound(&self) -> bool {
        *self.state.read() == DhcpState::Bound
    }
}

// ============================================================================
// Parsed Options
// ============================================================================

#[derive(Debug, Default)]
struct ParsedOptions {
    server_id: Option<[u8; 4]>,
    subnet_mask: Option<[u8; 4]>,
    router: Option<[u8; 4]>,
    dns_servers: Vec<[u8; 4]>,
    lease_time: Option<u64>,
}

// ============================================================================
// DHCP Action
// ============================================================================

#[derive(Debug)]
pub enum DhcpAction {
    SendRequest {
        offered_ip: [u8; 4],
        server_ip: [u8; 4],
    },
    Configured {
        ip: [u8; 4],
        subnet_mask: [u8; 4],
        router: Option<[u8; 4]>,
        dns_servers: Vec<[u8; 4]>,
    },
    Restart,
}

// ============================================================================
// Utilities
// ============================================================================

fn format_ip(ip: [u8; 4]) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpError {
    PacketTooSmall,
    InvalidMagic,
    InvalidTransactionId,
    MissingMessageType,
    MissingServerID,
    UnknownMessageType,
}
