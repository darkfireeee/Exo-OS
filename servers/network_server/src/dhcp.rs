#[allow(dead_code)]
pub const DHCP_CLIENT_PORT: u16 = 68;
#[allow(dead_code)]
pub const DHCP_SERVER_PORT: u16 = 67;
pub const DHCP_FIXED_LEN: usize = 236;
pub const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
pub const DHCP_MIN_PACKET_LEN: usize = DHCP_FIXED_LEN + 4 + 3;

const OP_BOOTREQUEST: u8 = 1;
const OP_BOOTREPLY: u8 = 2;
const HTYPE_ETHERNET: u8 = 1;
const HLEN_ETHERNET: u8 = 6;
const BROADCAST_FLAG: u16 = 0x8000;

const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_PARAMETER_REQUEST: u8 = 55;
const OPT_END: u8 = 255;

const MSG_DISCOVER: u8 = 1;
const MSG_OFFER: u8 = 2;
const MSG_REQUEST: u8 = 3;
const MSG_ACK: u8 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DhcpPhase {
    Init,
    Selecting,
    Requesting,
    Bound,
    Renewing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DhcpAction {
    None,
    Discover,
    Request,
    Renew,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DhcpLease {
    pub ip: u32,
    pub prefix_len: u8,
    pub gateway: u32,
    pub dns: u32,
    pub server_ip: u32,
    pub lease_seconds: u32,
}

pub struct DhcpClient {
    state: DhcpPhase,
    xid: u32,
    offered_ip: u32,
    server_ip: u32,
    lease_until_tick: u64,
    mac: [u8; 6],
}

impl DhcpClient {
    pub const fn new() -> Self {
        Self {
            state: DhcpPhase::Init,
            xid: 0x4558_4f44,
            offered_ip: 0,
            server_ip: 0,
            lease_until_tick: 0,
            mac: [0; 6],
        }
    }

    pub fn configure_mac(&mut self, mac: [u8; 6]) {
        if mac != [0; 6] {
            self.mac = mac;
        }
    }

    pub fn start(&mut self, seed: u32) {
        self.xid = seed ^ 0x4558_4f44;
        self.offered_ip = 0;
        self.server_ip = 0;
        self.lease_until_tick = 0;
        self.state = DhcpPhase::Init;
    }

    pub fn poll(&mut self, now_ms: u64) -> DhcpAction {
        match self.state {
            DhcpPhase::Init => {
                self.state = DhcpPhase::Selecting;
                DhcpAction::Discover
            }
            DhcpPhase::Selecting => DhcpAction::Discover,
            DhcpPhase::Requesting => DhcpAction::Request,
            DhcpPhase::Bound if now_ms >= self.lease_until_tick => {
                self.state = DhcpPhase::Renewing;
                DhcpAction::Renew
            }
            DhcpPhase::Renewing => DhcpAction::Renew,
            DhcpPhase::Bound => DhcpAction::None,
        }
    }

    pub fn ingest(&mut self, packet: &[u8], now_ms: u64) -> Option<DhcpLease> {
        let parsed = parse_packet(packet, self.xid, self.mac)?;
        match parsed.message_type {
            MSG_OFFER if self.state == DhcpPhase::Selecting => {
                self.offered_ip = parsed.yiaddr;
                self.server_ip = parsed.server_ip;
                self.state = DhcpPhase::Requesting;
                None
            }
            MSG_ACK if self.state == DhcpPhase::Requesting || self.state == DhcpPhase::Renewing => {
                let lease_seconds = parsed.lease_seconds.max(60);
                self.lease_until_tick =
                    now_ms.saturating_add((lease_seconds as u64).saturating_mul(1000));
                self.state = DhcpPhase::Bound;
                Some(DhcpLease {
                    ip: parsed.yiaddr,
                    prefix_len: prefix_from_mask(parsed.subnet_mask),
                    gateway: parsed.router,
                    dns: parsed.dns,
                    server_ip: parsed.server_ip,
                    lease_seconds,
                })
            }
            _ => None,
        }
    }

    pub fn build_discover(&self, out: &mut [u8]) -> Option<usize> {
        let opt_start = write_header(out, self.xid, self.mac, 0, 0)?;
        let mut pos = opt_start;
        pos = push_option(out, pos, OPT_MESSAGE_TYPE, &[MSG_DISCOVER])?;
        pos = push_option(
            out,
            pos,
            OPT_PARAMETER_REQUEST,
            &[OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_LEASE_TIME],
        )?;
        finish_options(out, pos)
    }

    pub fn build_request(&self, out: &mut [u8]) -> Option<usize> {
        if self.offered_ip == 0 || self.server_ip == 0 {
            return None;
        }
        let opt_start = write_header(out, self.xid, self.mac, 0, 0)?;
        let mut pos = opt_start;
        pos = push_option(out, pos, OPT_MESSAGE_TYPE, &[MSG_REQUEST])?;
        pos = push_option(out, pos, OPT_REQUESTED_IP, &self.offered_ip.to_be_bytes())?;
        pos = push_option(out, pos, OPT_SERVER_ID, &self.server_ip.to_be_bytes())?;
        pos = push_option(
            out,
            pos,
            OPT_PARAMETER_REQUEST,
            &[OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_LEASE_TIME],
        )?;
        finish_options(out, pos)
    }

    pub const fn state(&self) -> DhcpPhase {
        self.state
    }
}

struct ParsedDhcp {
    message_type: u8,
    yiaddr: u32,
    server_ip: u32,
    subnet_mask: u32,
    router: u32,
    dns: u32,
    lease_seconds: u32,
}

fn parse_packet(packet: &[u8], xid: u32, mac: [u8; 6]) -> Option<ParsedDhcp> {
    if packet.len() < DHCP_MIN_PACKET_LEN
        || packet[0] != OP_BOOTREPLY
        || packet[1] != HTYPE_ETHERNET
        || packet[2] != HLEN_ETHERNET
    {
        return None;
    }
    if u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]) != xid {
        return None;
    }
    if packet[28..34] != mac {
        return None;
    }
    if packet[DHCP_FIXED_LEN..DHCP_FIXED_LEN + 4] != DHCP_MAGIC_COOKIE {
        return None;
    }

    let mut parsed = ParsedDhcp {
        message_type: 0,
        yiaddr: u32::from_be_bytes([packet[16], packet[17], packet[18], packet[19]]),
        server_ip: 0,
        subnet_mask: 0xffff_ff00,
        router: 0,
        dns: 0,
        lease_seconds: 3600,
    };

    let mut pos = DHCP_FIXED_LEN + 4;
    while pos < packet.len() {
        let opt = packet[pos];
        pos += 1;
        if opt == OPT_END {
            break;
        }
        if opt == 0 {
            continue;
        }
        if pos >= packet.len() {
            return None;
        }
        let len = packet[pos] as usize;
        pos += 1;
        if pos + len > packet.len() {
            return None;
        }
        let data = &packet[pos..pos + len];
        match opt {
            OPT_MESSAGE_TYPE if len == 1 => parsed.message_type = data[0],
            OPT_SERVER_ID if len == 4 => parsed.server_ip = be_u32(data),
            OPT_SUBNET_MASK if len == 4 => parsed.subnet_mask = be_u32(data),
            OPT_ROUTER if len >= 4 => parsed.router = be_u32(data),
            OPT_DNS if len >= 4 => parsed.dns = be_u32(data),
            OPT_LEASE_TIME if len == 4 => parsed.lease_seconds = be_u32(data),
            _ => {}
        }
        pos += len;
    }

    (parsed.message_type != 0 && parsed.yiaddr != 0).then_some(parsed)
}

