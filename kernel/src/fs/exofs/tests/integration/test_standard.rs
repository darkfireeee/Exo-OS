/// test_standard.rs — Tests standard complets ExoFS
///
/// Couvre : BlobCache, Superblock, lifecycle objet, paths, FDs,
/// readdir, append, truncate, erreurs, persistence, epoch.
///
/// Chaque test est autonome : reset_state() en début + fin.
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use super::support::{
    close_fd, install_mock_disk, open_path_atomic, open_rdwr, read_at, readdir_fd, reset_state,
    write_at,
};
use crate::fs::exofs::cache::blob_cache::BlobCache;
use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, ExofsError};
use crate::fs::exofs::storage::blob_reader::{BlobReader, BlobVerifyMode};
use crate::fs::exofs::storage::blob_writer::{BlobWriter, BlobWriterConfig};
use crate::fs::exofs::storage::superblock::SuperblockManager;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::fs::exofs::syscall::object_stat::object_size;
use crate::fs::exofs::syscall::readdir::HEADER_SIZE;
// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
    match res {
        Ok(v) => v,
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

fn blob(seed: u8) -> BlobId {
    BlobId([seed; 32])
}

fn payload(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| seed.wrapping_add((i % 251) as u8))
        .collect()
}

/// Parse les noms et types des dirents retournés par readdir.
fn parse_dirents(buf: &[u8]) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off.saturating_add(HEADER_SIZE) <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off.saturating_add(reclen) > buf.len() {
            break;
        }
        let dtype = buf[off + 18];
        let name_start = off + HEADER_SIZE;
        let name_end = buf[name_start..off + reclen]
            .iter()
            .position(|&b| b == 0)
            .map(|p| name_start + p)
            .unwrap_or(off + reclen);
        out.push((
            String::from_utf8_lossy(&buf[name_start..name_end]).into_owned(),
            dtype,
        ));
        off = off.saturating_add(reclen);
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 1 — BlobCache
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cache_insert_and_get_roundtrip() {
    let c = BlobCache::new_const();
    let data = payload(0xAA, 256);
    ok(c.insert(blob(1), data.clone()));
    let got = match c.get(&blob(1)) {
        Some(blob) => blob,
        None => panic!("blob doit etre present"),
    };
    assert_eq!(&*got, data.as_slice());
}

#[test]
fn cache_get_absent_returns_none() {
    let c = BlobCache::new_const();
    assert!(c.get(&blob(99)).is_none());
    assert_eq!(c.misses(), 1);
}

#[test]
fn cache_hit_counter_increments() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(2), payload(0x11, 64)));
    c.get(&blob(2));
    c.get(&blob(2));
    assert_eq!(c.hits(), 2);
}

#[test]
fn cache_invalidate_removes_entry() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(3), payload(0x22, 64)));
    c.invalidate(&blob(3));
    assert!(c.get(&blob(3)).is_none());
    assert_eq!(c.used_bytes(), 0);
}

#[test]
fn cache_mark_dirty_absent_returns_err() {
    let c = BlobCache::new_const();
    assert!(matches!(
        c.mark_dirty(&blob(50)),
        Err(ExofsError::ObjectNotFound)
    ));
}

#[test]
fn cache_flush_all_clean_succeeds() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(4), payload(0x33, 32)));
    // aucun dirty → flush_all doit réussir
    ok(c.flush_all());
    assert_eq!(c.n_entries(), 0);
    assert_eq!(c.used_bytes(), 0);
}

#[test]
fn cache_flush_all_with_dirty_returns_err() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(5), payload(0x44, 32)));
    ok(c.mark_dirty(&blob(5)));
    let err = c.flush_all().expect_err("doit échouer avec dirty");
    assert!(matches!(err, ExofsError::DirtyDataLoss(1)));
    // données préservées
    assert_eq!(c.n_entries(), 1);
}

