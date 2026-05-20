use crate::fs::exofs::core::blob_id::compute_blob_id;
use crate::fs::exofs::core::{DiskOffset, EpochId, ExofsError, ObjectId};
use crate::fs::exofs::crypto::{SecretReader, SecretWriter};
use crate::fs::exofs::export::{
    CollectingReceiver, ExoarReader, ExoarReaderConfig, ExoarWriteOptions, ExoarWriter, SinkVec,
    SliceSource,
};
use crate::fs::exofs::snapshot::snapshot_create::{
    BlobEntry as SnapshotBlobEntry, SnapshotBlobSet, SnapshotCreateResult, SnapshotCreator,
    SnapshotParams,
};
use crate::fs::exofs::snapshot::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::snapshot::snapshot_restore::{
    RestoreOptions, RestoreSink, SnapshotBlobSource, SnapshotRestore,
};
use crate::fs::exofs::storage::compression_choice::{CompressionType, ContentHint};
use crate::fs::exofs::storage::compression_reader::DecompressReader as StorageDecompressReader;
use crate::fs::exofs::storage::compression_writer::CompressWriter as StorageCompressWriter;
use crate::fs::exofs::storage::dedup_reader::{DedupReadPipeline, DedupReader};
use crate::fs::exofs::storage::dedup_writer::{DedupDecision, DedupWriter};
use crate::fs::exofs::storage::object_reader::{
    verify_objects, ObjectRangeRead, ObjectRangeReader, ObjectReader, ObjectVerifyMode,
};
use crate::fs::exofs::storage::object_writer::{ObjectType, ObjectWriter, ObjectWriterConfig};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::vec::Vec;

struct SnapshotMapSource {
    order: Vec<crate::fs::exofs::core::BlobId>,
    payloads: BTreeMap<[u8; 32], Vec<u8>>,
}

impl SnapshotBlobSource for SnapshotMapSource {
    fn read_blob(
        &self,
        _snap_id: crate::fs::exofs::core::SnapshotId,
        blob_id: crate::fs::exofs::core::BlobId,
    ) -> Result<Vec<u8>, ExofsError> {
        self.payloads
            .get(blob_id.as_bytes())
            .cloned()
            .ok_or(ExofsError::ObjectNotFound)
    }

    fn list_blobs(
        &self,
        _snap_id: crate::fs::exofs::core::SnapshotId,
    ) -> Result<Vec<crate::fs::exofs::core::BlobId>, ExofsError> {
        Ok(self.order.clone())
    }
}

#[derive(Default)]
struct SnapshotCollectSink {
    payloads: BTreeMap<[u8; 32], Vec<u8>>,
    finalized: bool,
    aborted: bool,
}

impl RestoreSink for SnapshotCollectSink {
    fn write_blob(
        &mut self,
        blob_id: crate::fs::exofs::core::BlobId,
        data: &[u8],
    ) -> Result<usize, ExofsError> {
        self.payloads.insert(*blob_id.as_bytes(), data.to_vec());
        Ok(data.len())
    }

    fn finalize(&mut self) -> Result<(), ExofsError> {
        self.finalized = true;
        Ok(())
    }

    fn abort(&mut self) {
        self.aborted = true;
    }
}

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
        out.push(
            seed.wrapping_mul(17)
                .wrapping_add((i & 0xFF) as u8)
                .wrapping_add(((i >> 2) & 0x7F) as u8),
        );
        i = i.wrapping_add(1);
    }
    out
}

fn write_buf(
    disk: &RefCell<BTreeMap<u64, Vec<u8>>>,
    offset: DiskOffset,
    buf: &[u8],
) -> Result<usize, ExofsError> {
    disk.borrow_mut().insert(offset.0, buf.to_vec());
    Ok(buf.len())
}

