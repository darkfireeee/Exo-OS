//! Native Exo-OS (`target_os = "exo"`) and Linux implementation of the `rustix` subset
//! required by `wayland-backend`, `wayland-client`, `wayland-server`, and `wayland-cursor`.

#![allow(dead_code, unused_variables)]

use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

#[macro_export]
macro_rules! cmsg_space {
    (ScmRights($len:expr)) => {
        16 + ((($len) * ::core::mem::size_of::<i32>() + 7) & !7)
    };
}

#[cfg(target_arch = "x86_64")]
mod raw_sys {
    pub const SYS_POLL: u64 = 7;
    pub const SYS_SENDTO: u64 = 44;
    pub const SYS_RECVFROM: u64 = 45;
    pub const SYS_SENDMSG: u64 = 46;
    pub const SYS_RECVMSG: u64 = 47;
    pub const SYS_FCNTL: u64 = 72;
    pub const SYS_FLOCK: u64 = 73;
    pub const SYS_EPOLL_WAIT: u64 = 232;
    pub const SYS_EPOLL_CTL: u64 = 233;
    pub const SYS_EPOLL_CREATE1: u64 = 291;
    pub const SYS_MEMFD_CREATE: u64 = 319;

    #[inline(always)]
    pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
        let ret: i64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") nr => ret,
                in("rdi") a1,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack)
            );
        }
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
        let ret: i64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") nr => ret,
                in("rdi") a1,
                in("rsi") a2,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack)
            );
        }
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
        let ret: i64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") nr => ret,
                in("rdi") a1,
                in("rsi") a2,
                in("rdx") a3,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack)
            );
        }
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
        let ret: i64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") nr => ret,
                in("rdi") a1,
                in("rsi") a2,
                in("rdx") a3,
                in("r10") a4,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack)
            );
        }
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
        let ret: i64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") nr => ret,
                in("rdi") a1,
                in("rsi") a2,
                in("rdx") a3,
                in("r10") a4,
                in("r8") a5,
                in("r9") a6,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack)
            );
        }
        ret
    }

    #[inline(always)]
    pub fn decode(ret: i64) -> crate::io::Result<usize> {
        if ret < 0 && ret >= -4095 {
            Err(crate::io::Errno((-ret) as i32))
        } else {
            Ok(ret as usize)
        }
    }
}

pub mod io {
    use super::*;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct Errno(pub i32);

    impl Errno {
        pub const PERM: Self = Self(1);
        pub const NOENT: Self = Self(2);
        pub const INTR: Self = Self(4);
        pub const IO: Self = Self(5);
        pub const TOOBIG: Self = Self(7);
        pub const BADF: Self = Self(9);
        pub const AGAIN: Self = Self(11);
        pub const WOULDBLOCK: Self = Self(11);
        pub const NOMEM: Self = Self(12);
        pub const ACCES: Self = Self(13);
        pub const FAULT: Self = Self(14);
        pub const EXIST: Self = Self(17);
        pub const INVAL: Self = Self(22);
        pub const PIPE: Self = Self(32);
        pub const NOSYS: Self = Self(38);
        pub const PROTO: Self = Self(71);

        #[inline]
        pub const fn raw_os_error(self) -> i32 {
            self.0
        }
    }

    impl From<Errno> for std::io::Error {
        #[inline]
        fn from(e: Errno) -> Self {
            std::io::Error::from_raw_os_error(e.0)
        }
    }

    pub type Result<T> = core::result::Result<T, Errno>;