#[test]
fn cache_collect_dirty_returns_all_pending_writes() {
    let c = BlobCache::new_const();
    let d1 = payload(0xAB, 64);
    let d2 = payload(0xCD, 128);
    ok(c.insert(blob(10), d1.clone()));
    ok(c.insert(blob(11), d2.clone()));
    ok(c.mark_dirty(&blob(10)));
    // blob(11) reste clean

    let dirty = c.collect_dirty();
    assert_eq!(dirty.len(), 1);
    let (id, data) = &dirty[0];
    assert_eq!(*id, blob(10));
    assert_eq!(&**data, d1.as_slice());
}

#[test]
fn cache_mark_clean_after_writeback() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(6), payload(0x55, 64)));
    ok(c.mark_dirty(&blob(6)));
    assert_eq!(c.dirty_ids().len(), 1);
    ok(c.mark_clean(&blob(6)));
    assert_eq!(c.dirty_ids().len(), 0);
    // maintenant flush_all réussit
    ok(c.flush_all());
}

#[test]
fn cache_used_bytes_tracks_precisely() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(7), payload(0x66, 100)));
    assert_eq!(c.used_bytes(), 100);
    ok(c.insert(blob(8), payload(0x77, 200)));
    assert_eq!(c.used_bytes(), 300);
    c.invalidate(&blob(7));
    assert_eq!(c.used_bytes(), 200);
}

#[test]
fn cache_overwrite_updates_size_correctly() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(9), payload(0x88, 100)));
    assert_eq!(c.used_bytes(), 100);
    // réécriture avec payload plus grand
    ok(c.insert(blob(9), payload(0x99, 400)));
    assert_eq!(c.used_bytes(), 400);
}

#[test]
fn cache_hit_ratio_pct_correct() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(20), payload(0xAA, 16)));
    c.get(&blob(20)); // hit
    c.get(&blob(20)); // hit
    c.get(&blob(21)); // miss
                      // 2 hits / 3 total = 66%
    assert_eq!(c.hit_ratio_pct(), 66);
}

