//! Shared-memory (`wl_shm_pool` / `wl_buffer`) validation and mapping for `exo_compositor`.

use exo_syscall_abi as syscall;

pub const MAX_POOLS_PER_CLIENT: usize = 4;
pub const MAX_SHM_BYTES_PER_CLIENT: u64 = 16 * 1024 * 1024;

const F_ADD_SEALS: u64 = 1033;
const F_GET_SEALS: u64 = 1034;
const F_SEAL_SHRINK: u64 = 0x0002;

#[derive(Clone, Copy, Debug)]
pub struct ShmPool {
    pub fd: i32,
    pub size: u32,
    pub local_addr: u64,
    pub mapped_len: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct WlBuffer {
    pub pool_id: u32,
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bpp: u32,
}

#[inline]
pub fn validate_shm_buffer_bounds(
    offset: u32,
    width: u32,
    height: u32,
    stride: u32,
    bpp: u32,
    pool_limit: u64,
) -> bool {
    if width == 0 || height == 0 || width > 4096 || height > 4096 || bpp == 0 {
        return false;
    }
    let Some(min_stride) = width.checked_mul(bpp) else {
        return false;
    };
    if stride < min_stride {
        return false;
    }
    let Some(total_rows) = (height as u64).checked_mul(stride as u64) else {
        return false;
    };
    let Some(end_off) = (offset as u64).checked_add(total_rows) else {
        return false;
    };
    end_off <= pool_limit
}

pub fn format_bpp(fmt: u32) -> u32 {
    match fmt {
        4 | 0x36314752 => 2,
        5 | 0x34324752 | 6 | 0x34324742 => 3,
        _ => 4,
    }
}

pub fn unmap_close_pool(pool: ShmPool) {
    if pool.local_addr != 0 && pool.mapped_len != 0 {
        unsafe {
            let _ = syscall::syscall2(syscall::SYS_MUNMAP, pool.local_addr, pool.mapped_len);
        }
    }
    if pool.fd >= 0 {
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_CLOSE, pool.fd as u64);
        }
    }
}

pub fn map_verified_shm_fd(fd: i32, size: u32) -> Option<(u64, u64)> {
    if fd < 0 || size == 0 || (size as u64) > MAX_SHM_BYTES_PER_CLIENT {
        return None;
    }
    // Verify backing file size via `fstat` (st_size at offset 48 in 144-byte LinuxStat).
    let mut stat_buf = [0u8; 144];
    let st_rc = unsafe { syscall::syscall2(syscall::SYS_FSTAT, fd as u64, stat_buf.as_mut_ptr() as u64) };
    if st_rc < 0 {
        return None;
    }
    let st_size = i64::from_le_bytes([
        stat_buf[48],
        stat_buf[49],
        stat_buf[50],
        stat_buf[51],
        stat_buf[52],
        stat_buf[53],
        stat_buf[54],
        stat_buf[55],
    ]);
    if st_size <= 0 || (size as i64) > st_size {
        return None;
    }
    // Enforce `F_SEAL_SHRINK` on `memfd` SHM pools before mapping.
    let seals = unsafe { syscall::syscall3(syscall::SYS_FCNTL, fd as u64, F_GET_SEALS, 0) };
    if seals >= 0 && ((seals as u64) & F_SEAL_SHRINK) == 0 {
        let add_rc = unsafe {
            syscall::syscall3(syscall::SYS_FCNTL, fd as u64, F_ADD_SEALS, F_SEAL_SHRINK)
        };
        if add_rc < 0 {
            let seals2 = unsafe { syscall::syscall3(syscall::SYS_FCNTL, fd as u64, F_GET_SEALS, 0) };
            if seals2 < 0 || ((seals2 as u64) & F_SEAL_SHRINK) == 0 {
                return None;
            }
        }
    }
    let aligned = ((size as u64).max(4096) + 4095) & !4095;
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_MMAP,
            0,
            aligned,
            syscall::PROT_READ,
            syscall::MAP_SHARED,
            fd as u64,
            0,
        )
    };
    if rc > 0 {
        Some((rc as u64, aligned))
    } else {
        None
    }
}
