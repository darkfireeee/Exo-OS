pub const CAP_EXEC: u64 = 1 << 0;

pub fn may_exec(mask: u64) -> bool {
    mask & CAP_EXEC != 0
}
