#![allow(unused_imports, dead_code)]
use super::io_fmt::{
    push_bytes, push_colored_name, push_hex_bytes, push_hex_u16, push_human_size, push_i64,
    push_u64_padded4, write_all,
};
use super::path_fs::{is_dir_mode, path_len, stat_path};
use crate::bare::types::*;
use exo_syscall_abi as syscall;

    /// Résout un chemin via SYS_EXOFS_PATH_RESOLVE (500).
    pub fn resolve_exofs_path(path: &[u8; PATH_MAX]) -> Option<syscall::ExofsPathResolveResult> {
        let plen = path_len(path);
        if plen == 0 {
            return None;
        }
        let mut resolved = syscall::ExofsPathResolveResult::default();
        let rc = unsafe {
            syscall::exofs_path_resolve_raw(
                path.as_ptr() as u64,
                plen as u64,
                0,
                &mut resolved,
                syscall::EXOFS_RIGHT_READ_ONLY as u64,
            )
        };
        if rc >= 0 {
            Some(resolved)
        } else {
            None
        }
    }

    /// Formate une entrée selon SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md :
    /// `<type>  <rights:rwxlksd>  <flags:<10>  @<token:04x>  ep:<epoch:04>  <size>  <hash:8>  <name>`
    pub fn print_long_entry(
        path: &[u8; PATH_MAX],
        display_name: &[u8],
        human: bool,
        append_slash: bool,
    ) {
        let mut line = [0u8; PATH_MAX + 160];
        let mut len = 0usize;

        let resolved_opt = resolve_exofs_path(path);
        let stat_opt = stat_path(path);

        if resolved_opt.is_none() && stat_opt.is_none() {
            push_bytes(&mut line, &mut len, b"?  -------  .           @0000  ep:----  0         --------  ");
            push_bytes(&mut line, &mut len, display_name);
            push_bytes(&mut line, &mut len, b"\n");
            write_all(STDOUT, &line[..len]);
            return;
        }

        let is_usb = path.starts_with(b"/mnt/usb");
        let is_dir = stat_opt.map(|s| is_dir_mode(s.st_mode)).unwrap_or(append_slash);
        let is_exec = stat_opt.map(|s| (s.st_mode & 0o111) != 0).unwrap_or(false) && !is_usb;

        // 1. Type d'objet ExoFS : d | b | r | s | x | c
        let obj_type = if is_dir {
            b'd'
        } else if let Some(ref res) = resolved_opt {
            match res.object_kind {
                1 => b'd', // Directory
                2 => b'x', // Code exécutable
                3 => b'r', // Relation
                4 => b's', // Secret
                5 => b'c', // Config
                _ => if is_exec { b'x' } else { b'b' },
            }
        } else if is_exec {
            b'x'
        } else {
            b'b'
        };
        push_bytes(&mut line, &mut len, &[obj_type, b' ', b' ']);

        // 2. Rights (7 caractères : rwxlksd)
        let rights: [u8; 7] = if is_dir {
            *b"rw-lk-d"
        } else if is_usb {
            // FS-FAT32-03 : noexec forcé sur USB
            *b"rw----d"
        } else if obj_type == b'x' {
            *b"r-x---d"
        } else if obj_type == b's' {
            *b"r----sd"
        } else {
            *b"rw---sd"
        };
        push_bytes(&mut line, &mut len, &rights);
        push_bytes(&mut line, &mut len, b"  ");

        // 3. Flags sur 10 colonnes
        let flags_str: &[u8; 10] = if is_usb {
            b"[usb]     "
        } else if let Some(ref res) = resolved_opt {
            if (res.flags & 0x01) != 0 {
                b"[sealed]  "
            } else if obj_type == b'x' {
                b"[sig ok]  "
            } else if obj_type == b's' {
                b"[enc]     "
            } else {
                b".         "
            }
        } else {
            b".         "
        };
        push_bytes(&mut line, &mut len, flags_str);
        push_bytes(&mut line, &mut len, b"  @");

        // 4. Jeton Capability (@<token:04x>)
        let token_u16 = if let Some(ref res) = resolved_opt {
            u16::from_le_bytes([res.object_id[0], res.object_id[1]])
        } else if let Some(ref st) = stat_opt {
            (st.st_ino & 0xFFFF) as u16
        } else {
            0
        };
        push_hex_u16(&mut line, &mut len, token_u16);

        // 5. Epoch (ep:<epoch:04> ou ep:---- sur FAT32/pseudo-FS)
        push_bytes(&mut line, &mut len, b"  ep:");
        if let Some(ref res) = resolved_opt {
            if !is_usb {
                push_u64_padded4(&mut line, &mut len, res.epoch_id.max(1));
            } else {
                push_bytes(&mut line, &mut len, b"----");
            }
        } else {
            push_bytes(&mut line, &mut len, b"----");
        }
        push_bytes(&mut line, &mut len, b"  ");

        // 6. Taille
        let size_val = resolved_opt
            .map(|r| r.size_bytes as i64)
            .or_else(|| stat_opt.map(|s| s.st_size))
            .unwrap_or(0);
        let size_start = len;
        if human {
            push_human_size(&mut line, &mut len, size_val);
        } else {
            push_i64(&mut line, &mut len, size_val);
        }
        let size_written = len - size_start;
        let mut pad = 0usize;
        while size_written + pad < 8 {
            push_bytes(&mut line, &mut len, b" ");
            pad += 1;
        }
        push_bytes(&mut line, &mut len, b"  ");

        // 7. Content Hash BLAKE3 tronqué à 8 hex (-------- pour répertoire ou Secret S-09)
        if obj_type == b'd' || obj_type == b's' {
            push_bytes(&mut line, &mut len, b"--------");
        } else if let Some(ref res) = resolved_opt {
            if res.blob_id != [0u8; 32] {
                push_hex_bytes(&mut line, &mut len, &res.blob_id[..4]);
            } else {
                push_bytes(&mut line, &mut len, b"00000000");
            }
        } else {
            push_bytes(&mut line, &mut len, b"--------");
        }
        push_bytes(&mut line, &mut len, b"  ");

        // 8. Nom coloré
        let color_mode = if is_dir {
            S_IFDIR | 0o755
        } else if obj_type == b'x' {
            S_IFREG | 0o755
        } else {
            S_IFREG | 0o644
        };
        push_colored_name(&mut line, &mut len, display_name, color_mode, append_slash);
        push_bytes(&mut line, &mut len, b"\n");

        write_all(STDOUT, &line[..len]);
    }

