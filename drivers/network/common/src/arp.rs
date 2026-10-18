//! arp.rs — Parsing et sérialisation des paquets ARP (RFC 826) sans allocation
//!
//! Spécification ExoNet V4/V1.2 / V5 pour Ethernet + IPv4.

pub const ARP_HTYPE_ETHERNET: u16 = 1;
pub const ARP_PTYPE_IPV4: u16 = 0x0800;
pub const ARP_HLEN_ETHERNET: u8 = 6;
pub const ARP_PLEN_IPV4: u8 = 4;

pub const ARP_OP_REQUEST: u16 = 1;
pub const ARP_OP_REPLY: u16 = 2;

pub const ARP_PACKET_LEN: usize = 28;

/// Paquet ARP Ethernet/IPv4 de 28 octets
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArpPacket {
    pub htype: [u8; 2],
    pub ptype: [u8; 2],
    pub hlen: u8,
    pub plen: u8,
    pub oper: [u8; 2],
    pub sha: [u8; 6],
    pub spa: [u8; 4],
    pub tha: [u8; 6],
    pub tpa: [u8; 4],
}

impl ArpPacket {
    pub const LEN: usize = ARP_PACKET_LEN;

    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < ARP_PACKET_LEN {
            return None;
        }
        let packet = Self {
            htype: [buf[0], buf[1]],
            ptype: [buf[2], buf[3]],
            hlen: buf[4],
            plen: buf[5],
            oper: [buf[6], buf[7]],
            sha: [buf[8], buf[9], buf[10], buf[11], buf[12], buf[13]],
            spa: [buf[14], buf[15], buf[16], buf[17]],
            tha: [buf[18], buf[19], buf[20], buf[21], buf[22], buf[23]],
            tpa: [buf[24], buf[25], buf[26], buf[27]],
        };
        if packet.hardware_type() != ARP_HTYPE_ETHERNET
            || packet.protocol_type() != ARP_PTYPE_IPV4
            || packet.hlen != ARP_HLEN_ETHERNET
            || packet.plen != ARP_PLEN_IPV4
        {
            return None;
        }
        Some(packet)
    }

    #[inline]
    pub fn hardware_type(&self) -> u16 {
        u16::from_be_bytes(self.htype)
    }

    #[inline]
    pub fn protocol_type(&self) -> u16 {
        u16::from_be_bytes(self.ptype)
    }

    #[inline]
    pub fn operation(&self) -> u16 {
        u16::from_be_bytes(self.oper)
    }

    #[inline]
    pub fn is_request(&self) -> bool {
        self.operation() == ARP_OP_REQUEST
    }

    #[inline]
    pub fn is_reply(&self) -> bool {
        self.operation() == ARP_OP_REPLY
    }

    pub fn emit_request(sender_mac: &[u8; 6], sender_ip: &[u8; 4], target_ip: &[u8; 4], out: &mut [u8]) -> bool {
        if out.len() < ARP_PACKET_LEN {
            return false;
        }
        let htype = ARP_HTYPE_ETHERNET.to_be_bytes();
        let ptype = ARP_PTYPE_IPV4.to_be_bytes();
        let oper = ARP_OP_REQUEST.to_be_bytes();

        out[0..2].copy_from_slice(&htype);
        out[2..4].copy_from_slice(&ptype);
        out[4] = ARP_HLEN_ETHERNET;
        out[5] = ARP_PLEN_IPV4;
        out[6..8].copy_from_slice(&oper);
        out[8..14].copy_from_slice(sender_mac);
        out[14..18].copy_from_slice(sender_ip);
        out[18..24].copy_from_slice(&[0u8; 6]); // Inconnu dans la requête
        out[24..28].copy_from_slice(target_ip);
        true
    }

    pub fn emit_reply(
        sender_mac: &[u8; 6],
        sender_ip: &[u8; 4],
        target_mac: &[u8; 6],
        target_ip: &[u8; 4],
        out: &mut [u8],
    ) -> bool {
        if out.len() < ARP_PACKET_LEN {
            return false;
        }
        let htype = ARP_HTYPE_ETHERNET.to_be_bytes();
        let ptype = ARP_PTYPE_IPV4.to_be_bytes();
        let oper = ARP_OP_REPLY.to_be_bytes();

        out[0..2].copy_from_slice(&htype);
        out[2..4].copy_from_slice(&ptype);
        out[4] = ARP_HLEN_ETHERNET;
        out[5] = ARP_PLEN_IPV4;
        out[6..8].copy_from_slice(&oper);
        out[8..14].copy_from_slice(sender_mac);
        out[14..18].copy_from_slice(sender_ip);
        out[18..24].copy_from_slice(target_mac);
        out[24..28].copy_from_slice(target_ip);
        true
    }
}