#[test]
fn cache_flush_all_force_drops_dirty_without_error() {
    let c = BlobCache::new_const();
    ok(c.insert(blob(30), payload(0xBB, 64)));
    ok(c.mark_dirty(&blob(30)));
    c.flush_all_force(); // ne panique pas, ne retourne pas d'erreur
    assert_eq!(c.n_entries(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 2 — Superblock / Stockage
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn superblock_format_and_mount_roundtrip() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    let disk_size: u64 = 32 * 1024 * 1024; // 32 MiB

    ok(SuperblockManager::format(
        disk_size,
        b"test-vol",
        [0u8; 16],
        1000,
        |offset, buf| {
            disk.borrow_mut().insert(offset.0, buf.to_vec());
            Ok(buf.len())
        },
    ));

    let mgr = ok(SuperblockManager::mount(disk_size, |offset, len| {
        disk.borrow()
            .get(&offset.0)
            .cloned()
            .ok_or(ExofsError::IoError)
            .and_then(|b| {
                if b.len() >= len {
                    Ok(b[..len].to_vec())
                } else {
                    Err(ExofsError::IoError)
                }
            })
    }));

    assert_eq!(mgr.disk_size(), disk_size);
}

#[test]
fn superblock_mount_rejects_disk_too_small() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    // 1 MiB < MIN_DISK_SIZE (16 MiB)
    let small_disk: u64 = 1 * 1024 * 1024;

    let result = SuperblockManager::mount(small_disk, |offset, len| {
        disk.borrow()
            .get(&offset.0)
            .cloned()
            .ok_or(ExofsError::IoError)
            .and_then(|b| {
                if b.len() >= len {
                    Ok(b[..len].to_vec())
                } else {
                    Err(ExofsError::IoError)
                }
            })
    });
    assert!(
        matches!(result, Err(ExofsError::DiskTooSmall { .. })),
        "mount() doit rejeter un disque < MIN_DISK_SIZE, got: {result:?}"
    );
}

#[test]
fn superblock_format_rejects_disk_too_small() {
    let result = SuperblockManager::format(
        4 * 1024 * 1024, // 4 MiB < 16 MiB
        b"tiny",
        [0u8; 16],
        0,
        |_, buf| Ok(buf.len()),
    );
    assert!(matches!(result, Err(ExofsError::DiskTooSmall { .. })));
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 3 — Blob writer/reader
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn blob_write_read_full_verify() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    let next_off = RefCell::new(4096u64);
    let data = payload(0x42, 2048);
    let cfg = BlobWriterConfig::new(EpochId(1)).verify();

    let result = ok(BlobWriter::write_blob(
        &data,
        &cfg,
        |blocks| {
            let mut n = next_off.borrow_mut();
            let base = *n;
            *n = n.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |off, buf| {
            disk.borrow_mut().insert(off.0, buf.to_vec());
            Ok(buf.len())
        },
        |_| None,
    ));

    let read = ok(BlobReader::read_blob(
        result.offset,
        |off, len| {
            disk.borrow()
                .get(&off.0)
                .cloned()
                .ok_or(ExofsError::IoError)
                .and_then(|b| {
                    if b.len() >= len {
                        Ok(b[..len].to_vec())
                    } else {
                        Err(ExofsError::IoError)
                    }
                })
        },
        BlobVerifyMode::Full,
    ));

    assert_eq!(
        read.data, data,
        "données lues doivent correspondre à celles écrites"
    );
    assert_eq!(read.blob_id, result.blob_id, "BlobId doit être stable");
    assert!(read.id_verified, "vérification d'intégrité doit réussir");
}

#[test]
fn blob_tampered_data_fails_verification() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    let next_off = RefCell::new(4096u64);
    let data = payload(0x99, 512);
    let cfg = BlobWriterConfig::new(EpochId(2)).verify();

    let result = ok(BlobWriter::write_blob(
        &data,
        &cfg,
        |blocks| {
            let mut n = next_off.borrow_mut();
            let base = *n;
            *n = n.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |off, buf| {
            disk.borrow_mut().insert(off.0, buf.to_vec());
            Ok(buf.len())
        },
        |_| None,
    ));

    // Corrompre un octet au milieu du payload
    {
        let mut d = disk.borrow_mut();
        if let Some(blk) = d.get_mut(&result.offset.0) {
            let mid = blk.len() / 2;
            blk[mid] ^= 0xFF;
        }
    }

    let read_result = BlobReader::read_blob(
        result.offset,
        |off, len| {
            disk.borrow()
                .get(&off.0)
                .cloned()
                .ok_or(ExofsError::IoError)
                .and_then(|b| {
                    if b.len() >= len {
                        Ok(b[..len].to_vec())
                    } else {
                        Err(ExofsError::IoError)
                    }
                })
        },
        BlobVerifyMode::Full,
    );

    assert!(
        read_result.is_err(),
        "données corrompues doivent être détectées"
    );
}

#[test]
fn blob_write_empty_payload() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    let next_off = RefCell::new(4096u64);
    let cfg = BlobWriterConfig::new(EpochId(3));

    let result = ok(BlobWriter::write_blob(
        &[],
        &cfg,
        |blocks| {
            let mut n = next_off.borrow_mut();
            let base = *n;
            *n = n.saturating_add(blocks.saturating_mul(4096));
            Ok(DiskOffset(base))
        },
        |off, buf| {
            disk.borrow_mut().insert(off.0, buf.to_vec());
            Ok(buf.len())
        },
        |_| None,
    ));

    let read = ok(BlobReader::read_blob(
        result.offset,
        |off, len| {
            disk.borrow()
                .get(&off.0)
                .cloned()
                .ok_or(ExofsError::IoError)
                .and_then(|b| {
                    if b.len() >= len {
                        Ok(b[..len].to_vec())
                    } else {
                        Err(ExofsError::IoError)
                    }
                })
        },
        BlobVerifyMode::Full,
    ));

    assert!(read.data.is_empty(), "payload vide doit se lire vide");
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 4 — FD Table
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn fd_open_close_lifecycle() {
    let b = blob(0x01);
    let fd = ok(OBJECT_TABLE.open(b, open_flags::O_RDWR, 0, 0, 0));
    assert!(OBJECT_TABLE.get(fd).is_ok());
    assert!(OBJECT_TABLE.close(fd));
    assert!(
        OBJECT_TABLE.get(fd).is_err(),
        "fd fermé ne doit plus être valide"
    );
    OBJECT_TABLE.reset_all();
}

#[test]
fn fd_double_close_returns_false() {
    let b = blob(0x02);
    let fd = ok(OBJECT_TABLE.open(b, open_flags::O_RDONLY, 0, 0, 0));
    assert!(OBJECT_TABLE.close(fd));
    assert!(!OBJECT_TABLE.close(fd), "double close doit retourner false");
    OBJECT_TABLE.reset_all();
}

#[test]
fn fd_dup_shares_blob_id() {
    let b = blob(0x03);
    let fd = ok(OBJECT_TABLE.open(b, open_flags::O_RDWR, 1024, 0, 42));
    let dup = ok(OBJECT_TABLE.dup(fd));
    assert_ne!(fd, dup, "dup doit retourner un fd différent");
    let orig = ok(OBJECT_TABLE.get(fd));
    let duped = ok(OBJECT_TABLE.get(dup));
    assert_eq!(orig.blob_id, duped.blob_id, "blob_id doit être le même");
    OBJECT_TABLE.close(fd);
    OBJECT_TABLE.close(dup);
    OBJECT_TABLE.reset_all();
}

#[test]
fn fd_dup_shares_cursor() {
    let b = blob(0x04);
    let fd = ok(OBJECT_TABLE.open(b, open_flags::O_RDWR, 4096, 0, 0));
    let dup = ok(OBJECT_TABLE.dup(fd));
    ok(OBJECT_TABLE.set_cursor(fd, 512));
    let e1 = ok(OBJECT_TABLE.get(fd));
    let e2 = ok(OBJECT_TABLE.get(dup));
    assert_eq!(e1.cursor, 512);
    assert_eq!(e2.cursor, 512, "cursor partagé doit être synchronisé");
    OBJECT_TABLE.close(fd);
    OBJECT_TABLE.close(dup);
    OBJECT_TABLE.reset_all();
}

#[test]
fn fd_rdonly_rejects_write_flag() {
    let b = blob(0x05);
    let fd = ok(OBJECT_TABLE.open(b, open_flags::O_RDONLY, 0, 0, 0));
    let entry = ok(OBJECT_TABLE.get(fd));
    assert!(
        !entry.can_write(),
        "fd O_RDONLY ne doit pas autoriser l'écriture"
    );
    assert!(entry.can_read(), "fd O_RDONLY doit autoriser la lecture");
    OBJECT_TABLE.close(fd);
    OBJECT_TABLE.reset_all();
}

#[test]
fn fd_get_invalid_returns_err() {
    OBJECT_TABLE.reset_all();
    assert!(
        OBJECT_TABLE.get(9999).is_err(),
        "fd invalide doit retourner Err"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 5 — Lifecycle objet (syscall complet)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn object_write_read_roundtrip() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let data = payload(0x7F, 1024);
    let fd = open_rdwr("/std/rw/basic");
    assert_eq!(write_at(fd, &data, 0), data.len());
    let got = read_at(fd, data.len(), 0);
    assert_eq!(got, data, "lecture doit retourner les données écrites");
    close_fd(fd);
    reset_state();
}

#[test]
fn object_partial_read_at_offset() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let data = payload(0x3C, 512);
    let fd = open_rdwr("/std/rw/partial");
    write_at(fd, &data, 0);
    // lire 100 octets à partir de l'offset 100
    let got = read_at(fd, 100, 100);
    assert_eq!(got, data[100..200], "lecture partielle doit être correcte");
    close_fd(fd);
    reset_state();
}

#[test]
fn object_overwrite_updates_content() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd = open_rdwr("/std/rw/overwrite");
    let first = payload(0x11, 64);
    let second = payload(0x22, 64);
    write_at(fd, &first, 0);
    write_at(fd, &second, 0);
    let got = read_at(fd, 64, 0);
    assert_eq!(got, second, "écrasement doit mettre à jour les données");
    close_fd(fd);
    reset_state();
}

#[test]
fn object_append_extends_size() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd = open_rdwr("/std/append/basic");
    let part1 = payload(0xA1, 128);
    let part2 = payload(0xA2, 128);
    write_at(fd, &part1, 0);
    write_at(fd, &part2, 128);
    let combined = read_at(fd, 256, 0);
    assert_eq!(&combined[..128], part1.as_slice());
    assert_eq!(&combined[128..], part2.as_slice());
    close_fd(fd);
    reset_state();
}

