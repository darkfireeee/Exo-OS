//! Tests unitaires du pont FS
use super::*;
use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};

#[cfg(test)]
mod tests {
    use super::*;

    fn init_bridge() {
        unsafe {
            fs_bridge_init();
        }
        OBJECT_TABLE.reset_all();
        BLOB_CACHE.flush_all_force();
    }

    #[test]
    fn test_fs_bridge_open_write_read_lseek_roundtrip() {
        init_bridge();

        let path = b"/etc/exo/kernel.toml";
        let payload = *b"exo-kernel-config";
        let mut read_back = [0u8; 17];

        let fd = fs_open(path, open_flags::O_RDWR | open_flags::O_CREAT, 0, 7).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 7).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(fd, 0, SEEK_SET, 7).unwrap(), 0);
        assert_eq!(
            fs_read(fd, read_back.as_mut_ptr() as u64, read_back.len(), 7).unwrap(),
            read_back.len() as i64
        );
        assert_eq!(&read_back, &payload);
        assert_eq!(fs_close(fd, 7).unwrap(), 0);
    }

    #[test]
    fn test_fs_bridge_roundtrip_stress() {
        init_bridge();

        for idx in 0..512u32 {
            let mut path = [0u8; 32];
            let prefix_len = 8;
            path[..prefix_len].copy_from_slice(b"/stress/");
            let suffix_len = write_u32_hex(&mut path[prefix_len..], idx);

            let write_len = (idx as usize % 48) + 1;
            let mut payload = [0u8; 64];
            for (off, byte) in payload[..write_len].iter_mut().enumerate() {
                *byte = idx.wrapping_add(off as u32) as u8;
            }

            let fd = fs_open(
                &path[..prefix_len + suffix_len],
                open_flags::O_RDWR | open_flags::O_CREAT | open_flags::O_TRUNC,
                0,
                11,
            )
            .unwrap() as u32;
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, write_len, 11).unwrap(),
                write_len as i64
            );
            assert_eq!(fs_lseek(fd, 0, SEEK_SET, 11).unwrap(), 0);

            let mut out = [0u8; 64];
            assert_eq!(
                fs_read(fd, out.as_mut_ptr() as u64, write_len, 11).unwrap(),
                write_len as i64
            );
            assert_eq!(&out[..write_len], &payload[..write_len]);
            assert_eq!(fs_close(fd, 11).unwrap(), 0);
        }
    }

    #[test]
    fn test_fs_openat_at_fdcwd_aliases_open() {
        init_bridge();

        let path = b"relative/exo.conf";
        let fd = fs_openat(
            AT_FDCWD,
            path,
            open_flags::O_RDWR | open_flags::O_CREAT,
            0,
            17,
        )
        .unwrap() as u32;
        assert!(fd >= crate::fs::exofs::syscall::object_fd::FD_RESERVED);
        assert_eq!(fs_close(fd, 17).unwrap(), 0);
    }

    #[test]
    fn test_fs_openat_relative_dirfd_is_rejected() {
        init_bridge();

        let err = fs_openat(9, b"relative/exo.conf", open_flags::O_RDONLY, 0, 17).unwrap_err();
        assert_eq!(err, FsBridgeError::Invalid);
    }

    #[test]
    fn test_fs_open_accepts_linux_descriptor_flags_as_noops() {
        init_bridge();

        let fd = fs_open(
            b"/flags/nonblock.txt",
            open_flags::O_CREAT | open_flags::O_RDWR | O_NONBLOCK | O_CLOEXEC,
            0,
            18,
        )
        .unwrap() as u32;
        assert_eq!(fs_close(fd, 18).unwrap(), 0);
    }

    #[test]
    fn test_fs_stat_and_fstat_report_size_and_kind() {
        init_bridge();

        let dir_path = b"/var/exo";
        assert_eq!(fs_mkdir(dir_path, 0, 21).unwrap(), 0);

        let mut dir_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(dir_path, &mut dir_stat as *mut _ as u64, 21).unwrap(),
            0
        );
        assert_eq!(
            dir_stat.st_mode & STAT_MODE_MASK,
            STAT_MODE_DIR & STAT_MODE_MASK
        );

        let file_path = b"/var/exo/runtime.cfg";
        let payload = *b"exo-runtime";
        let fd =
            fs_open(file_path, open_flags::O_RDWR | open_flags::O_CREAT, 0, 21).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 21).unwrap(),
            payload.len() as i64
        );

        let mut path_stat = LinuxStat::default();
        let mut fd_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(file_path, &mut path_stat as *mut _ as u64, 21).unwrap(),
            0
        );
        assert_eq!(fs_fstat(fd, &mut fd_stat as *mut _ as u64, 21).unwrap(), 0);
        assert_eq!(path_stat.st_size, payload.len() as i64);
        assert_eq!(fd_stat.st_size, payload.len() as i64);
        assert_eq!(
            fd_stat.st_mode & STAT_MODE_MASK,
            STAT_MODE_FILE & STAT_MODE_MASK
        );
        assert_eq!(fs_close(fd, 21).unwrap(), 0);
    }

    #[test]
    fn test_fs_mkdir_rmdir_unlink_roundtrip() {
        init_bridge();

        let dir_path = b"/srv/exo";
        assert_eq!(fs_mkdir(dir_path, 0, 29).unwrap(), 0);
        assert_eq!(fs_unlink(dir_path, 29).unwrap_err(), FsBridgeError::IsDir);
        assert_eq!(fs_rmdir(dir_path, 29).unwrap(), 0);
        let mut dir_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(dir_path, &mut dir_stat as *mut _ as u64, 29).unwrap_err(),
            FsBridgeError::NotFound
        );

        let file_path = b"/srv/exo.log";
        let fd =
            fs_open(file_path, open_flags::O_CREAT | open_flags::O_RDWR, 0, 29).unwrap() as u32;
        assert_eq!(fs_close(fd, 29).unwrap(), 0);
        assert_eq!(fs_unlink(file_path, 29).unwrap(), 0);
        let mut file_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(file_path, &mut file_stat as *mut _ as u64, 29).unwrap_err(),
            FsBridgeError::NotFound
        );
    }

    #[test]
    fn test_fs_directory_lifecycle_stress() {
        init_bridge();

        for idx in 0..256u32 {
            let mut dir_path = [0u8; 40];
            let dir_prefix = b"/dirs/";
            dir_path[..dir_prefix.len()].copy_from_slice(dir_prefix);
            let dir_len = dir_prefix.len() + write_u32_hex(&mut dir_path[dir_prefix.len()..], idx);

            assert_eq!(fs_mkdir(&dir_path[..dir_len], 0, 42).unwrap(), 0);
            let mut dir_stat = LinuxStat::default();
            assert_eq!(
                fs_stat(&dir_path[..dir_len], &mut dir_stat as *mut _ as u64, 42).unwrap(),
                0
            );
            assert_eq!(
                dir_stat.st_mode & STAT_MODE_MASK,
                STAT_MODE_DIR & STAT_MODE_MASK
            );
            assert_eq!(fs_rmdir(&dir_path[..dir_len], 42).unwrap(), 0);

            let mut file_path = [0u8; 48];
            let file_prefix = b"/files/";
            file_path[..file_prefix.len()].copy_from_slice(file_prefix);
            let file_len =
                file_prefix.len() + write_u32_hex(&mut file_path[file_prefix.len()..], idx);
            let fd = fs_open(
                &file_path[..file_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                42,
            )
            .unwrap() as u32;
            assert_eq!(fs_close(fd, 42).unwrap(), 0);
            assert_eq!(fs_unlink(&file_path[..file_len], 42).unwrap(), 0);
        }
    }

    fn parse_dirent_names(buf: &[u8]) -> Vec<Vec<u8>> {
        let mut names = Vec::new();
        let mut off = 0usize;
        while off + DIRENT64_HEADER_SIZE <= buf.len() {
            let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
            if reclen == 0 || off + reclen > buf.len() {
                break;
            }
            let name_start = off + DIRENT64_HEADER_SIZE;
            let name_end = off + reclen;
            let mut cursor = name_start;
            while cursor < name_end && buf[cursor] != 0 {
                cursor += 1;
            }
            names.push(buf[name_start..cursor].to_vec());
            off += reclen;
        }
        names
    }

    #[test]
    fn test_fs_getdents64_reports_created_children() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/etc", 0, 51).unwrap(), 0);
        let file_fd = fs_open(
            b"/etc/hosts",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            51,
        )
        .unwrap() as u32;
        assert_eq!(fs_close(file_fd, 51).unwrap(), 0);

        let dir_fd = fs_open(b"/etc", open_flags::O_RDONLY, 0, 51).unwrap() as u32;
        let mut out = [0u8; 256];
        let n = fs_getdents64(dir_fd, out.as_mut_ptr() as u64, out.len(), 51).unwrap() as usize;
        let names = parse_dirent_names(&out[..n]);
        assert!(names.iter().any(|name| name.as_slice() == b"hosts"));
        assert_eq!(fs_close(dir_fd, 51).unwrap(), 0);
    }

    #[test]
    fn test_fs_shell_minimal_command_graph() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/auto/chain", 0, 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/auto/chain", 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/auto", 91).unwrap(), 0);

        assert_eq!(fs_mkdir(b"/tmp", 0, 91).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/tmp/t", 0, 91).unwrap(), 0);

        let fd = fs_open(
            b"/tmp/t/a",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            91,
        )
        .unwrap() as u32;
        let payload = *b"hello-terminal";
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 91).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(fd, 0, SEEK_SET, 91).unwrap(), 0);
        let mut out = [0u8; 32];
        let n = fs_read(fd, out.as_mut_ptr() as u64, out.len(), 91).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(fd, 91).unwrap(), 0);

        let dir_fd = fs_open(b"/tmp/t", open_flags::O_RDONLY, 0, 91).unwrap() as u32;
        let mut dirents = [0u8; 256];
        let n =
            fs_getdents64(dir_fd, dirents.as_mut_ptr() as u64, dirents.len(), 91).unwrap() as usize;
        let names = parse_dirent_names(&dirents[..n]);
        assert!(names.iter().any(|name| name.as_slice() == b"a"));
        assert_eq!(fs_close(dir_fd, 91).unwrap(), 0);

        let copy_fd = fs_open(
            b"/tmp/t/copy",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            91,
        )
        .unwrap() as u32;
        assert_eq!(
            fs_write(copy_fd, payload.as_ptr() as u64, payload.len(), 91).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_close(copy_fd, 91).unwrap(), 0);
        assert_eq!(fs_rename(b"/tmp/t/copy", b"/tmp/t/moved", 91).unwrap(), 0);

        let moved_fd = fs_open(b"/tmp/t/moved", open_flags::O_RDONLY, 0, 91).unwrap() as u32;
        let mut moved = [0u8; 32];
        let n = fs_read(moved_fd, moved.as_mut_ptr() as u64, moved.len(), 91).unwrap() as usize;
        assert_eq!(&moved[..n], &payload);
        assert_eq!(fs_close(moved_fd, 91).unwrap(), 0);

        assert_eq!(fs_unlink(b"/tmp/t/a", 91).unwrap(), 0);
        assert_eq!(fs_unlink(b"/tmp/t/moved", 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/tmp/t", 91).unwrap(), 0);
        assert_eq!(fs_rmdir(b"/tmp", 91).unwrap(), 0);
    }

    #[test]
    fn test_fs_getdents64_stress_tracks_many_children() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/stressdir", 0, 52).unwrap(), 0);
        for idx in 0..64u32 {
            let mut path = [0u8; 32];
            let prefix = b"/stressdir/f";
            path[..prefix.len()].copy_from_slice(prefix);
            let len = prefix.len() + write_u32_hex(&mut path[prefix.len()..], idx);
            let fd = fs_open(
                &path[..len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                52,
            )
            .unwrap() as u32;
            assert_eq!(fs_close(fd, 52).unwrap(), 0);
        }

        let dir_fd = fs_open(b"/stressdir", open_flags::O_RDONLY, 0, 52).unwrap() as u32;
        let mut out = [0u8; 4096];
        let n = fs_getdents64(dir_fd, out.as_mut_ptr() as u64, out.len(), 52).unwrap() as usize;
        let names = parse_dirent_names(&out[..n]);
        assert_eq!(names.len(), 64);
        assert!(names.iter().any(|name| name.as_slice() == b"f0"));
        assert!(names.iter().any(|name| name.as_slice() == b"f3f"));
        assert_eq!(fs_close(dir_fd, 52).unwrap(), 0);
    }

    #[test]
    fn test_fs_symlink_roundtrip_and_following() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/opt", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/opt/bin", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/usr", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/usr/bin", 0, 61).unwrap(), 0);

        let payload = *b"exo-shell";
        let target_path = b"/opt/bin/exo";
        let fd = fs_open(
            target_path,
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            61,
        )
        .unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 61).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_close(fd, 61).unwrap(), 0);

        let target = b"../../opt/bin/exo";
        assert_eq!(fs_symlink(target, b"/usr/bin/exo", 61).unwrap(), 0);

        let mut readlink_out = [0u8; 32];
        let readlink_len = fs_readlink(
            b"/usr/bin/exo",
            readlink_out.as_mut_ptr() as u64,
            readlink_out.len(),
            61,
        )
        .unwrap() as usize;
        assert_eq!(&readlink_out[..readlink_len], target);

        let fd = fs_open(b"/usr/bin/exo", open_flags::O_RDONLY, 0, 61).unwrap() as u32;
        let mut out = [0u8; 16];
        let n = fs_read(fd, out.as_mut_ptr() as u64, out.len(), 61).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(fd, 61).unwrap(), 0);

        let mut lstat_buf = LinuxStat::default();
        let mut stat_buf = LinuxStat::default();
        assert_eq!(
            fs_lstat(b"/usr/bin/exo", &mut lstat_buf as *mut _ as u64, 61).unwrap(),
            0
        );
        assert_eq!(
            fs_stat(b"/usr/bin/exo", &mut stat_buf as *mut _ as u64, 61).unwrap(),
            0
        );
        assert_eq!(
            lstat_buf.st_mode & STAT_MODE_MASK,
            STAT_MODE_SYMLINK & STAT_MODE_MASK
        );
        assert_eq!(
            stat_buf.st_mode & STAT_MODE_MASK,
            STAT_MODE_FILE & STAT_MODE_MASK
        );
    }

    #[test]
    fn test_fs_symlink_stress_many_relative_links() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/targets", 0, 62).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/links", 0, 62).unwrap(), 0);

        for idx in 0..96u32 {
            let mut target_path = [0u8; 32];
            let target_prefix = b"/targets/t";
            target_path[..target_prefix.len()].copy_from_slice(target_prefix);
            let target_len =
                target_prefix.len() + write_u32_hex(&mut target_path[target_prefix.len()..], idx);

            let fd = fs_open(
                &target_path[..target_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                62,
            )
            .unwrap() as u32;
            let payload = [idx as u8; 4];
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, payload.len(), 62).unwrap(),
                payload.len() as i64
            );
            assert_eq!(fs_close(fd, 62).unwrap(), 0);

            let mut link_path = [0u8; 32];
            let link_prefix = b"/links/l";
            link_path[..link_prefix.len()].copy_from_slice(link_prefix);
            let link_len =
                link_prefix.len() + write_u32_hex(&mut link_path[link_prefix.len()..], idx);

            let mut target_rel = [0u8; 24];
            let rel_prefix = b"../targets/t";
            target_rel[..rel_prefix.len()].copy_from_slice(rel_prefix);
            let rel_len =
                rel_prefix.len() + write_u32_hex(&mut target_rel[rel_prefix.len()..], idx);

            assert_eq!(
                fs_symlink(&target_rel[..rel_len], &link_path[..link_len], 62).unwrap(),
                0
            );

            let fd = fs_open(&link_path[..link_len], open_flags::O_RDONLY, 0, 62).unwrap() as u32;
            let mut out = [0u8; 4];
            assert_eq!(
                fs_read(fd, out.as_mut_ptr() as u64, out.len(), 62).unwrap(),
                out.len() as i64
            );
            assert_eq!(out, [idx as u8; 4]);
            assert_eq!(fs_close(fd, 62).unwrap(), 0);
        }
    }

    #[test]
    fn test_fs_truncate_and_ftruncate_resize_regular_file() {
        init_bridge();

        let path = b"/truncate/data.bin";
        let payload = *b"abcdef";
        let fd = fs_open(path, open_flags::O_CREAT | open_flags::O_RDWR, 0, 71).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 71).unwrap(),
            payload.len() as i64
        );

        assert_eq!(fs_truncate(path, 3, 71).unwrap(), 0);
        let mut stat_buf = LinuxStat::default();
        assert_eq!(
            fs_stat(path, &mut stat_buf as *mut _ as u64, 71).unwrap(),
            0
        );
        assert_eq!(stat_buf.st_size, 3);

        assert_eq!(fs_ftruncate(fd, 8, 71).unwrap(), 0);
        assert_eq!(fs_lseek(fd, 0, SEEK_SET, 71).unwrap(), 0);
        let mut out = [0xFFu8; 8];
        assert_eq!(
            fs_read(fd, out.as_mut_ptr() as u64, out.len(), 71).unwrap(),
            out.len() as i64
        );
        assert_eq!(&out[..3], b"abc");
        assert_eq!(&out[3..], &[0, 0, 0, 0, 0]);
        assert_eq!(fs_close(fd, 71).unwrap(), 0);
    }

    #[test]
    fn test_fs_truncate_stress_shrink_and_grow_many_files() {
        init_bridge();

        for idx in 0..96u32 {
            let mut path = [0u8; 40];
            let prefix = b"/trunc/f";
            path[..prefix.len()].copy_from_slice(prefix);
            let path_len = prefix.len() + write_u32_hex(&mut path[prefix.len()..], idx);

            let mut payload = [0u8; 24];
            let write_len = 8 + (idx as usize % 16);
            for (off, byte) in payload[..write_len].iter_mut().enumerate() {
                *byte = idx.wrapping_add(off as u32) as u8;
            }

            let fd = fs_open(
                &path[..path_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                72,
            )
            .unwrap() as u32;
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, write_len, 72).unwrap(),
                write_len as i64
            );

            let shrink_len = (idx as u64 % 5) + 1;
            assert_eq!(fs_truncate(&path[..path_len], shrink_len, 72).unwrap(), 0);
            let grow_len = shrink_len + 9;
            assert_eq!(fs_ftruncate(fd, grow_len, 72).unwrap(), 0);

            let mut stat_buf = LinuxStat::default();
            assert_eq!(
                fs_stat(&path[..path_len], &mut stat_buf as *mut _ as u64, 72).unwrap(),
                0
            );
            assert_eq!(stat_buf.st_size, grow_len as i64);
            assert_eq!(fs_close(fd, 72).unwrap(), 0);
        }
    }

    #[test]
    fn test_fs_bridge_large_sequential_write_128_mib() {
        init_bridge();

        let fd = fs_open(
            b"/bench/seq128",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            73,
        )
        .unwrap() as u32;
        let block = alloc::vec![0xA5u8; 1024 * 1024];

        let mut idx = 0usize;
        while idx < 128 {
            assert_eq!(
                fs_write(fd, block.as_ptr() as u64, block.len(), 73).unwrap(),
                block.len() as i64
            );
            idx += 1;
        }

        let mut stat = LinuxStat::default();
        assert_eq!(fs_fstat(fd, &mut stat as *mut _ as u64, 73).unwrap(), 0);
        assert_eq!(stat.st_size, 128 * 1024 * 1024);
        assert_eq!(
            fs_lseek(fd, 127 * 1024 * 1024, SEEK_SET, 73).unwrap(),
            127 * 1024 * 1024
        );

        let mut tail = [0u8; 4096];
        assert_eq!(
            fs_read(fd, tail.as_mut_ptr() as u64, tail.len(), 73).unwrap(),
            tail.len() as i64
        );
        assert!(tail.iter().all(|byte| *byte == 0xA5));
        assert_eq!(fs_close(fd, 73).unwrap(), 0);
    }

    #[test]
    fn test_fs_bridge_ftruncate_sparse_extends_128_mib() {
        init_bridge();

        let fd = fs_open(
            b"/bench/sparse128",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            74,
        )
        .unwrap() as u32;

        assert_eq!(fs_ftruncate(fd, 128 * 1024 * 1024, 74).unwrap(), 0);

        let mut stat = LinuxStat::default();
        assert_eq!(fs_fstat(fd, &mut stat as *mut _ as u64, 74).unwrap(), 0);
        assert_eq!(stat.st_size, 128 * 1024 * 1024);

        assert_eq!(
            fs_lseek(fd, 127 * 1024 * 1024, SEEK_SET, 74).unwrap(),
            127 * 1024 * 1024
        );
        let mut tail = [0xFFu8; 4096];
        assert_eq!(
            fs_read(fd, tail.as_mut_ptr() as u64, tail.len(), 74).unwrap(),
            tail.len() as i64
        );
        assert!(tail.iter().all(|byte| *byte == 0));
        assert_eq!(fs_close(fd, 74).unwrap(), 0);
    }

    #[test]
    fn test_fs_pipe2_eventfd_and_poll_roundtrip() {
        init_bridge();

        let mut fds = [0i32; 2];
        assert_eq!(fs_pipe2(fds.as_mut_ptr() as u64, 0, 81).unwrap(), 0);

        let payload = *b"pipe-data";
        assert_eq!(
            fs_write(fds[1] as u32, payload.as_ptr() as u64, payload.len(), 81).unwrap(),
            payload.len() as i64
        );

        let mut pollfd = LinuxPollFd {
            fd: fds[0],
            events: POLLIN,
            revents: 0,
        };
        assert_eq!(fs_poll(&mut pollfd as *mut _ as u64, 1, 0, 81).unwrap(), 1);
        assert_ne!(pollfd.revents & POLLIN, 0);

        let mut out = [0u8; 16];
        let n = fs_read(fds[0] as u32, out.as_mut_ptr() as u64, out.len(), 81).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(fds[0] as u32, 81).unwrap(), 0);
        assert_eq!(fs_close(fds[1] as u32, 81).unwrap(), 0);

        let event_fd = fs_eventfd2(2, 0, 81).unwrap() as u32;
        let add = 3u64;
        assert_eq!(
            fs_write(event_fd, &add as *const _ as u64, size_of::<u64>(), 81).unwrap(),
            size_of::<u64>() as i64
        );
        let mut value = 0u64;
        assert_eq!(
            fs_read(event_fd, &mut value as *mut _ as u64, size_of::<u64>(), 81).unwrap(),
            size_of::<u64>() as i64
        );
        assert_eq!(value, 5);
        assert_eq!(fs_close(event_fd, 81).unwrap(), 0);
    }

    #[test]
    fn test_fs_link_and_inotify_compat_descriptors() {
        init_bridge();

        let fd = fs_open(
            b"/links/source.txt",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            83,
        )
        .unwrap() as u32;
        let payload = *b"hard-link";
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 83).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_close(fd, 83).unwrap(), 0);

        assert_eq!(
            fs_link(b"/links/source.txt", b"/links/alias.txt", 83).unwrap(),
            0
        );
        let alias_fd = fs_open(b"/links/alias.txt", open_flags::O_RDONLY, 0, 83).unwrap() as u32;
        let mut out = [0u8; 16];
        let n = fs_read(alias_fd, out.as_mut_ptr() as u64, out.len(), 83).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(alias_fd, 83).unwrap(), 0);

        let inotify_fd = fs_inotify_init1(O_NONBLOCK | O_CLOEXEC, 83).unwrap() as u32;
        let mut pollfd = LinuxPollFd {
            fd: inotify_fd as i32,
            events: POLLIN,
            revents: 0,
        };
        assert_eq!(fs_poll(&mut pollfd as *mut _ as u64, 1, 0, 83).unwrap(), 0);
        assert_eq!(fs_close(inotify_fd, 83).unwrap(), 0);
    }

    #[test]
    fn test_fs_phase2_compat_splice_vmsplice_socketpair_mknod() {
        init_bridge();

        assert_eq!(
            fs_mknod(b"/nodes/fifo0", S_IFIFO | 0o644, 0, 84).unwrap(),
            0
        );
        let mut stat = LinuxStat::default();
        assert_eq!(
            fs_stat(b"/nodes/fifo0", &mut stat as *mut _ as u64, 84).unwrap(),
            0
        );

        let mut pipe_a = [0i32; 2];
        let mut pipe_b = [0i32; 2];
        assert_eq!(fs_pipe2(pipe_a.as_mut_ptr() as u64, 0, 84).unwrap(), 0);
        assert_eq!(fs_pipe2(pipe_b.as_mut_ptr() as u64, 0, 84).unwrap(), 0);

        let vm_payload = *b"vmsplice";
        let iov = LinuxIovec {
            iov_base: vm_payload.as_ptr() as u64,
            iov_len: vm_payload.len() as u64,
        };
        assert_eq!(
            fs_vmsplice(pipe_a[1] as u32, &iov as *const _ as u64, 1, 0, 84).unwrap(),
            vm_payload.len() as i64
        );
        assert_eq!(
            fs_tee(pipe_a[0] as u32, pipe_b[1] as u32, vm_payload.len(), 0, 84).unwrap(),
            vm_payload.len() as i64
        );

        let mut tee_out = [0u8; 16];
        let n = fs_read(
            pipe_b[0] as u32,
            tee_out.as_mut_ptr() as u64,
            tee_out.len(),
            84,
        )
        .unwrap() as usize;
        assert_eq!(&tee_out[..n], &vm_payload);

        let file_fd = fs_open(
            b"/splice/out.bin",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            84,
        )
        .unwrap() as u32;
        assert_eq!(
            fs_splice(pipe_a[0] as u32, 0, file_fd, 0, vm_payload.len(), 0, 84).unwrap(),
            vm_payload.len() as i64
        );
        assert_eq!(fs_lseek(file_fd, 0, SEEK_SET, 84).unwrap(), 0);
        let mut file_out = [0u8; 16];
        let n =
            fs_read(file_fd, file_out.as_mut_ptr() as u64, file_out.len(), 84).unwrap() as usize;
        assert_eq!(&file_out[..n], &vm_payload);

        let mut sv = [0i32; 2];
        assert_eq!(
            fs_socketpair(1, 1, 0, sv.as_mut_ptr() as u64, 84).unwrap(),
            0
        );
        let sock_payload = *b"sock";
        assert_eq!(
            fs_write(
                sv[0] as u32,
                sock_payload.as_ptr() as u64,
                sock_payload.len(),
                84
            )
            .unwrap(),
            sock_payload.len() as i64
        );
        let mut sock_out = [0u8; 8];
        let n = fs_read(
            sv[1] as u32,
            sock_out.as_mut_ptr() as u64,
            sock_out.len(),
            84,
        )
        .unwrap() as usize;
        assert_eq!(&sock_out[..n], &sock_payload);

        assert_eq!(fs_close(file_fd, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_a[0] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_a[1] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_b[0] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(pipe_b[1] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(sv[0] as u32, 84).unwrap(), 0);
        assert_eq!(fs_close(sv[1] as u32, 84).unwrap(), 0);
    }

    #[test]
    fn test_fs_copy_range_sendfile_statx_and_cwd_compat() {
        init_bridge();

        let src = fs_open(
            b"/copy/src.bin",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            82,
        )
        .unwrap() as u32;
        let dst = fs_open(
            b"/copy/dst.bin",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            82,
        )
        .unwrap() as u32;
        let payload = *b"translation-layer";
        assert_eq!(
            fs_write(src, payload.as_ptr() as u64, payload.len(), 82).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(src, 0, SEEK_SET, 82).unwrap(), 0);
        assert_eq!(
            fs_copy_file_range(src, 0, dst, 0, payload.len(), 0, 82).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(dst, 0, SEEK_SET, 82).unwrap(), 0);
        let mut out = [0u8; 32];
        let n = fs_read(dst, out.as_mut_ptr() as u64, payload.len(), 82).unwrap() as usize;
        assert_eq!(&out[..n], &payload);

        let mut statx = LinuxStatx::default();
        assert_eq!(
            fs_statx(
                AT_FDCWD,
                b"/copy/dst.bin",
                0,
                0,
                &mut statx as *mut _ as u64,
                82
            )
            .unwrap(),
            0
        );
        assert_eq!(statx.stx_size, payload.len() as u64);

        assert_eq!(fs_fallocate(dst, 0, 0, 64, 82).unwrap(), 0);
        let mut stat = LinuxStat::default();
        assert_eq!(fs_fstat(dst, &mut stat as *mut _ as u64, 82).unwrap(), 0);
        assert_eq!(stat.st_size, 64);

        let mut cwd = [0u8; 4];
        assert_eq!(
            fs_getcwd(cwd.as_mut_ptr() as u64, cwd.len(), 82).unwrap(),
            2
        );
        assert_eq!(&cwd[..2], b"/\0");
        assert_eq!(
            fs_sync_file_range(dst, 0, 64, SYNC_FILE_RANGE_WRITE, 82).unwrap(),
            0
        );
        assert_eq!(fs_close(src, 82).unwrap(), 0);
        assert_eq!(fs_close(dst, 82).unwrap(), 0);
    }

    fn write_u32_hex(dst: &mut [u8], value: u32) -> usize {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut shift = 28u32;
        let mut out = 0usize;
        let mut started = false;

        loop {
            let nibble = ((value >> shift) & 0xF) as usize;
            if nibble != 0 || started || shift == 0 {
                dst[out] = HEX[nibble];
                out += 1;
                started = true;
            }
            if shift == 0 {
                break;
            }
            shift -= 4;
        }

        out
    }
}
