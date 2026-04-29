use crate::fs::exofs::core::{DiskOffset, EpochId, ExofsError};
use crate::fs::exofs::storage::blob_reader::{BlobReadResult, BlobReader, BlobVerifyMode};
use crate::fs::exofs::storage::blob_writer::{BlobWriteResult, BlobWriter, BlobWriterConfig};
use std::cell::RefCell;
use std::collections::BTreeMap;
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
        let value = seed
            .wrapping_mul(31)
            .wrapping_add((i & 0xFF) as u8)
            .wrapping_add(((i >> 3) & 0x7F) as u8);
        out.push(value);
        i += 1;
    }
    out
}

fn read_blob(
    disk: &RefCell<BTreeMap<u64, Vec<u8>>>,
    offset: DiskOffset,
) -> Result<BlobReadResult, ExofsError> {
    BlobReader::read_blob(
        offset,
        |at, len| {
            let buf = disk
                .borrow()
                .get(&at.0)
                .cloned()
                .ok_or(ExofsError::IoError)?;
            if buf.len() < len {
                return Err(ExofsError::ShortWrite);
            }
            Ok(buf[..len].to_vec())
        },
        BlobVerifyMode::Full,
    )
}

#[test]
fn stress_blob_pipeline_handles_many_varied_roundtrips() {
    let disk = RefCell::new(BTreeMap::<u64, Vec<u8>>::new());
    let next_offset = RefCell::new(8192u64);
    let cfg = BlobWriterConfig::new(EpochId(42));
    let mut writes: Vec<(BlobWriteResult, Vec<u8>)> = Vec::new();

    let mut i = 0usize;
    while i < 128 {
        let size = 96 + ((i * 53) % 3072);
        let payload = make_payload((i & 0xFF) as u8, size);
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

        writes.push((result, payload));
        i += 1;
    }

    let mut j = 0usize;
    while j < writes.len() {
        let (result, expected) = &writes[j];
        let read_back = ok(read_blob(&disk, result.offset));
        assert_eq!(read_back.data, *expected);
        assert_eq!(read_back.blob_id, result.blob_id);
        assert!(read_back.id_verified);
        j += 1;
    }
}