#[test]
fn object_read_beyond_size_returns_partial() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd = open_rdwr("/std/rw/short");
    let data = payload(0x55, 64);
    write_at(fd, &data, 0);
    // demander plus que la taille de l'objet
    let got = read_at(fd, 256, 0);
    assert_eq!(
        got.len(),
        64,
        "lecture au-delà de la taille doit retourner seulement les données disponibles"
    );
    assert_eq!(got, data);
    close_fd(fd);
    reset_state();
}

#[test]
fn object_stat_reflects_write_size() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd = open_rdwr("/std/stat/size");
    let data = payload(0x60, 333);
    write_at(fd, &data, 0);
    let entry = ok(OBJECT_TABLE.get(fd));
    let sz = object_size(&entry.blob_id);
    assert_eq!(sz, 333, "stat.size doit refléter les octets écrits");
    close_fd(fd);
    reset_state();
}

#[test]
fn object_write_zero_bytes_is_noop() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd = open_rdwr("/std/rw/zerobytes");
    let data = payload(0x77, 64);
    write_at(fd, &data, 0);
    write_at(fd, &[], 0); // écriture vide
    let got = read_at(fd, 64, 0);
    assert_eq!(got, data, "écriture vide ne doit pas modifier le contenu");
    close_fd(fd);
    reset_state();
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 6 — Paths et nommage
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn path_open_same_path_returns_same_object() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd1 = open_rdwr("/std/path/same");
    let fd2 = open_rdwr("/std/path/same");
    let e1 = ok(OBJECT_TABLE.get(fd1));
    let e2 = ok(OBJECT_TABLE.get(fd2));
    assert_eq!(e1.blob_id, e2.blob_id, "même path → même BlobId");
    close_fd(fd1);
    close_fd(fd2);
    reset_state();
}

