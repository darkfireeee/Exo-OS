use exo_os::utils::bitops::BitField;
use exo_os::utils::math;

#[test_case]
fn test_bitfield_u64() {
    let mut val: u64 = 0;
    val.set_bit(0, true);
    assert_eq!(val, 1);
    assert!(val.get_bit(0));
}

#[test_case]
fn test_align_up() {
    assert_eq!(math::align_up(1, 4), 4);
    assert_eq!(math::align_up(4, 4), 4);
}
