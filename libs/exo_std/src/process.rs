// libs/exo_std/src/process.rs

/// ID de processus
pub type Pid = u32;

/// Quitte le processus actuel
pub fn exit(code: i32) -> ! {
    #[cfg(feature = "test_mode")]
    loop {}
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        extern "C" {
            fn sys_exit(code: i32) -> !;
        }
        sys_exit(code)
    }
}

/// ID du processus actuel
pub fn id() -> Pid {
    0 // TODO
}