#[test]
fn path_different_paths_return_different_objects() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd1 = open_rdwr("/std/path/alpha");
    let fd2 = open_rdwr("/std/path/beta");
    let e1 = ok(OBJECT_TABLE.get(fd1));
    let e2 = ok(OBJECT_TABLE.get(fd2));
    assert_ne!(
        e1.blob_id, e2.blob_id,
        "paths différents → BlobId différents"
    );
    close_fd(fd1);
    close_fd(fd2);
    reset_state();
}

#[test]
fn path_nested_deep_path_works() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd = open_rdwr("/std/deep/a/b/c/d/file.txt");
    let data = payload(0xDE, 32);
    write_at(fd, &data, 0);
    let got = read_at(fd, 32, 0);
    assert_eq!(got, data, "chemin profond doit fonctionner");
    close_fd(fd);
    reset_state();
}

#[test]
fn path_atomic_open_by_path() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd = open_path_atomic("/std/path/atomic", open_flags::O_RDWR);
    let data = payload(0xCA, 16);
    write_at(fd, &data, 0);
    let got = read_at(fd, 16, 0);
    assert_eq!(got, data);
    close_fd(fd);
    reset_state();
}

#[test]
fn path_case_sensitive() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let fd1 = open_rdwr("/std/path/File");
    let fd2 = open_rdwr("/std/path/file");
    let e1 = ok(OBJECT_TABLE.get(fd1));
    let e2 = ok(OBJECT_TABLE.get(fd2));
    assert_ne!(
        e1.blob_id, e2.blob_id,
        "paths sensibles à la casse → objets distincts"
    );
    close_fd(fd1);
    close_fd(fd2);
    reset_state();
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 7 — Readdir
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn readdir_lists_created_entries() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    // Créer des fichiers dans un même répertoire
    let fd_a = open_rdwr("/std/dir/listing/alpha.txt");
    let fd_b = open_rdwr("/std/dir/listing/beta.txt");
    let fd_c = open_rdwr("/std/dir/listing/gamma.txt");
    write_at(fd_a, b"a", 0);
    write_at(fd_b, b"b", 0);
    write_at(fd_c, b"c", 0);
    close_fd(fd_a);
    close_fd(fd_b);
    close_fd(fd_c);

    let dir_fd = open_rdwr("/std/dir/listing");
    let buf = readdir_fd(dir_fd, 4096);
    let entries = parse_dirents(&buf);
    close_fd(dir_fd);

    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"alpha.txt"), "alpha.txt doit être listé");
    assert!(names.contains(&"beta.txt"), "beta.txt doit être listé");
    assert!(names.contains(&"gamma.txt"), "gamma.txt doit être listé");
    reset_state();
}

