//! Native ExoFS object metadata, capabilities & integrity fingerprint reader.

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct ExoFsObjectInfo {
    pub has_native_stat: bool,
    pub size: u64,
    pub mode: u32,
    pub flags: u32,
    pub generation: u64,
    pub read_only_zone: bool,
    pub content_hash_hex: String,
    pub object_id_hex: String,
    pub cap_summary: String,
}

/// Inspect native ExoFS object metadata (`content_hash`, `object_id`, `generation`, capabilities).
pub fn inspect_exofs_object(path: &str) -> ExoFsObjectInfo {
    let read_only_zone = path == "/sbin"
        || path.starts_with("/sbin/")
        || path == "/bin"
        || path.starts_with("/bin/")
        || path == "/lib"
        || path.starts_with("/lib/")
        || path == "/etc"
        || path.starts_with("/etc/");

    let mut info = ExoFsObjectInfo {
        read_only_zone,
        cap_summary: if read_only_zone {
            String::from("Lire • Exécuter (Zone système)")
        } else {
            String::from("Lire • Écrire • Déléguer")
        },
        ..Default::default()
    };

    if let Ok(md) = std::fs::metadata(path) {
        info.size = md.len();
    }

    #[cfg(target_os = "exo")]
    {
        use std::os::exo::ExoFsExt;
        if let Ok(file) = std::fs::File::open(path) {
            if let Ok(st) = file.exofs_stat() {
                info.has_native_stat = true;
                info.size = st.size;
                info.mode = st.mode;
                info.flags = st.flags;
                info.generation = st.generation;
                info.content_hash_hex = hex_prefix(&st.content_hash, 16);
                info.object_id_hex = hex_prefix(&st.object_id, 12);
                return info;
            }
        }
    }

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in path.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    info.content_hash_hex = format!("{:016x}{:016x}", h, h.rotate_left(29) ^ info.size);
    info.object_id_hex = format!("{:016x}", h ^ 0x4558_4F46_5332_0001);
    info.generation = 1;
    info
}

#[cfg(target_os = "exo")]
fn hex_prefix(bytes: &[u8], take: usize) -> String {
    let mut out = String::with_capacity(take * 2);
    for &b in bytes.iter().take(take) {
        use std::fmt::Write;
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}
