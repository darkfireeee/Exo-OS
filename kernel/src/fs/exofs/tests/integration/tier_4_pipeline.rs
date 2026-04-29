use super::support::{
    close_fd, install_mock_disk, open_path, open_path_atomic, open_rdwr, read_at, readdir_fd,
    reset_state, write_at,
};
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, ObjectId};
use crate::fs::exofs::path::path_component::validate_component;
use crate::fs::exofs::path::path_index::{mount_secret_key, PathIndex};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::readdir::HEADER_SIZE;
use std::string::String;
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
        out.push(seed.wrapping_add((i % 251) as u8));
        i = i.wrapping_add(1);
    }
    out
}

fn parse_dirent_names(buf: &[u8]) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset.saturating_add(HEADER_SIZE) <= buf.len() {
        let reclen = u16::from_le_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
        if reclen == 0 || offset.saturating_add(reclen) > buf.len() {
            break;
        }
        let d_type = buf[offset + 18];
        let name_start = offset + HEADER_SIZE;
        let name_end = match buf[name_start..offset + reclen]
            .iter()
            .position(|byte| *byte == 0)
        {
            Some(pos) => name_start + pos,
            None => offset + reclen,
        };
        out.push((
            String::from_utf8_lossy(&buf[name_start..name_end]).into_owned(),
            d_type,
        ));
        offset = offset.saturating_add(reclen);
    }
    out
}

#[test]
fn syscall_pipeline_reloads_from_disk_after_cache_flush() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let path = "/tier4/pipeline/reload";
    let payload = make_payload(0x31, 2048);

    let fd = open_rdwr(path);
    assert_eq!(write_at(fd, &payload, 0), payload.len());
    close_fd(fd);

    BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let reopened = open_rdwr(path);
    let entry = ok(OBJECT_TABLE.get(reopened));
    assert_eq!(entry.size as usize, payload.len());

    let read_back = read_at(reopened, payload.len(), 0);
    assert_eq!(read_back, payload);
    close_fd(reopened);

    reset_state();
}

#[test]
fn cold_append_preserves_existing_content() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let path = "/tier4/pipeline/append";
    let prefix = b"prefix".to_vec();
    let suffix = b"_suffix".to_vec();

    let fd = open_rdwr(path);
    assert_eq!(write_at(fd, &prefix, 0), prefix.len());
    close_fd(fd);

    BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let reopened = open_rdwr(path);
    let append_offset = ok(OBJECT_TABLE.get(reopened)).size;
    assert_eq!(write_at(reopened, &suffix, append_offset), suffix.len());
    close_fd(reopened);

    BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let mut expected = prefix.clone();
    expected.extend_from_slice(&suffix);

    let appended_fd = open_rdwr(path);
    let appended = read_at(appended_fd, expected.len(), 0);
    assert_eq!(appended, expected);
    close_fd(appended_fd);

    BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let final_fd = open_path(path, open_flags::O_RDWR | open_flags::O_TRUNC);
    let truncated = read_at(final_fd, prefix.len() + suffix.len(), 0);
    assert!(truncated.is_empty());
    close_fd(final_fd);

    let rewritten_fd = open_rdwr(path);
    assert_eq!(write_at(rewritten_fd, &expected, 0), expected.len());
    close_fd(rewritten_fd);

    BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let verify_fd = open_rdwr(path);
    let read_back = read_at(verify_fd, expected.len(), 0);
    assert_eq!(read_back, expected);
    close_fd(verify_fd);

    reset_state();
}

#[test]
fn open_by_path_accepts_canonical_aliases_and_reloads_persisted_content() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let canonical_path = "/tier4/open-by-path/canonical/file";
    let alias_path = "/tier4/open-by-path//canonical/./file";
    let payload = make_payload(0x7B, 777);

    let fd = open_path_atomic(alias_path, open_flags::O_RDWR);
    assert_eq!(write_at(fd, &payload, 0), payload.len());
    close_fd(fd);

    BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let reopened = open_rdwr(canonical_path);
    assert_eq!(ok(OBJECT_TABLE.get(reopened)).size as usize, payload.len());
    let read_back = read_at(reopened, payload.len(), 0);
    assert_eq!(read_back, payload);
    close_fd(reopened);

    reset_state();
}

#[test]
fn readdir_reads_path_index_from_disk_after_cache_flush() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let dir_path = "/tier4/readdir/persisted";
    let dir_blob_id = BlobId::from_bytes_blake3(dir_path.as_bytes());
    let mut index = PathIndex::new_with_key(ObjectId([0u8; 32]), mount_secret_key());
    if let Err(err) = index.insert(&ok(validate_component(b"alpha.txt")), ObjectId([0x11; 32]), 8)
    {
        panic!("insert alpha failed: {err:?}");
    }
    if let Err(err) = index.insert(&ok(validate_component(b"nested")), ObjectId([0x22; 32]), 4) {
        panic!("insert nested failed: {err:?}");
    }
    let serialized = ok(index.serialize());
    let persisted = match object_store::persist_blob_data_if_disk(dir_blob_id, &serialized, true) {
        Ok(value) => value,
        Err(err) => panic!("persist dir blob failed: {err:?}"),
    };
    assert!(persisted);

    BLOB_CACHE.flush_all();
    OBJECT_TABLE.reset_all();

    let fd = open_path_atomic(dir_path, open_flags::O_RDONLY);
    let dirents = readdir_fd(fd, 2048);
    close_fd(fd);

    let parsed = parse_dirent_names(&dirents);
    assert!(parsed.iter().any(|(name, kind)| name == "alpha.txt" && *kind == 8));
    assert!(parsed.iter().any(|(name, kind)| name == "nested" && *kind == 4));

    reset_state();
}