    #[inline]
    pub fn retry_on_intr<F: FnMut() -> Result<T>, T>(mut f: F) -> Result<T> {
        loop {
            match f() {
                Err(Errno::INTR) => continue,
                res => return res,
            }
        }
    }

    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub struct FdFlags: u32 {
            const CLOEXEC = 1;
        }
    }

    pub fn fcntl_getfd<Fd: AsFd>(fd: Fd) -> Result<FdFlags> {
        let raw = fd.as_fd().as_raw_fd();
        let ret = unsafe { raw_sys::syscall3(raw_sys::SYS_FCNTL, raw as u64, 1, 0) };
        let val = raw_sys::decode(ret)? as u32;
        Ok(FdFlags::from_bits_truncate(val))
    }

    pub fn fcntl_setfd<Fd: AsFd>(fd: Fd, flags: FdFlags) -> Result<()> {
        let raw = fd.as_fd().as_raw_fd();
        let ret = unsafe {
            raw_sys::syscall3(raw_sys::SYS_FCNTL, raw as u64, 2, flags.bits() as u64)
        };
        raw_sys::decode(ret).map(|_| ())
    }
}

pub mod buffer {
    pub struct SpareCapacity<'a, T>(pub(crate) &'a mut Vec<T>);

    pub fn spare_capacity<T>(v: &mut Vec<T>) -> SpareCapacity<'_, T> {
        v.clear();
        SpareCapacity(v)
    }
}

pub mod process {
    pub type RawPid = i32;
    pub type RawUid = u32;
    pub type RawGid = u32;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct Pid(pub(crate) RawPid);

    impl Pid {
        #[inline]
        pub fn as_raw(pid: Option<Self>) -> RawPid {
            pid.map(|p| p.0).unwrap_or(0)
        }
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct Uid(pub(crate) RawUid);

    impl Uid {
        #[inline]
        pub fn as_raw(self) -> RawUid {
            self.0
        }
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct Gid(pub(crate) RawGid);

    impl Gid {
        #[inline]
        pub fn as_raw(self) -> RawGid {
            self.0
        }
    }
}

pub mod net {
    use super::*;

    pub const SOL_SOCKET: i32 = 1;
    pub const SCM_RIGHTS: i32 = 1;

    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub struct SendFlags: u32 {
            const DONTWAIT = 0x40;
            const NOSIGNAL = 0x4000;
        }
    }

    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub struct RecvFlags: u32 {
            const DONTWAIT = 0x40;
            const CMSG_CLOEXEC = 0x4000_0000;
            const TRUNC = 0x20;
        }
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct RawMsghdr {
        msg_name: *mut u8,
        msg_namelen: u32,
        _pad0: u32,
        msg_iov: *mut IoSlice<'static>,
        msg_iovlen: usize,
        msg_control: *mut u8,
        msg_controllen: usize,
        msg_flags: i32,
        _pad1: i32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct RawCmsghdr {
        cmsg_len: usize,
        cmsg_level: i32,
        cmsg_type: i32,
    }

    #[non_exhaustive]
    pub enum SendAncillaryMessage<'a> {
        ScmRights(&'a [BorrowedFd<'a>]),
    }

