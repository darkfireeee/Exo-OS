//! # fs/exofs/syscall/object_fd/tests.rs — Tests unitaires de la table de descripteurs

use super::*;
use crate::fs::exofs::core::types::BlobId;

fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
    match res {
        Ok(value) => value,
        Err(err) => panic!("unexpected error: {err:?}"),
    }
}

fn make_blob(b: u8) -> BlobId {
    BlobId([b; 32])
}

fn fresh_table() -> &'static ObjectFdTable {
    static TABLE: ObjectFdTable = ObjectFdTable::new_const();
    TABLE.reset_all();
    &TABLE
}

#[test]
fn test_open_close_basic() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(1), open_flags::O_RDWR, 0, 0, 0));
    assert!(fd >= FD_RESERVED);
    assert!(t.close(fd));
}

#[test]
fn test_open_not_found_after_close() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(2), open_flags::O_RDONLY, 0, 0, 0));
    t.close(fd);
    assert!(t.get(fd).is_err());
}

#[test]
fn test_blob_id_of() {
    let t = fresh_table();
    let blob = make_blob(3);
    let fd = ok(t.open(blob, open_flags::O_RDONLY, 0, 0, 0));
    assert_eq!(ok(t.blob_id_of(fd)).0, blob.0);
    t.close(fd);
}

#[test]
fn test_open_count() {
    let t = fresh_table();
    assert_eq!(t.open_count(), 0);
    let fd1 = ok(t.open(make_blob(4), open_flags::O_RDWR, 0, 0, 0));
    let fd2 = ok(t.open(make_blob(5), open_flags::O_RDWR, 0, 0, 0));
    assert_eq!(t.open_count(), 2);
    t.close(fd1);
    assert_eq!(t.open_count(), 1);
    t.close(fd2);
    assert_eq!(t.open_count(), 0);
}

#[test]
fn test_set_cursor() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(6), open_flags::O_RDWR, 1024, 1, 0));
    ok(t.set_cursor(fd, 512));
    assert_eq!(ok(t.get(fd)).cursor, 512);
    t.close(fd);
}

#[test]
fn test_advance_cursor() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(7), open_flags::O_RDWR, 4096, 1, 0));
    ok(t.advance_cursor(fd, 100));
    assert_eq!(ok(t.get(fd)).cursor, 100);
    ok(t.advance_cursor(fd, 200));
    assert_eq!(ok(t.get(fd)).cursor, 300);
    t.close(fd);
}

#[test]
fn test_check_readable_rdonly() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(8), open_flags::O_RDONLY, 0, 0, 0));
    assert!(t.check_readable(fd).is_ok());
    assert!(t.check_writable(fd).is_err());
    t.close(fd);
}

#[test]
fn test_check_writable_wronly() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(9), open_flags::O_WRONLY, 0, 0, 0));
    assert!(t.check_writable(fd).is_ok());
    assert!(t.check_readable(fd).is_err());
    t.close(fd);
}

#[test]
fn test_invalid_flags_rejected() {
    let t = fresh_table();
    let r = t.open(make_blob(10), 0xDEAD_BEEF, 0, 0, 0);
    assert!(r.is_err());
}

#[test]
fn test_close_nonexistent_returns_false() {
    let t = fresh_table();
    assert!(!t.close(9999));
}

#[test]
fn test_get_invalid_fd_returns_err() {
    let t = fresh_table();
    assert!(t.get(0).is_err());
    assert!(t.get(3).is_err());
}

#[test]
fn test_dup_creates_distinct_fd_with_shared_cursor() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(11), open_flags::O_RDWR, 0, 0, 0));
    let dup_fd = ok(t.dup(fd));
    assert_ne!(fd, dup_fd);
    ok(t.set_cursor(fd, 123));
    assert_eq!(ok(t.get(dup_fd)).cursor, 123);

    t.close(fd);
    assert_eq!(ok(t.get(dup_fd)).cursor, 123);
    t.close(dup_fd);
    assert!(t.get(dup_fd).is_err(), "duplicate closed after final close");
}

#[test]
fn test_dup2_retargets_requested_fd() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(15), open_flags::O_RDWR, 0, 0, 0));
    let target_fd = fd.saturating_add(7);
    let dup_fd = ok(t.dup2(fd, target_fd));
    assert_eq!(dup_fd, target_fd);
    ok(t.set_cursor(fd, 64));
    assert_eq!(ok(t.get(target_fd)).cursor, 64);
    t.close(fd);
    assert_eq!(ok(t.get(target_fd)).cursor, 64);
    t.close(target_fd);
}

#[test]
fn test_set_status_flags_updates_shared_description() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(16), open_flags::O_RDWR, 0, 0, 0));
    let dup_fd = ok(t.dup(fd));
    let updated = ok(t.set_status_flags(dup_fd, open_flags::O_APPEND));
    assert_ne!(updated & open_flags::O_APPEND, 0);
    assert_ne!(ok(t.get(fd)).flags & open_flags::O_APPEND, 0);
    t.close(fd);
    t.close(dup_fd);
}

#[test]
fn test_dup_promotion_stress() {
    let t = fresh_table();

    let mut seed = 0u32;
    while seed < 128 {
        let fd = t.open(make_blob(seed as u8), open_flags::O_RDWR, 0, 0, 0);
        let fd = ok(fd);
        let mut fds = [0u32; 8];
        fds[0] = fd;

        let mut i = 1usize;
        while i < fds.len() {
            fds[i] = ok(t.dup(fd));
            i = i.wrapping_add(1);
        }

        ok(t.set_cursor(fd, seed as u64));

        let mut j = 0usize;
        while j < fds.len() {
            assert_eq!(ok(t.get(fds[j])).cursor, seed as u64);
            j = j.wrapping_add(1);
        }

        let mut k = 0usize;
        while k < fds.len() {
            assert!(t.close(fds[k]));
            k = k.wrapping_add(1);
        }

        seed = seed.wrapping_add(1);
    }

    assert_eq!(t.open_count(), 0);
}

#[test]
fn test_stats_tracking() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(12), open_flags::O_RDWR, 0, 0, 0));
    t.close(fd);
    let (o, c, _) = t.stats();
    assert_eq!(o, 1);
    assert_eq!(c, 1);
}

#[test]
fn test_reset_all() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(13), open_flags::O_RDWR, 0, 0, 0));
    assert_eq!(t.open_count(), 1);
    t.reset_all();
    assert_eq!(t.open_count(), 0);
    assert!(t.get(fd).is_err());
}

#[test]
fn test_set_size() {
    let t = fresh_table();
    let fd = ok(t.open(make_blob(14), open_flags::O_RDWR, 0, 0, 0));
    ok(t.set_size(fd, 8192));
    assert_eq!(ok(t.get(fd)).size, 8192);
    t.close(fd);
}

#[test]
fn test_open_flags_can_read_write() {
    assert!(open_flags::can_read(open_flags::O_RDONLY));
    assert!(!open_flags::can_write(open_flags::O_RDONLY));
    assert!(open_flags::can_read(open_flags::O_RDWR));
    assert!(open_flags::can_write(open_flags::O_RDWR));
    assert!(!open_flags::can_read(open_flags::O_WRONLY));
    assert!(open_flags::can_write(open_flags::O_WRONLY));
}