#[test]
fn readdir_empty_dir_returns_empty_or_dot_entries() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let dir_fd = open_rdwr("/std/dir/empty");
    let buf = readdir_fd(dir_fd, 4096);
    // doit retourner sans panique, même si vide
    close_fd(dir_fd);
    // pas d'assertion sur le contenu — dépend de l'implémentation des . et ..
    let _ = parse_dirents(&buf);
    reset_state();
}

#[test]
fn readdir_buffer_too_small_truncates_gracefully() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    for i in 0u8..10 {
        let fd = open_rdwr(&alloc::format!("/std/dir/small/{i}.txt"));
        write_at(fd, &[i], 0);
        close_fd(fd);
    }

    let dir_fd = open_rdwr("/std/dir/small");
    // buffer trop petit pour toutes les entrées
    let buf = readdir_fd(dir_fd, HEADER_SIZE + 32);
    close_fd(dir_fd);
    // doit retourner sans panique, avec au moins 0 ou 1 entrée
    let entries = parse_dirents(&buf);
    assert!(
        entries.len() <= 10,
        "nombre d'entrées ne peut pas dépasser le nombre créé"
    );
    reset_state();
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 8 — Persistence (cache flush + relecture disque)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn persistence_reload_after_cache_eviction() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let path = "/std/persist/reload";
    let data = payload(0xBB, 1024);

    // Écrire
    let fd = open_rdwr(path);
    write_at(fd, &data, 0);
    close_fd(fd);

    // Vider le cache complètement (simuler redémarrage)
    BLOB_CACHE.flush_all_force();
    OBJECT_TABLE.reset_all();

    // Relire depuis le disque
    let fd2 = open_rdwr(path);
    let got = read_at(fd2, data.len(), 0);
    close_fd(fd2);

    assert_eq!(got, data, "données doivent survivre à un flush du cache");
    reset_state();
}

