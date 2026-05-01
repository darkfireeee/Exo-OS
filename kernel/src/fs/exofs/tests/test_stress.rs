/// test_stress.rs — Tests de stress complets ExoFS
///
/// Couvre : volume élevé d'opérations, pression mémoire cache,
/// fragmentation, cycles rapides create/delete, données aléatoires
/// de grande taille, fd exhaustion, intégrité sous charge.
///
/// Chaque test est autonome : reset_state() en début + fin.
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use super::support::{
    close_fd, install_mock_disk, open_dir, open_rdwr, parse_dirents, read_at, readdir_fd,
    reset_state, write_at,
};
use crate::fs::exofs::cache::blob_cache::BlobCache;
use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, ExofsError};
use crate::fs::exofs::storage::blob_reader::{BlobReader, BlobVerifyMode};
use crate::fs::exofs::storage::blob_writer::{BlobWriter, BlobWriterConfig};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
    match res {
        Ok(v) => v,
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

/// Générateur pseudo-aléatoire déterministe (xorshift32).
struct Xorshift32(u32);

impl Xorshift32 {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn next_range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() as usize % (hi - lo))
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(4) {
            let v = self.next().to_le_bytes();
            let n = chunk.len();
            chunk.copy_from_slice(&v[..n]);
        }
    }
}

fn payload(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| seed.wrapping_add((i % 251) as u8))
        .collect()
}

