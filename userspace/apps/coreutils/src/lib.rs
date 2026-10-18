#![cfg_attr(target_os = "none", no_std)]

#[cfg(not(target_os = "none"))]
pub mod host;

#[cfg(not(target_os = "none"))]
pub use host::{cat, echo, ls, mkdir, rm, rmdir, touch};

#[cfg(target_os = "none")]
pub mod bare;

#[cfg(target_os = "none")]
#[macro_export]
macro_rules! exo_command {
    ($run:path) => {
        core::arch::global_asm!(
            ".global _start",
            "_start:",
            "mov rdi, rsp",
            "and rsp, -16",
            "call {entry}",
            entry = sym __exo_coreutils_entry,
        );

        #[no_mangle]
        extern "C" fn __exo_coreutils_entry(stack: usize) -> ! {
            let args = unsafe { $crate::bare::Args::from_stack(stack) };
            let code = $run(&args);
            $crate::bare::exit(code);
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

