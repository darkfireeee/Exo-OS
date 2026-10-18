#!/usr/bin/env python3
"""
tools/split_fs_bridge.py
Découpe automatique, robuste et vérifiée par AST/items de kernel/src/syscall/fs_bridge.rs
en 8 sous-modules spécialisés + mod.rs dans kernel/src/syscall/fs_bridge/ :
  - mod.rs       : Façade publique, état FS_READY, fs_bridge_init, is_fs_ready
  - types.rs     : FsBridgeError, structures ABI POSIX, conversions, modes, inodes
  - cwd.rs       : CWD_MAP, cwd_release_pid, chdir, getcwd, umask, rlimit
  - fd.rs        : Résolution et gestion des FDs (resolve_fd, install_fd, close, dup, fcntl, flock)
  - special.rs   : TTY, pseudo-blobs (pipe, socket, eventfd), splice, tee, vmsplice
  - file_ops.rs  : Primitives de blobs, read, write, lseek, truncate, fsync, fallocate, ioctl
  - path_ops.rs  : Chemins, résolutions d'index, symlinks, open, mkdir, unlink, rename, stat...
  - poll.rs      : Poll, select, epoll, inotify
  - tests.rs     : Tests unitaires (mod tests)
"""

import subprocess
import os
import re
from collections import defaultdict

OUT_DIR = os.path.join("kernel", "src", "syscall", "fs_bridge")

COMMON_IMPORTS = """#![allow(unused_imports)]
use super::*;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use spin::Mutex as SpinMutex;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, ExofsError, ObjectId};
use crate::fs::exofs::path::path_component::{PathComponent, PathComponentBuf};
use crate::fs::exofs::path::path_index::{
    PathIndex, PathIndexHeader, PATH_INDEX_MAGIC, PATH_INDEX_VERSION,
};
use crate::fs::exofs::path::symlink::{
    invalidate_symlink, is_valid_symlink_target, register_symlink, SYMLINK_MAX_DEPTH,
};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::fs::exofs::syscall::object_store;
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::syscall::validation::{copy_from_user, copy_to_user, read_user_typed, write_user_typed};
"""

def extract_items_from_git():
    out = subprocess.check_output(
        ['git', 'show', 'HEAD:kernel/src/syscall/fs_bridge.rs'],
        text=True,
        encoding='utf-8'
    )
    lines = out.splitlines()

    items = []
    in_item = False
    brace_depth = 0
    comment_start = None

    for i, line in enumerate(lines):
        s = line.strip()
        if not in_item:
            if s.startswith('//') or s.startswith('/*') or s.startswith('#['):
                if comment_start is None:
                    comment_start = i
                continue
            if any(s.startswith(k + ' ') or s.startswith('pub ' + k + ' ') or s.startswith('pub(crate) ' + k + ' ') or s.startswith('unsafe ' + k + ' ') or s.startswith('pub unsafe ' + k + ' ') for k in ['fn', 'struct', 'enum', 'impl', 'static', 'const', 'type', 'mod']):
                in_item = True
                item_start = comment_start if comment_start is not None else i
                comment_start = None
                brace_depth = 0
            else:
                comment_start = None
                continue

        if in_item:
            brace_depth += line.count('{') - line.count('}')
            if brace_depth == 0 and (line.endswith(';') or line.strip() == '}' or line.endswith('}')):
                items.append((item_start, i, lines[item_start:i+1]))
                in_item = False
                comment_start = None

    return items

def get_symbol_name(chunk):
    for l in chunk:
        s = l.strip()
        for k in ['fn', 'struct', 'enum', 'impl', 'static', 'const', 'type', 'mod']:
            pattern = rf'^(?:pub\s+)?(?:unsafe\s+)?{k}\s+([A-Za-z0-9_]+)'
            m = re.match(pattern, s)
            if m:
                return m.group(1)
            if s.startswith('impl ') or s.startswith('impl<'):
                m = re.match(r'^impl(?:\s*<[^>]+>)?\s+([A-Za-z0-9_]+)', s)
                if m:
                    return m.group(1)
    return 'UNKNOWN'

