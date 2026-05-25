pub const CTRL: usize = 0x0000;
pub const STATUS: usize = 0x0008;
pub const ICR: usize = 0x00C0;
pub const IMS: usize = 0x00D0;
pub const IMC: usize = 0x00D8;
pub const RCTL: usize = 0x0100;
pub const TCTL: usize = 0x0400;
pub const TIPG: usize = 0x0410;
pub const RDBAL: usize = 0x2800;
pub const RDBAH: usize = 0x2804;
pub const RDLEN: usize = 0x2808;
pub const RDH: usize = 0x2810;
pub const RDT: usize = 0x2818;
pub const TDBAL: usize = 0x3800;
pub const TDBAH: usize = 0x3804;
pub const TDLEN: usize = 0x3808;
pub const TDH: usize = 0x3810;
pub const TDT: usize = 0x3818;
pub const RAL: usize = 0x5400;
pub const RAH: usize = 0x5404;
pub const MTA: usize = 0x5200;

pub const CTRL_RST: u32 = 1 << 26;
pub const CTRL_SLU: u32 = 1 << 6;

pub const RCTL_EN: u32 = 1 << 1;
pub const RCTL_SBP: u32 = 1 << 2;
pub const RCTL_UPE: u32 = 1 << 3;
pub const RCTL_MPE: u32 = 1 << 4;
pub const RCTL_BAM: u32 = 1 << 15;
pub const RCTL_BSIZE_2048: u32 = 0;
pub const RCTL_SECRC: u32 = 1 << 26;

pub const TCTL_EN: u32 = 1 << 1;
pub const TCTL_PSP: u32 = 1 << 3;
pub const TCTL_CT_SHIFT: u32 = 4;
pub const TCTL_COLD_SHIFT: u32 = 12;

pub const TX_CMD_EOP: u8 = 1 << 0;
pub const TX_CMD_IFCS: u8 = 1 << 1;
pub const TX_CMD_RS: u8 = 1 << 3;
pub const DESC_STATUS_DD: u8 = 1 << 0;

pub const IRQ_RXT0: u32 = 1 << 7;
pub const IRQ_RXO: u32 = 1 << 6;
pub const IRQ_RXDMT0: u32 = 1 << 4;
pub const IRQ_LSC: u32 = 1 << 2;
pub const IRQ_TXDW: u32 = 1 << 0;
