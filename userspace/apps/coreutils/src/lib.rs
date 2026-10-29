#![cfg_attr(any(target_os = "none", target_os = "exo"), no_std)]

#[cfg(any(target_os = "none", target_os = "exo"))]
extern crate exo_alloc;

#[cfg(any(target_os = "none", target_os = "exo"))]
pub use exo_rt;
#[cfg(any(target_os = "none", target_os = "exo"))]
pub use exo_std as std;

#[cfg(not(any(target_os = "none", target_os = "exo")))]
pub mod host;

#[cfg(not(any(target_os = "none", target_os = "exo")))]
pub use host::{cat, echo, ls, mkdir, rm, rmdir, touch};

#[cfg(any(target_os = "none", target_os = "exo"))]
pub mod bare;

#[cfg(any(target_os = "none", target_os = "exo"))]
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
            $crate::std::process::exit(code);
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
