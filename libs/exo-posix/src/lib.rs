#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PosixPortKind {
    Libc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PosixVerdict {
    Canonical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PosixPort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: PosixPortKind,
    pub verdict: PosixVerdict,
    pub exo_boundary: &'static str,
}

pub const POSIX_PORTS: &[PosixPort] = &[PosixPort {
    name: "musl",
    vendor_tree: "musl-upstream",
    kind: PosixPortKind::Libc,
    verdict: PosixVerdict::Canonical,
    exo_boundary: "musl-exo syscall compatibility layer",
}];

pub fn posix_port_allowed(name: &str) -> bool {
    POSIX_PORTS
        .iter()
        .find(|port| port.name == name)
        .map(|port| port.verdict == PosixVerdict::Canonical)
        .unwrap_or(false)
}

pub fn posix_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f50_4f53_u64;
    for i in 0..iterations.max(1) {
        let port = POSIX_PORTS[i as usize % POSIX_PORTS.len()];
        acc = acc.rotate_left(3) ^ port.vendor_tree.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_posix_libs_are_removed() {
        for name in ["linux-pam", "shadow-rs", "relibc"] {
            assert!(!posix_port_allowed(name), "{name}");
            assert!(POSIX_PORTS.iter().all(|port| port.name != name), "{name}");
        }
    }

    #[test]
    fn musl_is_the_single_canonical_libc() {
        let canonical = POSIX_PORTS
            .iter()
            .filter(|port| port.verdict == PosixVerdict::Canonical)
            .count();
        assert_eq!(canonical, 1);
    }
}
