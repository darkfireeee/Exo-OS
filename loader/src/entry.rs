#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserEntry {
    pub rip: u64,
    pub rsp: u64,
    pub argc: usize,
}

pub fn align_stack_down(value: u64) -> u64 {
    value & !0xf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_alignment_is_16_byte() {
        assert_eq!(align_stack_down(0x100f), 0x1000);
    }
}
