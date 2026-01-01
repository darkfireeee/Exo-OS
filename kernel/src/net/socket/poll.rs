// kernel/src/net/socket/poll.rs - poll/select Implementation
// Traditional I/O multiplexing for compatibility

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::Socket;

// ============================================================================
// poll() Structures
// ============================================================================

pub const POLLIN: i16 = 0x0001;      // Ready to read
pub const POLLOUT: i16 = 0x0004;     // Ready to write
pub const POLLERR: i16 = 0x0008;     // Error condition
pub const POLLHUP: i16 = 0x0010;     // Hang up
pub const POLLNVAL: i16 = 0x0020;    // Invalid request
pub const POLLPRI: i16 = 0x0002;     // Urgent data
pub const POLLRDHUP: i16 = 0x2000;   // Peer closed

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PollFd {
    pub fd: i32,        // File descriptor
    pub events: i16,    // Requested events
    pub revents: i16,   // Returned events
}

// ============================================================================
// select() Structures
// ============================================================================

pub const FD_SETSIZE: usize = 1024;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FdSet {
    bits: [u64; FD_SETSIZE / 64],
}

impl FdSet {
    pub const fn new() -> Self {
        Self { bits: [0; FD_SETSIZE / 64] }
    }

    pub fn clear(&mut self) {
        self.bits = [0; FD_SETSIZE / 64];
    }

    pub fn set(&mut self, fd: usize) {
        if fd < FD_SETSIZE {
            let idx = fd / 64;
            let bit = fd % 64;
            self.bits[idx] |= 1u64 << bit;
        }
    }

    pub fn clear_fd(&mut self, fd: usize) {
        if fd < FD_SETSIZE {
            let idx = fd / 64;
            let bit = fd % 64;
            self.bits[idx] &= !(1u64 << bit);
        }
    }

    pub fn is_set(&self, fd: usize) -> bool {
        if fd < FD_SETSIZE {
            let idx = fd / 64;
            let bit = fd % 64;
            (self.bits[idx] & (1u64 << bit)) != 0
        } else {
            false
        }
    }

