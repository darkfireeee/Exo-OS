//! # syscall/fs_bridge/special/pseudo_blob.rs — Typed `KernelFdKind` seam & pseudo-blob lifecycle
//!
//! Provides a typed `KernelFdKind` classifier separating genuine ExoFS storage
//! objects from kernel-managed POSIX IPC/pseudo descriptors (pipes, eventfds,
//! `AF_UNIX`/`AF_INET` sockets, epoll, memfd, inotify) without breaking handle
//! numbers across `dup`/`SCM_RIGHTS`/`fork`/`poll`.

use core::sync::atomic::Ordering;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::path::symlink::invalidate_symlink;
use crate::fs::exofs::syscall::object_fd::OBJECT_TABLE;
use crate::syscall::fs_bridge::{
    blob_id_to_object_id, on_socket_blob_closed, remove_blob_edge, remove_epoll_watches_for_blob,
    remove_memfd_seals, remove_mode, remove_times, take_deferred_unlink, NEXT_PSEUDO_ID,
    PSEUDO_EPOLL_TAG, PSEUDO_EVENTFD_TAG, PSEUDO_INOTIFY_TAG, PSEUDO_MEMFD_TAG,
    PSEUDO_NET_SOCKET_TAG, PSEUDO_PIPE_TAG, PSEUDO_SOCKET_TAG,
};

/// Typed classification of kernel open file handles (decoupling POSIX pseudo-FDs
/// from genuine ExoFS storage objects).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelFdKind {
    /// Real ExoFS persisted or staged object.
    ExoFsObject(BlobId),
    /// Anonymous POSIX pipe (`pipe` / `pipe2`).
    Pipe(u64),
    /// `eventfd` / `eventfd2` counter.
    EventFd(u64),
    /// `AF_UNIX` stream or datagram socket.
    UnixSocket(u64),
    /// `AF_INET` / `AF_INET6` network socket bridged to `network_server`.
    NetSocket(u64),
    /// `epoll` readiness instance.
    Epoll(u64),
    /// Anonymous memory file descriptor (`memfd_create`).
    MemFd(u64),
    /// `inotify` watcher instance.
    Inotify(u64),
}

impl KernelFdKind {
    /// Classifies an open handle's `BlobId` into its typed kernel descriptor kind.
    #[inline]
    pub fn classify(blob_id: BlobId) -> Self {
        let bytes = blob_id.as_bytes();
        if bytes[1] == b'E' && bytes[2] == b'X' && bytes[3] == b'O' {
            let seq = u64::from_le_bytes([
                bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11],
            ]);
            match bytes[0] {
                PSEUDO_PIPE_TAG => return Self::Pipe(seq),
                PSEUDO_EVENTFD_TAG => return Self::EventFd(seq),
                PSEUDO_SOCKET_TAG => return Self::UnixSocket(seq),
                PSEUDO_NET_SOCKET_TAG => return Self::NetSocket(seq),
                PSEUDO_EPOLL_TAG => return Self::Epoll(seq),
                PSEUDO_MEMFD_TAG => return Self::MemFd(seq),
                PSEUDO_INOTIFY_TAG => return Self::Inotify(seq),
                _ => {}
            }
        }
        Self::ExoFsObject(blob_id)
    }

    /// Returns `true` when this descriptor refers to a genuine ExoFS storage object.
    #[inline]
    pub const fn is_exofs_storage(self) -> bool {
        matches!(self, Self::ExoFsObject(_))
    }
}

#[inline]
pub fn pseudo_blob_id(tag: u8, seq: u64) -> BlobId {
    let mut bytes = [0u8; 32];
    bytes[0] = tag;
    bytes[1] = b'E';
    bytes[2] = b'X';
    bytes[3] = b'O';
    bytes[4..12].copy_from_slice(&seq.to_le_bytes());
    BlobId(bytes)
}

#[inline]
pub fn next_pseudo_blob(tag: u8) -> BlobId {
    let seq = NEXT_PSEUDO_ID.fetch_add(1, Ordering::Relaxed);
    pseudo_blob_id(tag, seq)
}

#[inline]
pub fn is_pseudo_blob(blob_id: &BlobId, tag: u8) -> bool {
    let bytes = blob_id.as_bytes();
    bytes[0] == tag && bytes[1] == b'E' && bytes[2] == b'X' && bytes[3] == b'O'
}

/// Clean up kernel-side pseudo-blob state (or POSIX deferred-unlinked regular files)
/// once `OBJECT_TABLE.open_count_for(&blob_id) == 0`.
pub fn cleanup_closed_blob(blob_id: BlobId) {
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return;
    }
    remove_blob_edge(&blob_id);
    match KernelFdKind::classify(blob_id) {
        KernelFdKind::UnixSocket(_) => on_socket_blob_closed(blob_id),
        KernelFdKind::NetSocket(_) => {
            crate::syscall::net_bridge::on_net_socket_blob_closed(blob_id);
            BLOB_CACHE.invalidate(&blob_id);
        }
        KernelFdKind::Epoll(_) => {
            remove_epoll_watches_for_blob(blob_id);
            BLOB_CACHE.invalidate(&blob_id);
        }
        KernelFdKind::MemFd(_) => {
            remove_memfd_seals(&blob_id);
            BLOB_CACHE.invalidate(&blob_id);
        }
        KernelFdKind::Pipe(_) | KernelFdKind::EventFd(_) | KernelFdKind::Inotify(_) => {
            BLOB_CACHE.invalidate(&blob_id);
        }
        KernelFdKind::ExoFsObject(_) => {
            if let Some(is_symlink) = take_deferred_unlink(&blob_id) {
                if is_symlink {
                    invalidate_symlink(&blob_id_to_object_id(blob_id));
                }
                BLOB_CACHE.invalidate(&blob_id);
                remove_mode(&blob_id);
                remove_times(&blob_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_distinguishes_pseudo_fds_from_exofs_objects() {
        assert_eq!(
            KernelFdKind::classify(pseudo_blob_id(PSEUDO_PIPE_TAG, 7)),
            KernelFdKind::Pipe(7)
        );
        assert_eq!(
            KernelFdKind::classify(pseudo_blob_id(PSEUDO_EVENTFD_TAG, 9)),
            KernelFdKind::EventFd(9)
        );
        assert_eq!(
            KernelFdKind::classify(pseudo_blob_id(PSEUDO_SOCKET_TAG, 11)),
            KernelFdKind::UnixSocket(11)
        );
        assert_eq!(
            KernelFdKind::classify(pseudo_blob_id(PSEUDO_NET_SOCKET_TAG, 13)),
            KernelFdKind::NetSocket(13)
        );
        let real = BlobId([0x42; 32]);
        assert!(KernelFdKind::classify(real).is_exofs_storage());
    }
}
