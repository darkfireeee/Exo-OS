//! Test simple du module fusion_ring

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

fn test_runner(_tests: &[&dyn Fn()]) {
    // Tests basiques sans framework complet
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[cfg(feature = "fusion_rings")]
#[test_case]
fn test_fusion_ring_basic() {
    use exo_kernel::ipc::fusion_ring::FusionRing;
    
    let ring = FusionRing::new();
    assert!(ring.available_slots() == 4096);
    assert!(ring.pending_messages() == 0);
}