def adjust_item_visibility(chunk, target_name):
    # Rendre les éléments racine pub ou pub(crate) pour visibilité inter-modules
    adjusted = []
    in_struct = False
    for line in chunk:
        stripped = line.strip()
        indent = line[:len(line) - len(line.lstrip())]

        # Détection début de struct
        if re.match(r'^(?:pub\s+)?struct\s+[A-Za-z0-9_]+', stripped) and '{' in stripped:
            in_struct = True
            if stripped.startswith('struct '):
                line = indent + 'pub ' + stripped
            adjusted.append(line)
            continue
        elif in_struct:
            if stripped == '}':
                in_struct = False
                adjusted.append(line)
                continue
            # rendre les champs de struct publics s'ils ne le sont pas déjà et ne commencent pas par un attribut ou commentaire
            if not stripped.startswith('//') and not stripped.startswith('#[') and not stripped.startswith('pub '):
                # c'est un champ comme "name: Type,"
                if ':' in stripped:
                    line = indent + 'pub ' + stripped
            adjusted.append(line)
            continue

        # Hors struct: fonctions, statics, consts, enums
        if stripped.startswith('fn '):
            line = indent + 'pub ' + stripped
        elif stripped.startswith('unsafe fn '):
            line = indent + 'pub unsafe fn ' + stripped[len('unsafe fn '):]
        elif stripped.startswith('struct '):
            line = indent + 'pub ' + stripped
        elif stripped.startswith('static ') and not stripped.startswith('static FS_READY'):
            line = indent + 'pub ' + stripped
        elif stripped.startswith('static mut ') and not stripped.startswith('static mut FS_READY'):
            line = indent + 'pub ' + stripped
        elif stripped.startswith('const ') and not stripped.startswith('const _'):
            line = indent + 'pub ' + stripped

        adjusted.append(line)
    return '\n'.join(adjusted)

