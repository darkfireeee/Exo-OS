#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DevicePort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub exo_boundary: &'static str,
}

pub const DEVICE_PORTS: &[DevicePort] = &[DevicePort {
    name: "libudev",
    vendor_tree: "libudev-rs-upstream",
    exo_boundary: "device_server hotplug event bridge",
}];

pub fn device_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f44_4556_u64;
    for i in 0..iterations.max(1) {
        acc = acc.rotate_left(5) ^ DEVICE_PORTS[0].vendor_tree.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}