    pub fn count(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TimeVal {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

// ============================================================================
// poll() Implementation
// ============================================================================

static POLL_STATS: PollStats = PollStats::new();

#[derive(Debug)]
struct PollStats {
    poll_calls: AtomicU64,
    select_calls: AtomicU64,
    total_fds_checked: AtomicU64,
    events_returned: AtomicU64,
}

impl PollStats {
    const fn new() -> Self {
        Self {
            poll_calls: AtomicU64::new(0),
            select_calls: AtomicU64::new(0),
            total_fds_checked: AtomicU64::new(0),
            events_returned: AtomicU64::new(0),
        }
    }
}

pub fn sys_poll(fds: &mut [PollFd], timeout_ms: i32) -> Result<usize, PollError> {
    POLL_STATS.poll_calls.fetch_add(1, Ordering::Relaxed);
    POLL_STATS.total_fds_checked.fetch_add(fds.len() as u64, Ordering::Relaxed);

    let start_time = crate::time::monotonic_time();
    let timeout_us = if timeout_ms < 0 {
        u64::MAX // Infinite
    } else {
        timeout_ms as u64 * 1000
    };

    loop {
        let mut ready_count = 0;

        // Vérifier chaque FD
        for pollfd in fds.iter_mut() {
            pollfd.revents = 0;

            if pollfd.fd < 0 {
                continue; // Ignore negative FDs
            }

            // Récupérer le socket
            let socket = match crate::net::socket::SOCKET_MANAGER.get(pollfd.fd as u32) {
                Some(s) => s,
                None => {
                    pollfd.revents = POLLNVAL;
                    ready_count += 1;
                    continue;
                }
            };

            // Vérifier les événements demandés
            let events = get_socket_poll_events(&socket);
            pollfd.revents = events & pollfd.events;

            // Toujours retourner POLLERR/POLLHUP même si non demandés
            pollfd.revents |= events & (POLLERR | POLLHUP | POLLNVAL);

            if pollfd.revents != 0 {
                ready_count += 1;
            }
        }

        if ready_count > 0 {
            POLL_STATS.events_returned.fetch_add(ready_count as u64, Ordering::Relaxed);
            return Ok(ready_count);
        }

        // Timeout ?
        if timeout_ms == 0 {
            return Ok(0); // Non-blocking
        }

        let elapsed = crate::time::monotonic_time() - start_time;
        if elapsed >= timeout_us {
            return Ok(0); // Timeout
        }

        // Sleep 1ms to avoid busy waiting (Phase 2c optimization)
        let sleep_duration = crate::syscall::handlers::time::TimeSpec::new(0, 1_000_000);
        let _ = crate::syscall::handlers::time::sys_nanosleep(sleep_duration);
    }
}

fn get_socket_poll_events(socket: &Socket) -> i16 {
    let mut events = 0i16;

    // POLLIN: données disponibles
    if !socket.recv_buffer.read().is_empty() {
        events |= POLLIN;
    }

    // POLLOUT: espace pour écrire
    let opts = socket.options.read();
    let current_buffered: usize = socket.send_buffer.read().iter().map(|b| b.len()).sum();
    if current_buffered < opts.send_buffer {
        events |= POLLOUT;
    }

    // POLLHUP: connexion fermée
    use super::SocketState;
    if *socket.state.read() == SocketState::Closed {
        events |= POLLHUP;
    }

    events
}

// ============================================================================
// select() Implementation
// ============================================================================

pub fn sys_select(
    nfds: usize,
    readfds: Option<&mut FdSet>,
    writefds: Option<&mut FdSet>,
    exceptfds: Option<&mut FdSet>,
    timeout: Option<TimeVal>,
) -> Result<usize, PollError> {
    POLL_STATS.select_calls.fetch_add(1, Ordering::Relaxed);

    let timeout_us = match timeout {
        Some(tv) => (tv.tv_sec as u64 * 1_000_000) + tv.tv_usec as u64,
        None => u64::MAX, // Infinite
    };

    let start_time = crate::time::monotonic_time();

    // Créer des copies pour ne pas modifier les originaux avant de savoir ce qui est prêt
    let mut read_result = readfds.map(|fds| *fds).unwrap_or(FdSet::new());
    let mut write_result = writefds.map(|fds| *fds).unwrap_or(FdSet::new());
    let mut except_result = exceptfds.map(|fds| *fds).unwrap_or(FdSet::new());

    loop {
        let mut ready_count = 0;

        // Vérifier chaque FD jusqu'à nfds
        for fd in 0..nfds {
            let want_read = readfds.map(|fds| fds.is_set(fd)).unwrap_or(false);
            let want_write = writefds.map(|fds| fds.is_set(fd)).unwrap_or(false);
            let want_except = exceptfds.map(|fds| fds.is_set(fd)).unwrap_or(false);

            if !want_read && !want_write && !want_except {
                continue;
            }

            // Récupérer le socket
            let socket = match crate::net::socket::SOCKET_MANAGER.get(fd as u32) {
                Some(s) => s,
                None => {
                    // FD invalide: le marquer dans except
                    if want_except {
                        except_result.set(fd);
                        ready_count += 1;
                    }
                    continue;
                }
            };

            let events = get_socket_poll_events(&socket);

            // Vérifier read
            if want_read && (events & POLLIN) != 0 {
                read_result.set(fd);
                ready_count += 1;
            } else if want_read {
                read_result.clear_fd(fd);
            }

            // Vérifier write
            if want_write && (events & POLLOUT) != 0 {
                write_result.set(fd);
                ready_count += 1;
            } else if want_write {
                write_result.clear_fd(fd);
            }

            // Vérifier except (errors/hangup)
            if want_except && (events & (POLLERR | POLLHUP)) != 0 {
                except_result.set(fd);
                ready_count += 1;
            } else if want_except {
                except_result.clear_fd(fd);
            }
        }

        if ready_count > 0 {
            // Copier les résultats dans les fd_sets originaux
            if let Some(fds) = readfds {
                *fds = read_result;
            }
            if let Some(fds) = writefds {
                *fds = write_result;
            }
            if let Some(fds) = exceptfds {
                *fds = except_result;
            }

            POLL_STATS.events_returned.fetch_add(ready_count as u64, Ordering::Relaxed);
            return Ok(ready_count);
        }

        // Timeout ?
        if timeout_us == 0 {
            return Ok(0); // Non-blocking
        }

        let elapsed = crate::time::monotonic_time() - start_time;
        if elapsed >= timeout_us {
            return Ok(0); // Timeout
        }

        // Sleep 1ms to avoid busy waiting (Phase 2c optimization)
        let sleep_duration = crate::syscall::handlers::time::TimeSpec::new(0, 1_000_000);
        let _ = crate::syscall::handlers::time::sys_nanosleep(sleep_duration);
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

pub fn fd_zero(fds: &mut FdSet) {
    fds.clear();
}

pub fn fd_set(fd: i32, fds: &mut FdSet) {
    if fd >= 0 {
        fds.set(fd as usize);
    }
}

pub fn fd_clr(fd: i32, fds: &mut FdSet) {
    if fd >= 0 {
        fds.clear_fd(fd as usize);
    }
}

pub fn fd_isset(fd: i32, fds: &FdSet) -> bool {
    if fd >= 0 {
        fds.is_set(fd as usize)
    } else {
        false
    }
}

// ============================================================================
// Statistics
// ============================================================================

pub fn get_poll_stats() -> (u64, u64, u64, u64) {
    (
        POLL_STATS.poll_calls.load(Ordering::Relaxed),
        POLL_STATS.select_calls.load(Ordering::Relaxed),
        POLL_STATS.total_fds_checked.load(Ordering::Relaxed),
        POLL_STATS.events_returned.load(Ordering::Relaxed),
    )
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollError {
    InvalidArgument,
    BadFileDescriptor,
    NoMemory,
}