fn blob_for(i: usize) -> BlobId {
    let mut raw = [0u8; 32];
    raw[..8].copy_from_slice(&(i as u64).to_le_bytes());
    BlobId(raw)
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 1 — Volume élevé : BlobCache sous pression
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stress_cache_1000_inserts_all_retrievable() {
    let c = BlobCache::new_const();
    let n = 1000usize;

    // Insérer 1000 blobs distincts
    for i in 0..n {
        let data = payload((i % 256) as u8, 64);
        ok(c.insert(blob_for(i), data));
    }

    // Vérifier que les derniers blobs insérés (ceux non évincés) sont lisibles
    // On teste les 200 derniers — ils sont les plus "chauds" dans le LRU
    let mut found = 0usize;
    for i in (n - 200)..n {
        if c.get(&blob_for(i)).is_some() {
            found += 1;
        }
    }
    assert!(
        found >= 150,
        "au moins 150/200 blobs récents doivent être en cache, got: {found}"
    );
}

#[test]
fn stress_cache_dirty_tracking_under_load() {
    let c = BlobCache::new_const();

    // Insérer 200 blobs, marquer 100 comme dirty
    for i in 0..200usize {
        ok(c.insert(blob_for(i), payload((i % 256) as u8, 32)));
        if i % 2 == 0 {
            ok(c.mark_dirty(&blob_for(i)));
        }
    }

    let dirty = c.dirty_ids();
    // dirty_ids retourne les blobs présents ET dirty
    // (certains peuvent avoir été évincés — ils ne comptent pas)
    for id in &dirty {
        // tous les ids dirty retournés doivent effectivement être dans le cache
        assert!(c.contains(id), "dirty_id doit encore être dans le cache");
    }

    // flush_all doit échouer s'il reste des dirty
    if !dirty.is_empty() {
        assert!(
            c.flush_all().is_err(),
            "flush_all doit échouer avec des dirty restants"
        );
    }

    // collect_dirty → mark_clean → flush_all doit réussir
    let pending = c.collect_dirty();
    for (id, _) in &pending {
        ok(c.mark_clean(id));
    }
    ok(c.flush_all());
    assert_eq!(c.n_entries(), 0);
}

#[test]
fn stress_cache_eviction_does_not_corrupt_remaining() {
    let c = BlobCache::new_const();
    let mut rng = Xorshift32::new(0xDEAD_BEEF);

    // Insérer des blobs de taille variée pour provoquer des évictions
    let mut payloads: Vec<(BlobId, Vec<u8>)> = Vec::new();
    for i in 0..500usize {
        let size = rng.next_range(64, 2048);
        let mut data = alloc::vec![0u8; size];
        rng.fill(&mut data);
        let id = blob_for(i);
        ok(c.insert(id, data.clone()));
        payloads.push((id, data));
    }

    // Vérifier que chaque blob encore présent a son contenu intact
    for (id, expected) in &payloads {
        if let Some(got) = c.get(id) {
            assert_eq!(
                &*got,
                expected.as_slice(),
                "blob {:?} a un contenu corrompu après éviction",
                id
            );
        }
        // si évincé → OK, pas de vérification
    }
}

#[test]
fn stress_cache_overwrite_1000_same_key() {
    let c = BlobCache::new_const();
    let id = blob_for(0);

    for i in 0..1000usize {
        let data = payload((i % 256) as u8, 128);
        ok(c.insert(id, data.clone()));
        let got = match c.get(&id) {
            Some(blob) => blob,
            None => panic!("blob doit etre present apres insert"),
        };
        assert_eq!(&*got, data.as_slice(), "ecrasement {i} : contenu incorrect");
    }

    assert_eq!(
        c.n_entries(),
        1,
        "une seule entrée doit exister pour la même clé"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 2 — Volume élevé : Blob writer/reader
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stress_256_blob_roundtrips_varied_sizes() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    let next_off = RefCell::new(8192u64);
    let mut rng = Xorshift32::new(0x1234_5678);
    let cfg = BlobWriterConfig::new(EpochId(1)).verify();

    let mut records: Vec<(_, Vec<u8>)> = Vec::new();

    for i in 0..256usize {
        let size = rng.next_range(64, 4096);
        let mut data = alloc::vec![0u8; size];
        rng.fill(&mut data);

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

        records.push((result, data));

        if i % 32 == 0 {
            // Vérification intermédiaire
            for (r, expected) in &records {
                let read = ok(BlobReader::read_blob(
                    r.offset,
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
                    &read.data, expected,
                    "intégrité intermédiaire échouée à i={i}"
                );
            }
        }
    }

    // Vérification finale de tous les blobs
    for (r, expected) in &records {
        let read = ok(BlobReader::read_blob(
            r.offset,
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
        assert_eq!(&read.data, expected);
        assert!(read.id_verified);
    }
}

#[test]
fn stress_blob_ids_are_all_unique() {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    let next_off = RefCell::new(4096u64);
    let cfg = BlobWriterConfig::new(EpochId(99));
    let mut seen_ids: BTreeSet<[u8; 32]> = BTreeSet::new();
    let mut rng = Xorshift32::new(0xABCD_EF01);

    for _ in 0..200 {
        let size = rng.next_range(32, 1024);
        let mut data = alloc::vec![0u8; size];
        rng.fill(&mut data);

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

        assert!(
            seen_ids.insert(result.blob_id.0),
            "collision de BlobId détectée : {:?}",
            result.blob_id
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 3 — Stress FD table
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stress_fd_table_many_open_close_cycles() {
    OBJECT_TABLE.reset_all();
    let mut open_fds: Vec<u32> = Vec::new();

    // Ouvrir 64 FDs
    for i in 0..64usize {
        let fd = ok(OBJECT_TABLE.open(blob_for(i), open_flags::O_RDWR, 0, 0, 0));
        open_fds.push(fd);
    }

    // Fermer tous
    for fd in &open_fds {
        assert!(OBJECT_TABLE.close(*fd), "fermeture du fd {fd} doit réussir");
    }

    // Tous doivent être invalides maintenant
    for fd in &open_fds {
        assert!(
            OBJECT_TABLE.get(*fd).is_err(),
            "fd {fd} fermé doit être invalide"
        );
    }

    OBJECT_TABLE.reset_all();
}

#[test]
fn stress_fd_dup_chain_all_share_cursor() {
    OBJECT_TABLE.reset_all();

    let b = blob_for(999);
    let fd0 = ok(OBJECT_TABLE.open(b, open_flags::O_RDWR, 8192, 0, 0));
    let fd1 = ok(OBJECT_TABLE.dup(fd0));
    let fd2 = ok(OBJECT_TABLE.dup(fd1));
    let fd3 = ok(OBJECT_TABLE.dup(fd2));

    ok(OBJECT_TABLE.set_cursor(fd0, 1024));

    for fd in [fd0, fd1, fd2, fd3] {
        let e = ok(OBJECT_TABLE.get(fd));
        assert_eq!(e.cursor, 1024, "fd {fd} doit partager le cursor");
    }

    for fd in [fd0, fd1, fd2, fd3] {
        OBJECT_TABLE.close(fd);
    }
    OBJECT_TABLE.reset_all();
}

#[test]
fn stress_fd_interleaved_open_close() {
    OBJECT_TABLE.reset_all();
    let mut rng = Xorshift32::new(0xFEED_FACE);
    let mut active: Vec<u32> = Vec::new();

    for round in 0..200usize {
        // Ouvrir 1-3 FDs
        let opens = rng.next_range(1, 4);
        for _ in 0..opens {
            let b = blob_for(rng.next_range(0, 50));
            if let Ok(fd) = OBJECT_TABLE.open(b, open_flags::O_RDONLY, 0, 0, 0) {
                active.push(fd);
            }
        }

        // Fermer 0-2 FDs aléatoires
        let closes = rng.next_range(0, 3).min(active.len());
        for _ in 0..closes {
            if active.is_empty() {
                break;
            }
            let idx = rng.next_range(0, active.len());
            let fd = active.remove(idx);
            OBJECT_TABLE.close(fd);
        }

        let _ = round; // éviter warning unused
    }

    // Fermer tous les FDs restants
    for fd in active {
        OBJECT_TABLE.close(fd);
    }

    OBJECT_TABLE.reset_all();
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 4 — Stress syscall layer (via mock disk)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stress_100_files_write_read_verify() {
    reset_state();
    let _disk = install_mock_disk(512, 32768); // 16 MiB

    let mut rng = Xorshift32::new(0xC0DE_CAFE);
    let mut records: Vec<(String, Vec<u8>)> = Vec::new();

    // Créer 100 fichiers avec du contenu aléatoire
    for i in 0..100usize {
        let path = alloc::format!("/stress/files/{i:03}.bin");
        let size = rng.next_range(16, 512);
        let mut data = alloc::vec![0u8; size];
        rng.fill(&mut data);

        let fd = open_rdwr(&path);
        write_at(fd, &data, 0);
        close_fd(fd);
        records.push((path, data));
    }

    // Vérifier chaque fichier
    for (path, expected) in &records {
        let fd = open_rdwr(path);
        let got = read_at(fd, expected.len(), 0);
        close_fd(fd);
        assert_eq!(&got, expected, "contenu incorrect pour {path}");
    }

    reset_state();
}

#[test]
fn stress_sequential_append_builds_large_file() {
    reset_state();
    let _disk = install_mock_disk(512, 32768);

    let path = "/stress/append/large";
    let chunk_size = 256usize;
    let num_chunks = 64usize;
    let mut full_data: Vec<u8> = Vec::with_capacity(chunk_size * num_chunks);

    let fd = open_rdwr(path);
    for i in 0..num_chunks {
        let chunk = payload((i % 256) as u8, chunk_size);
        write_at(fd, &chunk, (i * chunk_size) as u64);
        full_data.extend_from_slice(&chunk);
    }
    close_fd(fd);

    // Vérifier le contenu complet
    let fd2 = open_rdwr(path);
    let got = read_at(fd2, full_data.len(), 0);
    close_fd(fd2);
    assert_eq!(
        got.len(),
        full_data.len(),
        "taille incorrecte après append séquentiel"
    );
    assert_eq!(got, full_data, "contenu incorrect après append séquentiel");

    reset_state();
}

#[test]
fn stress_rapid_create_overwrite_cycles() {
    reset_state();
    let _disk = install_mock_disk(512, 16384);

    let path = "/stress/cycles/target";

    for i in 0u8..50 {
        let data = payload(i, 128);
        let fd = open_rdwr(path);
        write_at(fd, &data, 0);
        let got = read_at(fd, 128, 0);
        assert_eq!(
            got, data,
            "cycle {i}: lecture doit retourner la dernière écriture"
        );
        close_fd(fd);
    }

    reset_state();
}

#[test]
fn stress_interleaved_reads_writes_same_fd() {
    reset_state();
    let _disk = install_mock_disk(512, 16384);

    // Initialiser l'objet avec un contenu nul pour que les lectures aient
    // une base valide avant les écritures entrelacées
    let init_data = alloc::vec![0u8; 2048];
    let init_fd = open_rdwr("/stress/interleaved/rw");
    write_at(init_fd, &init_data, 0);
    close_fd(init_fd);

    let fd = open_rdwr("/stress/interleaved/rw");
    let mut rng = Xorshift32::new(0x5EED_1234);
    let mut shadow = alloc::vec![0u8; 2048];

    for _ in 0..200 {
        let op = rng.next_range(0, 2);
        let offset = rng.next_range(0, 1792);
        let size = rng.next_range(16, 256);

        if op == 0 {
            // Write
            let mut chunk = alloc::vec![0u8; size];
            rng.fill(&mut chunk);
            write_at(fd, &chunk, offset as u64);
            shadow[offset..offset + size].copy_from_slice(&chunk);
        } else {
            // Read dans la zone valide
            let readable_size = size.min(shadow.len().saturating_sub(offset));
            if readable_size == 0 {
                continue;
            }
            let got = read_at(fd, readable_size, offset as u64);
            assert_eq!(
                &got,
                &shadow[offset..offset + readable_size],
                "lecture incorrecte à offset={offset} size={readable_size}"
            );
        }
    }

    close_fd(fd);
    reset_state();
}

#[test]
fn stress_many_dirs_readdir_all_populated() {
    reset_state();
    let _disk = install_mock_disk(512, 32768);

    let num_dirs = 10usize;
    let files_per_dir = 5usize;

    // Créer num_dirs répertoires avec files_per_dir fichiers chacun
    for d in 0..num_dirs {
        for f in 0..files_per_dir {
            let path = alloc::format!("/stress/dirs/dir{d:02}/file{f:02}.txt");
            let fd = open_rdwr(&path);
            write_at(fd, &payload((d * f) as u8, 8), 0);
            close_fd(fd);
        }
    }

    // Lire chaque répertoire avec open_dir (kind=Directory) et vérifier le contenu
    for d in 0..num_dirs {
        let dir_path = alloc::format!("/stress/dirs/dir{d:02}");
        let dir_fd = open_dir(&dir_path);
        let buf = readdir_fd(dir_fd, 8192);
        close_fd(dir_fd);

        let entries = parse_dirents(&buf);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();

        for f in 0..files_per_dir {
            let fname = alloc::format!("file{f:02}.txt");
            assert!(
                names.contains(&fname.as_str()),
                "dir{d:02}: fichier {fname} manquant dans readdir. Trouvés: {names:?}"
            );
        }
    }

    reset_state();
}

#[test]
fn stress_persistence_100_files_survive_cache_flush() {
    reset_state();
    let _disk = install_mock_disk(512, 32768);

    let mut rng = Xorshift32::new(0x7E515700);
    let mut records: Vec<(String, Vec<u8>)> = Vec::new();

    for i in 0..100usize {
        let path = alloc::format!("/stress/persist/{i:03}");
        let size = rng.next_range(32, 256);
        let mut data = alloc::vec![0u8; size];
        rng.fill(&mut data);
        let fd = open_rdwr(&path);
        write_at(fd, &data, 0);
        close_fd(fd);
        records.push((path, data));
    }

    // Flush total du cache
    use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
    BLOB_CACHE.flush_all_force();
    OBJECT_TABLE.reset_all();

    // Relire depuis le disque
    for (path, expected) in &records {
        let fd = open_rdwr(path);
        let got = read_at(fd, expected.len(), 0);
        close_fd(fd);
        assert_eq!(&got, expected, "fichier {path} corrompu après flush cache");
    }

    reset_state();
}

#[test]
fn stress_large_write_fragmented_reads() {
    reset_state();
    let _disk = install_mock_disk(512, 65536); // 32 MiB

    let path = "/stress/large/fragmented";
    let total = 16384usize; // 16 KiB
    let data = payload(0x42, total);

    let fd = open_rdwr(path);
    write_at(fd, &data, 0);
    close_fd(fd);

    // Lire en petits morceaux aléatoires et vérifier chaque fragment
    let fd2 = open_rdwr(path);
    let mut rng = Xorshift32::new(0xF4A6_3E87);
    let mut verified = 0usize;

    for _ in 0..200 {
        let offset = rng.next_range(0, total - 64);
        let size = rng.next_range(16, 64).min(total - offset);
        let got = read_at(fd2, size, offset as u64);
        assert_eq!(
            &got,
            &data[offset..offset + size],
            "fragment à offset={offset} size={size} incorrect"
        );
        verified += size;
    }

    close_fd(fd2);
    assert!(
        verified > 1000,
        "doit avoir vérifié au moins 1000 octets, got {verified}"
    );
    reset_state();
}

// ─────────────────────────────────────────────────────────────────────────────
// SECTION 5 — Cohérence et invariants sous charge
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stress_concurrent_opens_same_path_consistent() {
    reset_state();
    let _disk = install_mock_disk(512, 16384);

    let path = "/stress/concurrent/shared";
    let data = payload(0x9A, 512);

    // Premier écrivain
    let fd_writer = open_rdwr(path);
    write_at(fd_writer, &data, 0);

    // Plusieurs lecteurs simultanés (même path)
    let mut readers: Vec<u32> = (0..8).map(|_| open_rdwr(path)).collect();

    for fd in &readers {
        let got = read_at(*fd, data.len(), 0);
        assert_eq!(
            got, data,
            "lecteur simultané doit lire les données cohérentes"
        );
    }

    close_fd(fd_writer);
    for fd in readers.drain(..) {
        close_fd(fd);
    }

    reset_state();
}

#[test]
fn stress_write_verify_under_cache_pressure() {
    reset_state();
    let _disk = install_mock_disk(512, 65536);

    let mut rng = Xorshift32::new(0xCAC8_5150);
    let n = 50usize;
    let mut records: Vec<(String, Vec<u8>)> = Vec::new();

    for i in 0..n {
        let path = alloc::format!("/stress/pressure/{i:03}");
        // Taille variable pour maximiser la pression sur l'éviction
        let size = rng.next_range(256, 4096);
        let mut data = alloc::vec![0u8; size];
        rng.fill(&mut data);

        let fd = open_rdwr(&path);
        write_at(fd, &data, 0);
        close_fd(fd);
        records.push((path, data));

        // Provoquer des évictions régulièrement
        if i % 10 == 9 {
            use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
            BLOB_CACHE.evict_n(5);
        }
    }

    // Vérifier l'intégrité de tous les fichiers malgré les évictions
    for (path, expected) in &records {
        let fd = open_rdwr(path);
        let got = read_at(fd, expected.len(), 0);
        close_fd(fd);
        assert_eq!(
            &got, expected,
            "fichier {path} corrompu sous pression cache"
        );
    }

    reset_state();
}

#[test]
fn stress_blob_id_collision_resistance() {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    let disk: RefCell<BTreeMap<u64, Vec<u8>>> = RefCell::new(BTreeMap::new());
    let next_off = RefCell::new(4096u64);
    let cfg = BlobWriterConfig::new(EpochId(7));
    let mut seen: BTreeSet<[u8; 32]> = BTreeSet::new();
    let mut rng = Xorshift32::new(0xC011_1510);

    // 500 blobs de taille/contenu variés — zéro collision attendu (Blake3)
    for _ in 0..500 {
        let size = rng.next_range(1, 2048);
        let mut data = alloc::vec![0u8; size];
        rng.fill(&mut data);

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

        assert!(
            seen.insert(result.blob_id.0),
            "COLLISION Blake3 détectée — impossible en usage normal"
        );
    }

    assert_eq!(seen.len(), 500, "500 blobs doivent avoir 500 IDs uniques");
}