    pub struct SendAncillaryBuffer<'a> {
        space: &'a mut [MaybeUninit<u8>],
        used: usize,
    }

    impl<'a> SendAncillaryBuffer<'a> {
        pub fn new(space: &'a mut [MaybeUninit<u8>]) -> Self {
            Self { space, used: 0 }
        }

        pub fn push(&mut self, msg: SendAncillaryMessage<'_>) -> bool {
            match msg {
                SendAncillaryMessage::ScmRights(fds) => {
                    let payload_bytes = fds.len() * core::mem::size_of::<RawFd>();
                    let cmsg_len = 16 + payload_bytes;
                    let aligned_len = (cmsg_len + 7) & !7;
                    if self.used + aligned_len > self.space.len() {
                        return false;
                    }
                    let base = unsafe { self.space.as_mut_ptr().add(self.used) as *mut u8 };
                    unsafe {
                        core::ptr::write_bytes(base, 0, aligned_len);
                        let hdr = RawCmsghdr {
                            cmsg_len,
                            cmsg_level: SOL_SOCKET,
                            cmsg_type: SCM_RIGHTS,
                        };
                        core::ptr::write_unaligned(base as *mut RawCmsghdr, hdr);
                        let fd_dst = base.add(16) as *mut RawFd;
                        for (i, fd) in fds.iter().enumerate() {
                            core::ptr::write_unaligned(fd_dst.add(i), fd.as_raw_fd());
                        }
                    }
                    self.used += aligned_len;
                    true
                }
            }
        }
    }

    #[non_exhaustive]
    pub enum RecvAncillaryMessage {
        ScmRights(std::vec::IntoIter<OwnedFd>),
    }

    pub struct RecvAncillaryBuffer<'a> {
        space: &'a mut [MaybeUninit<u8>],
        len: usize,
    }

    impl<'a> RecvAncillaryBuffer<'a> {
        pub fn new(space: &'a mut [MaybeUninit<u8>]) -> Self {
            Self { space, len: 0 }
        }

        pub fn drain(&mut self) -> std::vec::IntoIter<RecvAncillaryMessage> {
            let mut out = Vec::new();
            let mut offset = 0usize;
            while offset + 16 <= self.len && offset + 16 <= self.space.len() {
                let base = unsafe { self.space.as_ptr().add(offset) as *const u8 };
                let hdr = unsafe { core::ptr::read_unaligned(base as *const RawCmsghdr) };
                if hdr.cmsg_len < 16 || offset + hdr.cmsg_len > self.len {
                    break;
                }
                if hdr.cmsg_level == SOL_SOCKET && hdr.cmsg_type == SCM_RIGHTS {
                    let count = (hdr.cmsg_len - 16) / core::mem::size_of::<RawFd>();
                    let fd_src = unsafe { base.add(16) as *const RawFd };
                    let mut fds = Vec::with_capacity(count);
                    for i in 0..count {
                        let raw = unsafe { core::ptr::read_unaligned(fd_src.add(i)) };
                        if raw >= 0 {
                            fds.push(unsafe { OwnedFd::from_raw_fd(raw) });
                        }
                    }
                    out.push(RecvAncillaryMessage::ScmRights(fds.into_iter()));
                }
                let aligned = (hdr.cmsg_len + 7) & !7;
                if aligned == 0 {
                    break;
                }
                offset += aligned;
            }
            self.len = 0;
            out.into_iter()
        }
    }

    #[derive(Copy, Clone, Debug)]
    pub struct RecvMsgReturn {
        pub bytes: usize,
        pub flags: RecvFlags,
    }

    pub fn send<Fd: AsFd>(fd: Fd, buf: &[u8], flags: SendFlags) -> crate::io::Result<usize> {
        let raw = fd.as_fd().as_raw_fd();
        let ret = unsafe {
            raw_sys::syscall6(
                raw_sys::SYS_SENDTO,
                raw as u64,
                buf.as_ptr() as u64,
                buf.len() as u64,
                flags.bits() as u64,
                0,
                0,
            )
        };
        raw_sys::decode(ret)
    }

    pub fn sendmsg<Fd: AsFd>(
        fd: Fd,
        iov: &[IoSlice<'_>],
        control: &mut SendAncillaryBuffer<'_>,
        flags: SendFlags,
    ) -> crate::io::Result<usize> {
        let raw = fd.as_fd().as_raw_fd();
        let (ctrl_ptr, ctrl_len) = if control.used > 0 {
            (control.space.as_mut_ptr() as *mut u8, control.used)
        } else {
            (core::ptr::null_mut(), 0)
        };
        let hdr = RawMsghdr {
            msg_name: core::ptr::null_mut(),
            msg_namelen: 0,
            _pad0: 0,
            msg_iov: iov.as_ptr() as *mut IoSlice<'static>,
            msg_iovlen: iov.len(),
            msg_control: ctrl_ptr,
            msg_controllen: ctrl_len,
            msg_flags: 0,
            _pad1: 0,
        };
        let ret = unsafe {
            raw_sys::syscall3(
                raw_sys::SYS_SENDMSG,
                raw as u64,
                &hdr as *const RawMsghdr as u64,
                flags.bits() as u64,
            )
        };
        raw_sys::decode(ret)
    }

    pub fn recvmsg<Fd: AsFd>(
        fd: Fd,
        iov: &mut [IoSliceMut<'_>],
        control: &mut RecvAncillaryBuffer<'_>,
        flags: RecvFlags,
    ) -> crate::io::Result<RecvMsgReturn> {
        let raw = fd.as_fd().as_raw_fd();
        let (ctrl_ptr, ctrl_len) = if !control.space.is_empty() {
            (control.space.as_mut_ptr() as *mut u8, control.space.len())
        } else {
            (core::ptr::null_mut(), 0)
        };
        let mut hdr = RawMsghdr {
            msg_name: core::ptr::null_mut(),
            msg_namelen: 0,
            _pad0: 0,
            msg_iov: iov.as_mut_ptr() as *mut IoSlice<'static>,
            msg_iovlen: iov.len(),
            msg_control: ctrl_ptr,
            msg_controllen: ctrl_len,
            msg_flags: 0,
            _pad1: 0,
        };
        let ret = unsafe {
            raw_sys::syscall3(
                raw_sys::SYS_RECVMSG,
                raw as u64,
                &mut hdr as *mut RawMsghdr as u64,
                flags.bits() as u64,
            )
        };
        let bytes = raw_sys::decode(ret)?;
        control.len = hdr.msg_controllen;
        Ok(RecvMsgReturn {
            bytes,
            flags: RecvFlags::from_bits_truncate(hdr.msg_flags as u32),
        })
    }

    pub mod sockopt {
        use super::*;
        use crate::process::{Gid, Pid, Uid};

        #[derive(Copy, Clone, Debug)]
        pub struct Ucred {
            pub pid: Pid,
            pub uid: Uid,
            pub gid: Gid,
        }

        pub fn socket_peercred<Fd: AsFd>(_fd: Fd) -> crate::io::Result<Ucred> {
            Ok(Ucred {
                pid: Pid(0),
                uid: Uid(0),
                gid: Gid(0),
            })
        }

        pub fn set_socket_nosigpipe<Fd: AsFd>(_fd: Fd, _val: bool) -> crate::io::Result<()> {
            Ok(())
        }
    }
}

pub mod event {
    use super::*;

    #[derive(Copy, Clone, Debug, Default)]
    pub struct Timespec {
        pub tv_sec: i64,
        pub tv_nsec: i64,
    }

    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub struct PollFlags: i16 {
            const IN = 0x0001;
            const PRI = 0x0002;
            const OUT = 0x0004;
            const ERR = 0x0008;
            const HUP = 0x0010;
            const NVAL = 0x0020;
        }
    }

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct PollFd<'a> {
        fd: RawFd,
        events: i16,
        revents: i16,
        _marker: core::marker::PhantomData<&'a ()>,
    }

    impl<'a> PollFd<'a> {
        pub fn new<Fd: AsFd>(fd: &'a Fd, events: PollFlags) -> Self {
            Self {
                fd: fd.as_fd().as_raw_fd(),
                events: events.bits(),
                revents: 0,
                _marker: core::marker::PhantomData,
            }
        }

        pub fn revents(&self) -> PollFlags {
            PollFlags::from_bits_truncate(self.revents)
        }
    }

    pub fn poll(fds: &mut [PollFd<'_>], timeout: Option<&Timespec>) -> crate::io::Result<usize> {
        let timeout_ms: i32 = match timeout {
            Some(ts) => (ts.tv_sec.saturating_mul(1000) + ts.tv_nsec / 1_000_000) as i32,
            None => -1,
        };
        let ret = unsafe {
            raw_sys::syscall3(
                raw_sys::SYS_POLL,
                fds.as_mut_ptr() as u64,
                fds.len() as u64,
                timeout_ms as i64 as u64,
            )
        };
        raw_sys::decode(ret)
    }

    pub mod epoll {
        use super::*;

        pub const EPOLL_CTL_ADD: u64 = 1;
        pub const EPOLL_CTL_DEL: u64 = 2;
        pub const EPOLL_CTL_MOD: u64 = 3;

        bitflags::bitflags! {
            #[derive(Copy, Clone, Debug, PartialEq, Eq)]
            pub struct CreateFlags: u32 {
                const CLOEXEC = 0o2000000;
            }
        }

        bitflags::bitflags! {
            #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
            pub struct EventFlags: u32 {
                const IN = 0x0001;
                const PRI = 0x0002;
                const OUT = 0x0004;
                const ERR = 0x0008;
                const HUP = 0x0010;
                const ET = 1 << 31;
            }
        }

        #[repr(C)]
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
        pub struct EventData(u64);

        impl EventData {
            #[inline]
            pub const fn new_u64(val: u64) -> Self {
                Self(val)
            }

            #[inline]
            pub const fn u64(&self) -> u64 {
                self.0
            }
        }

        #[repr(C)]
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
        pub struct Event {
            pub flags: EventFlags,
            pub data: EventData,
        }

        #[cfg(target_os = "linux")]
        #[repr(C, packed)]
        #[derive(Copy, Clone)]
        struct RawEpollEvent {
            events: u32,
            data: u64,
        }

        #[cfg(not(target_os = "linux"))]
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct RawEpollEvent {
            events: u32,
            data: u64,
        }

        pub fn create(flags: CreateFlags) -> crate::io::Result<OwnedFd> {
            let ret = unsafe {
                raw_sys::syscall1(raw_sys::SYS_EPOLL_CREATE1, flags.bits() as u64)
            };
            let fd = raw_sys::decode(ret)? as RawFd;
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }

        pub fn add<EpFd: AsFd, Fd: AsFd>(
            epfd: EpFd,
            source: Fd,
            data: EventData,
            event_flags: EventFlags,
        ) -> crate::io::Result<()> {
            let raw_ev = RawEpollEvent {
                events: event_flags.bits(),
                data: data.u64(),
            };
            let ret = unsafe {
                raw_sys::syscall4(
                    raw_sys::SYS_EPOLL_CTL,
                    epfd.as_fd().as_raw_fd() as u64,
                    EPOLL_CTL_ADD,
                    source.as_fd().as_raw_fd() as u64,
                    &raw_ev as *const RawEpollEvent as u64,
                )
            };
            raw_sys::decode(ret).map(|_| ())
        }

        pub fn delete<EpFd: AsFd, Fd: AsFd>(epfd: EpFd, source: Fd) -> crate::io::Result<()> {
            let ret = unsafe {
                raw_sys::syscall4(
                    raw_sys::SYS_EPOLL_CTL,
                    epfd.as_fd().as_raw_fd() as u64,
                    EPOLL_CTL_DEL,
                    source.as_fd().as_raw_fd() as u64,
                    0,
                )
            };
            raw_sys::decode(ret).map(|_| ())
        }

        pub fn wait<EpFd: AsFd>(
            epfd: EpFd,
            event_list: crate::buffer::SpareCapacity<'_, Event>,
            timeout: Option<&Timespec>,
        ) -> crate::io::Result<()> {
            let cap = event_list.0.capacity().max(16).min(64);
            let mut raw_events = vec![
                RawEpollEvent {
                    events: 0,
                    data: 0
                };
                cap
            ];
            let timeout_ms: i32 = match timeout {
                Some(ts) => (ts.tv_sec.saturating_mul(1000) + ts.tv_nsec / 1_000_000) as i32,
                None => -1,
            };
            let ret = unsafe {
                raw_sys::syscall4(
                    raw_sys::SYS_EPOLL_WAIT,
                    epfd.as_fd().as_raw_fd() as u64,
                    raw_events.as_mut_ptr() as u64,
                    cap as u64,
                    timeout_ms as i64 as u64,
                )
            };
            let n = raw_sys::decode(ret)?;
            for raw in raw_events.into_iter().take(n) {
                let ev_bits = raw.events;
                let ev_data = raw.data;
                event_list.0.push(Event {
                    flags: EventFlags::from_bits_truncate(ev_bits),
                    data: EventData::new_u64(ev_data),
                });
            }
            Ok(())
        }
    }
}

