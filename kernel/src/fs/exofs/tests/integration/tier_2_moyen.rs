use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, ExofsError};
use crate::fs::exofs::storage::blob_reader::{BlobReader, BlobVerifyMode};
use crate::fs::exofs::storage::blob_writer::{blob_total_disk_size, BlobWriter, BlobWriterConfig};
use crate::fs::exofs::syscall::object_fd::{open_flags, ObjectFdTable};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::vec::Vec;

fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
    match res {
        Ok(value) => value,
        Err(err) => panic!("unexpected error: {err:?}"),
    }
}

fn make_blob(seed: u8) -> BlobId {
    BlobId([seed; 32])
}

fn make_payload(seed: u8, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0usize;
    while i < len {
        out.push(seed.wrapping_add((i % 251) as u8));
        i += 1;
    }
    out
}

fn fresh_fd_table() -> &'static ObjectFdTable {
    static TABLE: ObjectFdTable = ObjectFdTable::new_const();
    TABLE.reset_all();
    &TABLE
}

#[test]
fn blob_roundtrip_survives_header_and_payload_verification() {
    let disk = RefCell::new(BTreeMap::<u64, Vec<u8>>::new());
    let next_offset = RefCell::new(4096u64);
    let payload = make_payload(0x41, 1536);
    let cfg = BlobWriterConfig::new(EpochId(7)).verify();

    let result = ok(BlobWriter::write_blob(
        &payload,
        &cfg,
        |blocks| {
            let mut next = next_offset.borrow_mut();
            let base = *next;
            *next = next.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |offset, buf| {
            disk.borrow_mut().insert(offset.0, buf.to_vec());
            Ok(buf.len())
        },
        |_| None,
    ));

    let read_back = ok(BlobReader::read_blob(
        result.offset,
        |offset, len| {
            let buf = disk
                .borrow()
                .get(&offset.0)
                .cloned()
                .ok_or(ExofsError::IoError)?;
            if buf.len() < len {
                return Err(ExofsError::ShortWrite);
            }
            Ok(buf[..len].to_vec())
        },
        BlobVerifyMode::Full,
    ));

    assert_eq!(read_back.data, payload);
    assert_eq!(read_back.blob_id, result.blob_id);
    assert_eq!(result.disk_size, blob_total_disk_size(result.stored_size as u32));
    assert!(read_back.id_verified);
}

#[test]
fn object_fd_dup_keeps_shared_cursor_and_blob_identity() {
    let table = fresh_fd_table();
    let blob = make_blob(0x33);

    let fd = ok(table.open(blob, open_flags::O_RDWR, 4096, 12, 99));
    let dup = ok(table.dup(fd));

    ok(table.set_cursor(fd, 512));

    let original = ok(table.get(fd));
    let duplicate = ok(table.get(dup));

    assert_eq!(original.cursor, 512);
    assert_eq!(duplicate.cursor, 512);
    assert_eq!(original.blob_id, blob);
    assert_eq!(duplicate.blob_id, blob);
}
