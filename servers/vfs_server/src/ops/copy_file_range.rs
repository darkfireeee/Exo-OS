//! copy_file_range role boundary.

pub fn must_delegate_to_ring0() -> bool {
    true
}

pub fn can_reflink(src_encrypted: bool, dst_encrypted: bool) -> bool {
    !src_encrypted && !dst_encrypted
}
