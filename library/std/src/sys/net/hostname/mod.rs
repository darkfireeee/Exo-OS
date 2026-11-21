cfg_select! {
    target_os = "exo" => {
        mod exo;
        pub use exo::hostname;
    }
    all(target_family = "unix", not(any(target_os = "espidf", target_os = "exo"))) => {
        mod unix;
        pub use unix::hostname;
    }
    // `GetHostNameW` is only available starting with Windows 8.
    all(target_os = "windows", not(target_vendor = "win7")) => {
        mod windows;
        pub use windows::hostname;
    }
    _ => {
        mod unsupported;
        pub use unsupported::hostname;
    }
}