def main():
    items = extract_items_from_git()
    print(f"Total extracted items: {len(items)}")

    CWD_SYMS = {'CWD_MAX_LEN', 'CWD_MAP_SIZE', 'CwdEntry', 'CwdMap', 'CWD_LOCK', 'CWD_MAP', 'cwd_slot_for_pid', 'cwd_release_pid', 'process_umask', 'swap_process_umask', 'apply_umask', 'fs_chdir', 'fs_fchdir', 'fs_getcwd', 'fs_umask', 'fs_getrlimit', 'fs_setrlimit'}
    FD_SYMS = {'ResolvedFd', 'fd_can_read_flags', 'fd_can_write_flags', 'fd_table_flags', 'process_has_fd_table', 'process_fd_descriptor', 'resolve_fd', 'install_process_fd', 'install_process_fd_at', 'close_process_fd', 'set_process_fd_flags', 'fd_readiness', 'flock_range_from_abi', 'fs_close', 'fs_dup', 'fs_dup2', 'fs_fcntl', 'fs_flock'}
    SPECIAL_SYMS = {'TTY_E9ONLY_LOGGED', 'TtyRequestWire', 'TtyReplyWire', 'TtyInputBuffer', 'is_tty_handle', 'is_tty_handle_u64', 'is_tty_path', 'tty_endpoint', 'tty_call', 'tty_send', 'tty_write_bytes', 'tty_refill_stdin', 'tty_read_bytes', 'pseudo_blob_id', 'next_pseudo_blob', 'is_pseudo_blob', 'eventfd_state', 'store_eventfd_state', 'socket_blob_with_peer', 'socket_peer_blob', 'socket_payload_len', 'read_socket_payload', 'append_socket_payload', 'read_pipe_payload', 'append_pipe_payload', 'fs_pipe2', 'fs_eventfd2', 'fs_socketpair', 'fs_splice', 'fs_tee', 'fs_vmsplice'}
    FILE_SYMS = {'blob_len', 'ensure_blob_exists', 'snapshot_blob', 'resize_regular_blob', 'read_blob_at', 'write_blob_at', 'read_blob_bytes_at', 'write_blob_bytes_at', 'fs_read', 'fs_write', 'fs_pread64', 'fs_pwrite64', 'fs_readv', 'fs_writev', 'fs_preadv', 'fs_pwritev', 'fs_lseek', 'fs_truncate', 'fs_ftruncate', 'fs_fsync', 'fs_fallocate', 'fs_copy_file_range', 'fs_sendfile', 'fs_fadvise64', 'fs_ioctl'}
    PATH_SYMS = {'validate_path', 'normalize_path_buf', 'path_buf_to_bytes', 'normalized_path_bytes', 'split_parent_and_leaf', 'blob_id_for_path', 'blob_id_to_object_id', 'object_id_to_blob_id', 'ensure_root_directory', 'load_path_index', 'store_path_index', 'ensure_directory_exists', 'ensure_directory_chain', 'upsert_parent_entry', 'remove_parent_entry', 'directory_mount_key', 'path_index_entry_count', 'blob_is_directory', 'blob_is_directory_by_id', 'path_entry', 'resolve_path_with_symlinks', 'fs_open', 'fs_openat', 'fs_stat', 'fs_lstat', 'fs_fstat', 'fs_statx', 'fs_statfs', 'fs_fstatfs', 'fs_access', 'fs_chmod', 'fs_fchmod', 'fs_chown', 'fs_fchown', 'fs_mkdir', 'fs_rmdir', 'fs_unlink', 'fs_rename', 'fs_renameat', 'fs_link', 'fs_linkat', 'fs_symlink', 'fs_symlinkat', 'fs_readlink', 'fs_readlinkat', 'fs_getdents64', 'fs_mknod', 'fs_mknodat', 'fs_sync', 'fs_sync_file_range'}
    POLL_SYMS = {'set_fdset_bit', 'fdset_bit', 'fs_poll', 'fs_select', 'fs_epoll_create1', 'fs_epoll_ctl', 'fs_epoll_wait', 'fs_inotify_init1'}
    MOD_SYMS = {'FS_READY', 'fs_bridge_init', 'is_fs_ready'}
    TESTS_SYMS = {'tests'}

    targets = defaultdict(list)
    for start, end, chunk in items:
        sym = get_symbol_name(chunk)
        if sym in MOD_SYMS:
            targets['mod'].append((sym, chunk))
        elif sym in CWD_SYMS:
            targets['cwd'].append((sym, chunk))
        elif sym in FD_SYMS:
            targets['fd'].append((sym, chunk))
        elif sym in SPECIAL_SYMS:
            targets['special'].append((sym, chunk))
        elif sym in FILE_SYMS:
            targets['file_ops'].append((sym, chunk))
        elif sym in PATH_SYMS:
            targets['path_ops'].append((sym, chunk))
        elif sym in POLL_SYMS:
            targets['poll'].append((sym, chunk))
        elif sym in TESTS_SYMS:
            targets['tests'].append((sym, chunk))
        else:
            targets['types'].append((sym, chunk))

    os.makedirs(OUT_DIR, exist_ok=True)

    # Ecriture mod.rs
    mod_code = """//! # syscall/fs_bridge — Pont VFS / POSIX d'Exo-OS
//!
//! Ce module ségrègue les fonctionnalités du pont FS en sous-modules spécialisés :
//! - `types`     : Erreurs FsBridgeError, structures ABI POSIX (Stat, Dirent...) et conversions
//! - `cwd`       : Répertoire de travail courant par processus (CWD), umask et limites rlimit
//! - `fd`        : Résolution et gestion des descripteurs de fichiers (open/close/dup/fcntl/flock)
//! - `special`   : Périphériques TTY, pseudo-fichiers IPC (pipe, socketpair, eventfd, splice)
//! - `file_ops`  : Primitives d'accès direct aux blobs et opérations d'I/O (read, write, lseek...)
//! - `path_ops`  : Résolution de chemins arborescents, indexation, symlinks et métadonnées
//! - `poll`      : Multiplexage I/O synchrone et asynchrone (poll, select, epoll, inotify)
//! - `tests`     : Suite de tests unitaires et d'intégration

use core::sync::atomic::{AtomicBool, Ordering};

pub mod types;
pub mod cwd;
pub mod fd;
pub mod special;
pub mod file_ops;
pub mod path_ops;
pub mod poll;

#[cfg(test)]
pub mod tests;

pub use types::*;
pub use cwd::*;
pub use fd::*;
pub use special::*;
pub use file_ops::*;
pub use path_ops::*;
pub use poll::*;

/// Indique si le sous-système fs/ est initialisé et prêt.
/// Mis à `true` par `fs_bridge_init()` lors du boot.
static FS_READY: AtomicBool = AtomicBool::new(false);

/// Initialise le bridge fs.
/// À appeler depuis la séquence de boot après `fs::init()`.
///
/// # Safety
/// Doit être appelé exactement une fois depuis le BSP.
pub unsafe fn fs_bridge_init() {
    FS_READY.store(true, Ordering::Release);
}

/// Retourne `true` si le sous-système fs/ est prêt.
#[inline(always)]
pub fn is_fs_ready() -> bool {
    FS_READY.load(Ordering::Acquire)
}
"""
    with open(os.path.join(OUT_DIR, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(mod_code)

    # Ecriture des sous-modules
    modules = ['types', 'cwd', 'fd', 'special', 'file_ops', 'path_ops', 'poll']
    for m in modules:
        content = f"//! # syscall/fs_bridge/{m}.rs\n\n" + COMMON_IMPORTS + "\n"
        for sym, chunk in targets[m]:
            content += adjust_item_visibility(chunk, m) + "\n\n"
        with open(os.path.join(OUT_DIR, f"{m}.rs"), "w", encoding="utf-8") as f:
            f.write(content)
        print(f"Wrote {m}.rs ({len(targets[m])} items)")

    # Ecriture tests.rs
    tests_content = "//! Tests unitaires du pont FS\nuse super::*;\n\n"
    for sym, chunk in targets['tests']:
        # chunk contains `#[cfg(test)] mod tests { ... }`
        # On extrait juste le corps ou on met tel quel
        tests_content += '\n'.join(chunk) + "\n"
    with open(os.path.join(OUT_DIR, "tests.rs"), "w", encoding="utf-8") as f:
        f.write(tests_content)
    print("Wrote tests.rs")

if __name__ == "__main__":
    main()