#[test]
fn persistence_multiple_writes_survive_cache_flush() {
    reset_state();
    let _disk = install_mock_disk(512, 8192);

    let path = "/std/persist/multi";
    let writes: Vec<Vec<u8>> = (0u8..5).map(|i| payload(i * 10, 200)).collect();

    let fd = open_rdwr(path);
    for (i, w) in writes.iter().enumerate() {
        write_at(fd, w, (i * 200) as u64);
    }
    close_fd(fd);

    BLOB_CACHE.flush_all_force();
    OBJECT_TABLE.reset_all();

    let fd2 = open_rdwr(path);
    let total = 5 * 200;
    let got = read_at(fd2, total, 0);
    close_fd(fd2);

    for (i, w) in writes.iter().enumerate() {
        assert_eq!(
            &got[i * 200..(i + 1) * 200],
            w.as_slice(),
            "segment {i} doit survivre au flush"
        );
    }
    reset_state();
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 9 — Gestion des erreurs
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn error_inline_data_max_constant_is_canonical() {
    use crate::fs::exofs::core::constants::INLINE_DATA_MAX;
    // INLINE_DATA_MAX doit être 512 (valeur canonique post-fix CORR)
    assert_eq!(
        INLINE_DATA_MAX, 512,
        "INLINE_DATA_MAX doit être 512 (valeur canonique)"
    );
}

#[test]
fn error_gc_delay_constant_is_unique() {
    use crate::fs::exofs::core::constants::GC_MIN_EPOCH_DELAY;
    assert!(GC_MIN_EPOCH_DELAY > 0, "GC_MIN_EPOCH_DELAY doit être > 0");
    // Vérifier que la constante dépréciée pointe vers la même valeur
    #[allow(deprecated)]
    let deprecated_delay = crate::fs::exofs::core::constants::GC_MIN_EPOCH_DELAY_SECS;
    assert_eq!(
        GC_MIN_EPOCH_DELAY, deprecated_delay,
        "les deux constantes doivent avoir la même valeur"
    );
}

#[test]
fn error_blob_id_deterministic_for_same_content() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let data = payload(0xFF, 128);
    let cfg = BlobWriterConfig::new(EpochId(1));
    let next_off = RefCell::new(4096u64);

    let write_once = |disk: &RefCell<BTreeMap<u64, Vec<u8>>>| {
        BlobWriter::write_blob(
            &data,
            &cfg,
            |blocks| {
                let mut n = next_off.borrow_mut();
                let base = *n;
                *n = n.saturating_add(blocks.saturating_mul(4096));
                Ok(DiskOffset(base))
            },
            |off, buf| {
                disk.borrow_mut().insert(off.0, buf.to_vec());
                Ok(buf.len())
            },
            |_| None,
        )
    };

    let disk1 = RefCell::new(BTreeMap::new());
    let r1 = ok(write_once(&disk1));
    let disk2 = RefCell::new(BTreeMap::new());
    let r2 = ok(write_once(&disk2));

    assert_eq!(
        r1.blob_id, r2.blob_id,
        "même contenu → même BlobId (déterministe)"
    );
}

#[test]
fn error_different_content_gives_different_blob_id() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let next_off = RefCell::new(4096u64);
    let cfg = BlobWriterConfig::new(EpochId(1));

    let write = |data: &[u8], disk: &RefCell<BTreeMap<u64, Vec<u8>>>| {
        ok(BlobWriter::write_blob(
            data,
            &cfg,
            |blocks| {
                let mut n = next_off.borrow_mut();
                let base = *n;
                *n = n.saturating_add(blocks.saturating_mul(4096));
                Ok(DiskOffset(base))
            },
            |off, buf| {
                disk.borrow_mut().insert(off.0, buf.to_vec());
                Ok(buf.len())
            },
            |_| None,
        ))
    };

    let disk = RefCell::new(BTreeMap::new());
    let r1 = write(&payload(0x01, 64), &disk);
    let r2 = write(&payload(0x02, 64), &disk);
    assert_ne!(
        r1.blob_id, r2.blob_id,
        "contenus différents → BlobIds différents"
    );
}