fn read_buf(
    disk: &RefCell<BTreeMap<u64, Vec<u8>>>,
    offset: DiskOffset,
    len: usize,
) -> Result<Vec<u8>, ExofsError> {
    let buf = disk
        .borrow()
        .get(&offset.0)
        .cloned()
        .ok_or(ExofsError::IoError)?;
    if buf.len() < len {
        return Err(ExofsError::ShortWrite);
    }
    Ok(buf[..len].to_vec())
}

#[test]
fn multi_blob_object_roundtrip_uses_real_extent_map() {
    let disk = RefCell::new(BTreeMap::<u64, Vec<u8>>::new());
    let next_offset = RefCell::new(4096u64);
    let object_id = ObjectId([0x44; 32]);
    let payload = make_payload(0x22, 13_537);
    let config = ObjectWriterConfig::new(EpochId(11))
        .with_type(ObjectType::Regular)
        .with_hint(ContentHint::Unknown)
        .no_dedup()
        .with_chunk_size(4096);

    let result = ok(ObjectWriter::write_object(
        object_id,
        &payload,
        &config,
        |blocks| {
            let mut next = next_offset.borrow_mut();
            let base = *next;
            *next = next.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |offset, buf| write_buf(&disk, offset, buf),
        |_| None,
    ));

    assert!(result.blob_count > 1);
    assert_ne!(result.extent_map_offset.0, result.blobs[0].offset.0);

    let header_offset = ok(ObjectWriter::write_header(
        &result,
        &config,
        |blocks| {
            let mut next = next_offset.borrow_mut();
            let base = *next;
            *next = next.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |offset, buf| write_buf(&disk, offset, buf),
    ));

    let read_back = ok(ObjectReader::read_object(
        header_offset,
        |offset, len| read_buf(&disk, offset, len),
        ObjectVerifyMode::Full,
    ));

    assert_eq!(read_back.data, payload);
    assert_eq!(read_back.meta.object_id, object_id);
    assert_eq!(read_back.meta.blob_count, result.blob_count);
    assert!(read_back.hash_verified);

    let report = verify_objects(&[header_offset], &|offset, len| {
        read_buf(&disk, offset, len)
    });
    assert_eq!(report.checked, 1);
    assert_eq!(report.ok, 1);
}

#[test]
fn object_range_reader_spans_chunk_boundaries_without_full_scan_loss() {
    let disk = RefCell::new(BTreeMap::<u64, Vec<u8>>::new());
    let next_offset = RefCell::new(8192u64);
    let object_id = ObjectId([0x55; 32]);
    let chunk_size = 4096usize;
    let payload = make_payload(0x63, 9_500);
    let config = ObjectWriterConfig::new(EpochId(13))
        .with_type(ObjectType::Regular)
        .no_dedup()
        .with_chunk_size(chunk_size);

    let result = ok(ObjectWriter::write_object(
        object_id,
        &payload,
        &config,
        |blocks| {
            let mut next = next_offset.borrow_mut();
            let base = *next;
            *next = next.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |offset, buf| write_buf(&disk, offset, buf),
        |_| None,
    ));
    let header_offset = ok(ObjectWriter::write_header(
        &result,
        &config,
        |blocks| {
            let mut next = next_offset.borrow_mut();
            let base = *next;
            *next = next.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |offset, buf| write_buf(&disk, offset, buf),
    ));

    let range = ObjectRangeRead {
        logical_offset: 1733,
        length: 4097,
    };
    let ranged = ok(ObjectRangeReader::read_range(
        header_offset,
        &range,
        |offset, len| read_buf(&disk, offset, len),
        chunk_size,
    ));

    let expected =
        &payload[range.logical_offset as usize..range.logical_offset as usize + range.length];
    assert_eq!(ranged, expected);
}

#[test]
fn compression_crypto_and_dedup_roundtrip_stays_coherent() {
    let raw = make_payload(0x4D, 12_288);
    let compressed = ok(StorageCompressWriter::new(CompressionType::Lz4).compress(&raw));

    let key = [0xAB; 32];
    let encrypted = ok(SecretWriter::new(&key).encrypt(&compressed.data));
    let decrypted = ok(SecretReader::new(&key).decrypt(&encrypted));
    let decompressed = ok(StorageDecompressReader::decompress(&decrypted));
    assert_eq!(decompressed.data, raw);

    let dedup_writer = DedupWriter::new();
    let dedup_reader = DedupReader::new();
    let blob_id = match dedup_writer.check(&raw) {
        DedupDecision::Miss { blob_id } => {
            ok(dedup_writer.register(blob_id, DiskOffset(16_384), raw.len() as u64));
            blob_id
        }
        DedupDecision::Hit { blob_id, .. } => blob_id,
    };
    assert!(matches!(
        dedup_writer.check(&raw),
        DedupDecision::Hit { .. }
    ));

    let pipeline = DedupReadPipeline::new(&dedup_reader, &dedup_writer);
    let read_back = ok(pipeline.read_blob(&blob_id, &|offset, out| {
        assert_eq!(offset, DiskOffset(16_384));
        let n = out.len().min(raw.len());
        out[..n].copy_from_slice(&raw[..n]);
        Ok(n)
    }));
    assert_eq!(read_back, raw);
}

#[test]
fn snapshot_export_and_restore_roundtrip() {
    SNAPSHOT_LIST.clear();

    let payload_a = make_payload(0x31, 2048);
    let payload_b = make_payload(0x92, 1536);
    let blob_a = compute_blob_id(&payload_a);
    let blob_b = compute_blob_id(&payload_b);

    let mut blob_set = SnapshotBlobSet::new();
    ok(blob_set.push(SnapshotBlobEntry::new(blob_a, payload_a.len() as u64)));
    ok(blob_set.push(SnapshotBlobEntry::new(blob_b, payload_b.len() as u64)));

    let params = SnapshotParams::new(b"tier5-roundtrip", None, EpochId(77));
    let created: SnapshotCreateResult = ok(SnapshotCreator::create(&params, blob_set));
    assert_eq!(created.n_blobs, 2);

    let mut sink = SinkVec::new();
    let mut writer = ExoarWriter::new(ExoarWriteOptions::snapshot(77));
    ok(writer.begin(&mut sink).map_err(ExofsError::from));
    ok(writer
        .write_blob(&mut sink, blob_a.as_bytes(), &payload_a, 0, 77)
        .map_err(ExofsError::from));
    ok(writer
        .write_blob(&mut sink, blob_b.as_bytes(), &payload_b, 0, 77)
        .map_err(ExofsError::from));
    ok(writer.finalize(&mut sink).map_err(ExofsError::from));

    let archive = sink.into_inner();
    let mut source = SliceSource::new(&archive);
    let mut receiver = CollectingReceiver::new();
    let report = ok(ExoarReader::new(ExoarReaderConfig::strict())
        .read(&mut source, &mut receiver)
        .map_err(ExofsError::from));
    assert!(report.archive_valid);
    assert_eq!(report.entries_read, 2);
    assert_eq!(receiver.blobs.len(), 2);

    let snapshot_source = SnapshotMapSource {
        order: vec![blob_a, blob_b],
        payloads: BTreeMap::from([
            (*blob_a.as_bytes(), payload_a.clone()),
            (*blob_b.as_bytes(), payload_b.clone()),
        ]),
    };
    let mut restore_sink = SnapshotCollectSink::default();
    let restore = SnapshotRestore::new();
    let restored = ok(restore.restore(
        created.id,
        &snapshot_source,
        &mut restore_sink,
        RestoreOptions::default(),
    ));

    assert_eq!(restored.n_blobs_ok, 2);
    assert_eq!(
        restored.bytes_restored,
        payload_a.len() as u64 + payload_b.len() as u64
    );
    assert!(restore_sink.finalized);
    assert!(!restore_sink.aborted);
    assert_eq!(
        restore_sink.payloads.get(blob_a.as_bytes()),
        Some(&payload_a)
    );
    assert_eq!(
        restore_sink.payloads.get(blob_b.as_bytes()),
        Some(&payload_b)
    );

    SNAPSHOT_LIST.clear();
}
