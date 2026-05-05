#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Errno(pub i64);

impl Errno {
    pub const EPERM: Self = Self(1);
    pub const ENOENT: Self = Self(2);
    pub const EIO: Self = Self(5);
    pub const EBADF: Self = Self(9);
    pub const EACCES: Self = Self(13);
    pub const EFAULT: Self = Self(14);
    pub const EEXIST: Self = Self(17);
    pub const ENOTDIR: Self = Self(20);
    pub const EISDIR: Self = Self(21);
    pub const EINVAL: Self = Self(22);
    pub const ENOSYS: Self = Self(38);

    pub const fn from_ret(ret: i64) -> Self {
        Self(-ret)
    }
}
