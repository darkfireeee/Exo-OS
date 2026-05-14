#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PosixPortKind {
    Libc,
    Identity,
    Auth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PosixPort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: PosixPortKind,
}

pub const POSIX_PORTS: &[PosixPort] = &[
    PosixPort {
        name: "musl",
        vendor_tree: "musl-upstream",
        kind: PosixPortKind::Libc,
    },
    PosixPort {
        name: "relibc",
        vendor_tree: "relibc-git-upstream",
        kind: PosixPortKind::Libc,
    },
    PosixPort {
        name: "linux-pam",
        vendor_tree: "linux-pam-upstream",
        kind: PosixPortKind::Auth,
    },
    PosixPort {
        name: "shadow-rs",
        vendor_tree: "shadow-rs-upstream",
        kind: PosixPortKind::Identity,
    },
];

pub fn posix_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f50_4f53_u64;
    for i in 0..iterations.max(1) {
        let port = POSIX_PORTS[i as usize % POSIX_PORTS.len()];
        acc = acc.rotate_left(3) ^ port.vendor_tree.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}
