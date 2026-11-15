use pkg_config::Config;

fn main() {
    if std::env::var_os("CARGO_FEATURE_DLOPEN").is_some()
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("exo")
    {
        // Do not link to anything
        return;
    }

    if std::env::var_os("CARGO_FEATURE_CLIENT").is_some() {
        Config::new().probe("wayland-client").unwrap();
    }
    if std::env::var_os("CARGO_FEATURE_CURSOR").is_some() {
        Config::new().probe("wayland-cursor").unwrap();
    }
    if std::env::var_os("CARGO_FEATURE_EGL").is_some() {
        Config::new().probe("wayland-egl").unwrap();
    }
    if std::env::var_os("CARGO_FEATURE_SERVER").is_some() {
        Config::new().probe("wayland-server").unwrap();
    }
}