fn write_header(out: &mut [u8], xid: u32, mac: [u8; 6], ciaddr: u32, yiaddr: u32) -> Option<usize> {
    if out.len() < DHCP_FIXED_LEN + 4 {
        return None;
    }
    out[..DHCP_FIXED_LEN + 4].fill(0);
    out[0] = OP_BOOTREQUEST;
    out[1] = HTYPE_ETHERNET;
    out[2] = HLEN_ETHERNET;
    out[4..8].copy_from_slice(&xid.to_be_bytes());
    out[10..12].copy_from_slice(&BROADCAST_FLAG.to_be_bytes());
    out[12..16].copy_from_slice(&ciaddr.to_be_bytes());
    out[16..20].copy_from_slice(&yiaddr.to_be_bytes());
    out[28..34].copy_from_slice(&mac);
    out[DHCP_FIXED_LEN..DHCP_FIXED_LEN + 4].copy_from_slice(&DHCP_MAGIC_COOKIE);
    Some(DHCP_FIXED_LEN + 4)
}

fn push_option(out: &mut [u8], pos: usize, opt: u8, data: &[u8]) -> Option<usize> {
    if data.len() > u8::MAX as usize || pos + 2 + data.len() > out.len() {
        return None;
    }
    out[pos] = opt;
    out[pos + 1] = data.len() as u8;
    out[pos + 2..pos + 2 + data.len()].copy_from_slice(data);
    Some(pos + 2 + data.len())
}

fn finish_options(out: &mut [u8], pos: usize) -> Option<usize> {
    if pos >= out.len() {
        return None;
    }
    out[pos] = OPT_END;
    Some(pos + 1)
}

fn be_u32(data: &[u8]) -> u32 {
    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

pub fn prefix_from_mask(mask: u32) -> u8 {
    let mut prefix = 0u8;
    let mut bit = 31i32;
    while bit >= 0 {
        if (mask & (1u32 << bit)) == 0 {
            break;
        }
        prefix += 1;
        bit -= 1;
    }
    prefix
}
