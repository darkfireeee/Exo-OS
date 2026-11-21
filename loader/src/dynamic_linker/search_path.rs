//! Dynamic library path resolution (`LD_LIBRARY_PATH`, `DT_RPATH`, `DT_RUNPATH`, `$ORIGIN`, `/lib`, `/usr/lib`).

use alloc::string::String;
use alloc::vec::Vec;

use crate::syscall::{sys_close, sys_open_ro};

pub const DEFAULT_LIB_DIRS: &[&str] = &["/lib", "/usr/lib"];

/// Returns the parent directory slice of a path (or `"/"` if none).
pub fn dirname_of(path: &str) -> &str {
    if let Some(pos) = path.rfind('/') {
        if pos == 0 {
            "/"
        } else {
            &path[..pos]
        }
    } else {
        "."
    }
}

/// Expands `$ORIGIN` / `${ORIGIN}` inside a rpath/runpath component.
pub fn expand_origin(template: &str, origin_dir: &str) -> String {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"${ORIGIN}") {
            out.push_str(origin_dir);
            i += 9;
        } else if bytes[i..].starts_with(b"$ORIGIN") {
            let next = bytes.get(i + 7).copied();
            if next.is_none() || next == Some(b'/') || next == Some(b':') {
                out.push_str(origin_dir);
                i += 7;
                continue;
            }
            out.push('$');
            i += 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Joins directory prefix and SONAME into a NUL-terminated buffer ready for `sys_open_ro`.
pub fn join_path_cstr(dir: &str, name: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(dir.len() + name.len() + 3);
    if dir == "/" {
        buf.extend_from_slice(b"/");
    } else if !dir.is_empty() {
        buf.extend_from_slice(dir.as_bytes());
        if !dir.ends_with('/') {
            buf.push(b'/');
        }
    }
    buf.extend_from_slice(name.as_bytes());
    buf.push(0);
    buf
}

/// Generates ordered candidate library paths for `name` per ELF SysV rules:
/// 1. Direct path if `name.contains('/')`.
/// 2. `DT_RPATH` (only if `DT_RUNPATH` is absent).
/// 3. `LD_LIBRARY_PATH` (ignored if `secure`).
/// 4. `DT_RUNPATH`.
/// 5. `/lib`, `/usr/lib`.
pub fn collect_candidate_paths(
    name: &str,
    origin_dir: &str,
    rpath: Option<&str>,
    runpath: Option<&str>,
    ld_library_path: Option<&str>,
    secure: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if name.contains('/') {
        out.push(String::from(name));
        return out;
    }

    let push_colon_list = |out: &mut Vec<String>, list: &str, expand: bool| {
        for raw_dir in list.split(':') {
            if raw_dir.is_empty() {
                continue;
            }
            let dir = if expand {
                expand_origin(raw_dir, origin_dir)
            } else {
                String::from(raw_dir)
            };
            let mut full = String::with_capacity(dir.len() + name.len() + 2);
            if dir == "/" {
                full.push('/');
            } else {
                full.push_str(&dir);
                if !dir.ends_with('/') {
                    full.push('/');
                }
            }
            full.push_str(name);
            out.push(full);
        }
    };

    if runpath.is_none() {
        if let Some(rp) = rpath {
            push_colon_list(&mut out, rp, true);
        }
    }
    if !secure {
        if let Some(llp) = ld_library_path {
            push_colon_list(&mut out, llp, false);
        }
    }
    if let Some(rp) = runpath {
        push_colon_list(&mut out, rp, true);
    }
    for sys_dir in DEFAULT_LIB_DIRS {
        let mut full = String::with_capacity(sys_dir.len() + name.len() + 2);
        full.push_str(sys_dir);
        full.push('/');
        full.push_str(name);
        out.push(full);
    }
    out
}

/// Probes candidates on ExoFS and returns `(opened_fd, resolved_path)` for the first match.
pub fn open_library_file(
    name: &str,
    origin_dir: &str,
    rpath: Option<&str>,
    runpath: Option<&str>,
    ld_library_path: Option<&str>,
    secure: bool,
) -> Option<(i32, String)> {
    let candidates =
        collect_candidate_paths(name, origin_dir, rpath, runpath, ld_library_path, secure);
    for path in candidates {
        let mut cstr = Vec::with_capacity(path.len() + 1);
        cstr.extend_from_slice(path.as_bytes());
        cstr.push(0);
        if let Ok(fd) = unsafe { sys_open_ro(cstr.as_ptr()) } {
            if fd >= 0 {
                return Some((fd, path));
            }
            let _ = unsafe { sys_close(fd) };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_origin_and_respects_runpath_precedence() {
        let paths = collect_candidate_paths(
            "libfoo.so",
            "/usr/app",
            Some("$ORIGIN/rpath"),
            Some("${ORIGIN}/runpath:/opt/lib"),
            Some("/tmp/evil:/custom"),
            false,
        );
        // When DT_RUNPATH is set, DT_RPATH must be ignored and LD_LIBRARY_PATH comes before DT_RUNPATH.
        assert_eq!(
            paths,
            alloc::vec![
                "/tmp/evil/libfoo.so",
                "/custom/libfoo.so",
                "/usr/app/runpath/libfoo.so",
                "/opt/lib/libfoo.so",
                "/lib/libfoo.so",
                "/usr/lib/libfoo.so",
            ]
        );
    }

    #[test]
    fn secure_mode_ignores_ld_library_path() {
        let paths = collect_candidate_paths(
            "libbar.so",
            "/bin",
            Some("$ORIGIN/../lib"),
            None,
            Some("/tmp/untrusted"),
            true,
        );
        assert_eq!(
            paths,
            alloc::vec![
                "/bin/../lib/libbar.so",
                "/lib/libbar.so",
                "/usr/lib/libbar.so",
            ]
        );
    }
}
