#!/usr/bin/env python3
"""
tools/split_vfs_compat.py
Découpe modulaire de kernel/src/fs/exofs/posix_bridge/vfs_compat.rs (1 508 lignes) en :
  kernel/src/fs/exofs/posix_bridge/vfs_compat/
    - types.rs     : Constantes, file_mode, open_flags, structures VfsInode, VfsDirent, VfsFd
    - fd_table.rs  : OpenFdTable, allocation/gestion des descripteurs VFS et vérification des PIDs
    - directory.rs : DirRecord, DirectoryRegistry (arborescence répertoires en mémoire)
    - persist.rs   : Sérialisation, persistance et chargement de l'arborescence VFS sur disque ExoFS
    - ops.rs       : Fonctions d'accès VFS (lookup, create, open, read, write, getattr, readdir, ...)
    - helpers.rs   : Helpers internes (blob_id_for_object, resize_inode_data, entry_to_vfs_inode, validate_name, hash_name)
    - tests.rs     : Tests unitaires de conformité VFS
    - mod.rs       : Façade et réexports complets
"""

import os
import subprocess

SRC_PATH = os.path.join("kernel", "src", "fs", "exofs", "posix_bridge", "vfs_compat.rs")
OUT_DIR = os.path.join("kernel", "src", "fs", "exofs", "posix_bridge", "vfs_compat")

def main():
    raw_text = subprocess.check_output(
        ['git', 'show', 'HEAD:' + SRC_PATH.replace('\\', '/')],
        text=True,
        encoding='utf-8'
    )
    lines = raw_text.splitlines(keepends=True)

    os.makedirs(OUT_DIR, exist_ok=True)

    def slice_lines(start, end):
        return "".join(lines[start - 1 : end])

    def make_pub(code):
        out = []
        in_struct = False
        for line in code.splitlines():
            stripped = line.strip()
            indent = line[: len(line) - len(line.lstrip())]
            if stripped.startswith("fn "):
                if stripped.startswith("fn fmt(") or stripped.startswith("fn default("):
                    line = indent + stripped
                else:
                    line = indent + "pub fn " + stripped[3:]
            elif stripped.startswith("const ") and not stripped.startswith("pub const ") and not stripped.startswith("const _"):
                line = indent + "pub const " + stripped[len("const ") :]
            elif stripped.startswith("static ") and not stripped.startswith("pub static "):
                line = indent + "pub static " + stripped[len("static ") :]
            elif stripped.startswith("struct ") and not stripped.startswith("pub struct "):
                line = indent + "pub struct " + stripped[len("struct ") :]
                if "{" in stripped:
                    in_struct = True
            elif in_struct:
                if stripped.startswith("}"):
                    in_struct = False
                elif ":" in stripped and not stripped.startswith("//") and not stripped.startswith("pub "):
                    line = indent + "pub " + stripped
            out.append(line)
        return "\n".join(out) + "\n"

    COMMON_HEADER = """#![allow(unused_imports, dead_code)]
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::posix_bridge::inode_emulation::{
    decode_inode_entry, encode_inode_entry, inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
};
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::object_write::{persist_cached_blob_if_disk, reject_if_immutable};
use crate::fs::exofs::syscall::quota_query::{check_quota, quota_add_usage, quota_sub_usage};

use super::*;
"""

    # 1. TYPES.RS : L24..L161
    types_raw = slice_lines(24, 161)
    types_code = """#![allow(unused_imports, dead_code)]
use crate::fs::exofs::posix_bridge::inode_emulation::ObjectIno;

""" + make_pub(types_raw)
    with open(os.path.join(OUT_DIR, "types.rs"), "w", encoding="utf-8") as f:
        f.write(types_code)
    print("Wrote types.rs")

    # 2. FD_TABLE.RS : L162..L365
    fd_table_raw = slice_lines(162, 365)
    fd_table_code = COMMON_HEADER + "\n" + make_pub(fd_table_raw)
    with open(os.path.join(OUT_DIR, "fd_table.rs"), "w", encoding="utf-8") as f:
        f.write(fd_table_code)
    print("Wrote fd_table.rs")

    # 3. DIRECTORY.RS : L366..L649
    directory_raw = slice_lines(366, 649)
    directory_code = COMMON_HEADER + "\n" + make_pub(directory_raw)
    with open(os.path.join(OUT_DIR, "directory.rs"), "w", encoding="utf-8") as f:
        f.write(directory_code)
    print("Wrote directory.rs")

    # 4. PERSIST.RS : L650..L869
    persist_raw = slice_lines(650, 869)
    persist_code = COMMON_HEADER + "\n" + make_pub(persist_raw)
    with open(os.path.join(OUT_DIR, "persist.rs"), "w", encoding="utf-8") as f:
        f.write(persist_code)
    print("Wrote persist.rs")

    # 5. OPS.RS : L870..L1260
    ops_raw = slice_lines(870, 1260)
    ops_code = COMMON_HEADER + "\n" + ops_raw
    with open(os.path.join(OUT_DIR, "ops.rs"), "w", encoding="utf-8") as f:
        f.write(ops_code)
    print("Wrote ops.rs")

    # 6. HELPERS.RS : L1261..L1382
    helpers_raw = slice_lines(1261, 1382)
    helpers_code = COMMON_HEADER + "\n" + make_pub(helpers_raw)
    with open(os.path.join(OUT_DIR, "helpers.rs"), "w", encoding="utf-8") as f:
        f.write(helpers_code)
    print("Wrote helpers.rs")

    # 7. TESTS.RS : L1383..L1509
    tests_raw = slice_lines(1383, len(lines))
    tests_code = COMMON_HEADER + "\n" + tests_raw
    with open(os.path.join(OUT_DIR, "tests.rs"), "w", encoding="utf-8") as f:
        f.write(tests_code)
    print("Wrote tests.rs")

    # 8. MOD.RS : Façade et réexports
    mod_code = """//! # kernel::fs::exofs::posix_bridge::vfs_compat — Couche de compatibilité VFS
//!
//! Organisation modulaire ségréguée :
//! - `types`     : Constantes POSIX, structures VfsInode, VfsDirent, VfsFd
//! - `fd_table`  : Table des descripteurs de fichiers ouverts et isolation par PID
//! - `directory` : Registre des répertoires et hiérarchie en mémoire
//! - `persist`   : Persistance et sérialisation du namespace sur disque ExoFS
//! - `ops`       : Surface POSIX (lookup, create, open, read, write, getattr, readdir, ...)
//! - `helpers`   : Helpers internes de conversion d'inodes, hashing et validation de noms
//! - `tests`     : Tests unitaires de conformité

pub mod types;
pub mod fd_table;
pub mod directory;
pub mod persist;
pub mod ops;
pub mod helpers;
#[cfg(test)]
pub mod tests;

pub use types::*;
pub use fd_table::*;
pub use directory::*;
pub use persist::*;
pub use ops::*;
pub use helpers::*;
"""
    with open(os.path.join(OUT_DIR, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(mod_code)
    print("Wrote mod.rs")

    # Supprimer l'ancien vfs_compat.rs s'il existe
    if os.path.isfile(SRC_PATH):
        os.remove(SRC_PATH)
        print("Removed monolithic vfs_compat.rs")

if __name__ == "__main__":
    main()
