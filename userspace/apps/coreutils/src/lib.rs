#![cfg_attr(target_os = "none", no_std)]

#[cfg(target_os = "none")]
extern crate exo_alloc;

#[cfg(target_os = "none")]
pub use exo_rt;

#[cfg(not(any(target_os = "none", target_os = "exo")))]
pub mod host;

#[cfg(not(any(target_os = "none", target_os = "exo")))]
pub use host::{cat, echo, ls, mkdir, rm, rmdir, touch};

#[cfg(any(target_os = "none", target_os = "exo"))]
pub mod bare;

#[cfg(target_os = "none")]
#[macro_export]
macro_rules! exo_command {
    ($run:path) => {
        core::arch::global_asm!(
            ".global _start",
            "_start:",
            "xor rbp, rbp",
            "mov rdi, rsp",
            "and rsp, -16",
            "call {entry}",
            "ud2",
            entry = sym __exo_coreutils_entry,
        );

        #[no_mangle]
        extern "C" fn __exo_coreutils_entry(stack: usize) -> ! {
            unsafe {
                let _ = $crate::exo_rt::exo_rt_init_from_stack(stack as *const usize);
            }
            let args = unsafe { $crate::bare::Args::from_stack(stack) };
            let code = $run(&args);
            $crate::bare::exit(code);
        }
    };
}

#[cfg(target_os = "exo")]
#[macro_export]
macro_rules! exo_command {
    ($run:path) => {
        fn main() {
            use std::os::exo::ffi::OsStrExt;
            let raw_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
            let raw_envs: Vec<Vec<u8>> = std::env::vars_os()
                .map(|(k, v)| {
                    let mut entry = Vec::with_capacity(k.as_bytes().len() + 1 + v.as_bytes().len());
                    entry.extend_from_slice(k.as_bytes());
                    entry.push(b'=');
                    entry.extend_from_slice(v.as_bytes());
                    entry
                })
                .collect();
            let arg_slices: Vec<&[u8]> = raw_args.iter().map(|s| s.as_bytes()).collect();
            let env_slices: Vec<&[u8]> = raw_envs.iter().map(|v| v.as_slice()).collect();
            let args = $crate::bare::Args::from_slices(&arg_slices, &env_slices);
            let code = $run(&args);
            std::process::exit(code);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::host::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmpdir() -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("exo-coreutils-{nonce}"));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn touch_cat_ls_rm_roundtrip() {
        let dir = tmpdir();
        let file = dir.join("a");
        touch(&file).unwrap();
        fs::write(&file, b"hi").unwrap();
        let mut cat_out = Vec::new();
        cat(&file, &mut cat_out).unwrap();
        assert_eq!(cat_out, b"hi");
        let mut ls_out = Vec::new();
        ls(&dir, &mut ls_out).unwrap();
        assert_eq!(String::from_utf8(ls_out).unwrap(), "a\n");
        rm(&file).unwrap();
        fs::remove_dir(&dir).unwrap();
    }
}
