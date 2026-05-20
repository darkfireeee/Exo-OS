use super::support::{
    close_fd, install_mock_disk, open_path, open_rdwr, read_at, reset_state, write_at,
};
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::posix_bridge::vfs_compat::{
    file_mode, open_flags as vfs_open_flags, register_exofs_vfs_ops, reset_vfs_state_for_test,
    root_inode, vfs_close, vfs_create, vfs_lookup, vfs_mkdir, vfs_open, vfs_read, vfs_readdir,
    vfs_rename, vfs_rmdir, vfs_unlink, vfs_write,
};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use std::vec::Vec;

fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
    match res {
        Ok(value) => value,
        Err(err) => panic!("unexpected error: {err:?}"),
    }
}

fn make_payload(seed: u8, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0usize;
    while i < len {
        out.push(seed.wrapping_add(((i * 19) % 251) as u8));
        i = i.wrapping_add(1);
    }
    out
}

#[test]
fn virtio_backed_syscall_workflow_survives_cold_cache_appends() {
    reset_state();
    let _disk = install_mock_disk(512, 32_768);

    let mut paths = Vec::new();
    let mut expected = Vec::new();

    let mut i = 0usize;
    while i < 16 {
        let path = format!("/virtio/workflow/{i}");
        let payload = make_payload((0x10 + i) as u8, 256 + (i * 37));
        let fd = open_rdwr(&path);
        assert_eq!(write_at(fd, &payload, 0), payload.len());
        close_fd(fd);

        paths.push(path);
        expected.push(payload);
        i = i.wrapping_add(1);
    }

    let _ = BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let mut j = 0usize;
    while j < paths.len() {
        let fd = open_rdwr(&paths[j]);
        let suffix = make_payload((0x80 + j) as u8, 33 + j);
        let size = ok(OBJECT_TABLE.get(fd)).size;
        assert_eq!(write_at(fd, &suffix, size), suffix.len());
        expected[j].extend_from_slice(&suffix);
        close_fd(fd);
        j = j.wrapping_add(1);
    }

    let _ = BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let mut k = 0usize;
    while k < paths.len() {
        let fd = open_rdwr(&paths[k]);
        let size = ok(OBJECT_TABLE.get(fd)).size as usize;
        let read_back = read_at(fd, size, 0);
        assert_eq!(read_back, expected[k]);
        close_fd(fd);
        k = k.wrapping_add(1);
    }

    reset_state();
}

#[test]
fn virtio_backed_truncate_is_visible_after_reopen() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let path = "/virtio/truncate/visible";
    let payload = make_payload(0xA4, 1024);

    let fd = open_rdwr(path);
    assert_eq!(write_at(fd, &payload, 0), payload.len());
    close_fd(fd);

    let _ = BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let trunc_fd = open_path(path, open_flags::O_RDWR | open_flags::O_TRUNC);
    assert_eq!(ok(OBJECT_TABLE.get(trunc_fd)).size, 0);
    close_fd(trunc_fd);

    let _ = BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let reopened = open_rdwr(path);
    assert_eq!(ok(OBJECT_TABLE.get(reopened)).size, 0);
    let read_back = read_at(reopened, 64, 0);
    assert!(read_back.is_empty());
    close_fd(reopened);

    reset_state();
}

#[test]
fn vfs_bridge_tracks_names_and_persists_file_content() {
    reset_state();
    reset_vfs_state_for_test();
    let _disk = install_mock_disk(512, 16_384);

    ok(register_exofs_vfs_ops());
    let root = root_inode();
    let docs = ok(vfs_mkdir(root, b"docs", file_mode::DEFAULT_DIR, 0));
    let note = ok(vfs_create(docs, b"note.txt", file_mode::DEFAULT_FILE, 1000));
    assert_eq!(ok(vfs_lookup(docs, b"note.txt")), note);

    let payload = make_payload(0x52, 912);
    let fd = ok(vfs_open(note, vfs_open_flags::O_RDWR, 77));
    assert_eq!(ok(vfs_write(fd, &payload, payload.len())), payload.len());
    ok(vfs_close(fd));

    let reopened = ok(vfs_open(note, vfs_open_flags::O_RDONLY, 77));
    let mut read_back = vec![0u8; payload.len()];
    assert_eq!(
        ok(vfs_read(reopened, &mut read_back, payload.len())),
        payload.len()
    );
    assert_eq!(read_back, payload);
    ok(vfs_close(reopened));

    let dirents = ok(vfs_readdir(docs, 0));
    assert!(dirents.iter().any(|entry| entry.get_name() == b"note.txt"));

    ok(vfs_rename(docs, b"note.txt", docs, b"renamed.txt"));
    assert_eq!(ok(vfs_lookup(docs, b"renamed.txt")), note);
    let renamed_dirents = ok(vfs_readdir(docs, 0));
    assert!(renamed_dirents
        .iter()
        .any(|entry| entry.get_name() == b"renamed.txt"));

    ok(vfs_unlink(docs, b"renamed.txt"));
    assert!(vfs_lookup(docs, b"renamed.txt").is_err());
    ok(vfs_rmdir(root, b"docs"));

    reset_vfs_state_for_test();
    reset_state();
}
