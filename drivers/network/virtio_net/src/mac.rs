use crate::config::VIRTIO_NET_F_MAC;

pub const DEFAULT_MAC: [u8; 6] = [0x02, 0x45, 0x58, 0x4f, 0x00, 0x01];

pub unsafe fn read_mac(device_cfg: *mut u8, negotiated_features: u64) -> [u8; 6] {
    if device_cfg.is_null() || negotiated_features & VIRTIO_NET_F_MAC == 0 {
        return DEFAULT_MAC;
    }
    let mut mac = [0u8; 6];
    let mut idx = 0usize;
    while idx < mac.len() {
        mac[idx] = unsafe { core::ptr::read_volatile(device_cfg.add(idx)) };
        idx += 1;
    }
    if mac == [0; 6] {
        DEFAULT_MAC
    } else {
        mac
    }
}