pub mod fs {
    use super::*;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum FlockOperation {
        LockShared,
        LockExclusive,
        Unlock,
        NonBlockingLockShared,
        NonBlockingLockExclusive,
        NonBlockingUnlock,
    }

    pub fn flock<Fd: AsFd>(fd: Fd, operation: FlockOperation) -> crate::io::Result<()> {
        let op: u64 = match operation {
            FlockOperation::LockShared => 1,
            FlockOperation::LockExclusive => 2,
            FlockOperation::Unlock => 8,
            FlockOperation::NonBlockingLockShared => 1 | 4,
            FlockOperation::NonBlockingLockExclusive => 2 | 4,
            FlockOperation::NonBlockingUnlock => 8 | 4,
        };
        let ret = unsafe {
            raw_sys::syscall2(raw_sys::SYS_FLOCK, fd.as_fd().as_raw_fd() as u64, op)
        };
        raw_sys::decode(ret).map(|_| ())
    }

    #[derive(Copy, Clone, Debug)]
    pub struct Stat {
        pub st_dev: u64,
        pub st_ino: u64,
        pub st_mode: u32,
        pub st_size: i64,
    }

    pub fn fstat<Fd: AsFd>(fd: Fd) -> crate::io::Result<Stat> {
        let raw = fd.as_fd().as_raw_fd();
        let file = core::mem::ManuallyDrop::new(unsafe { File::from_raw_fd(raw) });
        let meta = file.metadata().map_err(|e| {
            crate::io::Errno(e.raw_os_error().unwrap_or(crate::io::Errno::IO.0))
        })?;
        Ok(Stat {
            st_dev: meta.dev(),
            st_ino: meta.ino(),
            st_mode: meta.mode(),
            st_size: meta.size() as i64,
        })
    }

    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub struct Mode: u32 {
            const RUSR = 0o400;
            const WUSR = 0o200;
            const XUSR = 0o100;
            const RGRP = 0o040;
            const WGRP = 0o020;
            const ROOTH = 0o004;
        }
    }

    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub struct OFlags: u32 {
            const RDONLY = 0;
            const WRONLY = 1;
            const RDWR = 2;
            const CREATE = 0x40;
            const EXCL = 0x80;
            const TRUNC = 0x200;
            const CLOEXEC = 0o2000000;
        }
    }

    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub struct MemfdFlags: u32 {
            const CLOEXEC = 0x0001;
        }
    }

    pub fn memfd_create<P: AsRef<CStr>>(name: P, flags: MemfdFlags) -> crate::io::Result<OwnedFd> {
        #[cfg(target_os = "linux")]
        {
            let ret = unsafe {
                raw_sys::syscall2(
                    raw_sys::SYS_MEMFD_CREATE,
                    name.as_ref().as_ptr() as u64,
                    flags.bits() as u64,
                )
            };
            let fd = raw_sys::decode(ret)? as RawFd;
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(crate::io::Errno::NOSYS)
        }
    }
}

pub mod shm {
    use super::*;
    pub use crate::fs::{Mode, OFlags};

    pub fn open(name: &str, flags: OFlags, mode: Mode) -> crate::io::Result<OwnedFd> {
        let trimmed = name.trim_start_matches('/');
        let path = format!("/tmp/{trimmed}");
        let mut opts = OpenOptions::new();
        opts.read(true)
            .write(flags.contains(OFlags::RDWR) || flags.contains(OFlags::WRONLY))
            .create(flags.contains(OFlags::CREATE))
            .create_new(flags.contains(OFlags::EXCL))
            .mode(mode.bits());
        let file = opts.open(&path).map_err(|e| {
            crate::io::Errno(e.raw_os_error().unwrap_or(crate::io::Errno::IO.0))
        })?;
        Ok(unsafe { OwnedFd::from_raw_fd(file.into_raw_fd()) })
    }

    pub fn unlink(name: &str) -> crate::io::Result<()> {
        let trimmed = name.trim_start_matches('/');
        let path = format!("/tmp/{trimmed}");
        std::fs::remove_file(&path).map_err(|e| {
            crate::io::Errno(e.raw_os_error().unwrap_or(crate::io::Errno::IO.0))
        })
    }
}
