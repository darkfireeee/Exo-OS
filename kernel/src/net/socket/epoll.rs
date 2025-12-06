// kernel/src/net/socket/epoll.rs - epoll Implementation (Linux-compatible)
// High-performance I/O event notification mechanism

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::RwLock;

use super::Socket;

// ============================================================================
// epoll Event Types
// ============================================================================

pub const EPOLLIN: u32 = 0x001;      // Ready to read
pub const EPOLLOUT: u32 = 0x004;     // Ready to write
pub const EPOLLERR: u32 = 0x008;     // Error condition
pub const EPOLLHUP: u32 = 0x010;     // Hang up
pub const EPOLLPRI: u32 = 0x002;     // Urgent data
pub const EPOLLRDHUP: u32 = 0x2000;  // Peer closed connection
pub const EPOLLET: u32 = 0x80000000; // Edge-triggered
pub const EPOLLONESHOT: u32 = 0x40000000; // One-shot

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,    // Event mask
    pub data: u64,      // User data
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpollOp {
    Add = 1,    // EPOLL_CTL_ADD
    Del = 2,    // EPOLL_CTL_DEL
    Mod = 3,    // EPOLL_CTL_MOD
}

// ============================================================================
// Epoll Instance
// ============================================================================

pub struct Epoll {
    id: u32,
    registered_fds: RwLock<BTreeMap<u32, EpollEntry>>,
    ready_list: RwLock<Vec<EpollEvent>>,
    stats: EpollStats,
}

#[derive(Debug)]
struct EpollEntry {
    fd: u32,
    socket: Arc<Socket>,
    event: EpollEvent,
    edge_triggered: bool,
    one_shot: bool,
    last_events: u32,
}

#[derive(Debug, Default)]
struct EpollStats {
    add_count: AtomicU64,
    del_count: AtomicU64,
    mod_count: AtomicU64,
    wait_count: AtomicU64,
    events_delivered: AtomicU64,
}

impl Epoll {
    pub fn new() -> Arc<Self> {
        static EPOLL_ID_COUNTER: AtomicU32 = AtomicU32::new(1);
        
        Arc::new(Self {
            id: EPOLL_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            registered_fds: RwLock::new(BTreeMap::new()),
            ready_list: RwLock::new(Vec::new()),
            stats: EpollStats::default(),
        })
    }

    // ========================================================================
    // epoll_ctl - Control interface
    // ========================================================================
    
    pub fn ctl(&self, op: EpollOp, fd: u32, event: Option<EpollEvent>) -> Result<(), EpollError> {
        match op {
            EpollOp::Add => {
                let event = event.ok_or(EpollError::InvalidArgument)?;
                self.add_fd(fd, event)?;
                self.stats.add_count.fetch_add(1, Ordering::Relaxed);
            }
            EpollOp::Del => {
                self.del_fd(fd)?;
                self.stats.del_count.fetch_add(1, Ordering::Relaxed);
            }
            EpollOp::Mod => {
                let event = event.ok_or(EpollError::InvalidArgument)?;
                self.mod_fd(fd, event)?;
                self.stats.mod_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn add_fd(&self, fd: u32, event: EpollEvent) -> Result<(), EpollError> {
        let mut registered = self.registered_fds.write();
        
        if registered.contains_key(&fd) {
            return Err(EpollError::Exists);
        }

        // Récupérer le socket
        let socket = crate::net::socket::SOCKET_MANAGER.get(fd)
            .ok_or(EpollError::BadFileDescriptor)?;

        let entry = EpollEntry {
            fd,
            socket,
            event,
            edge_triggered: (event.events & EPOLLET) != 0,
            one_shot: (event.events & EPOLLONESHOT) != 0,
            last_events: 0,
        };

        registered.insert(fd, entry);
        log::debug!("[Epoll {}] Added fd {} with events 0x{:x}", self.id, fd, event.events);
        Ok(())
    }

    fn del_fd(&self, fd: u32) -> Result<(), EpollError> {
        let mut registered = self.registered_fds.write();
        registered.remove(&fd).ok_or(EpollError::NotFound)?;
        log::debug!("[Epoll {}] Deleted fd {}", self.id, fd);
        Ok(())
    }

    fn mod_fd(&self, fd: u32, event: EpollEvent) -> Result<(), EpollError> {
        let mut registered = self.registered_fds.write();
        let entry = registered.get_mut(&fd).ok_or(EpollError::NotFound)?;
        
        entry.event = event;
        entry.edge_triggered = (event.events & EPOLLET) != 0;
        entry.one_shot = (event.events & EPOLLONESHOT) != 0;
        
        log::debug!("[Epoll {}] Modified fd {} with events 0x{:x}", self.id, fd, event.events);
        Ok(())
    }

    // ========================================================================
    // epoll_wait - Wait for events
    // ========================================================================
    
    pub fn wait(&self, events: &mut [EpollEvent], timeout_ms: i32) -> Result<usize, EpollError> {
        self.stats.wait_count.fetch_add(1, Ordering::Relaxed);
        
        let start_time = crate::time::monotonic_time();
        let timeout_us = if timeout_ms < 0 {
            u64::MAX // Infinite
        } else {
            timeout_ms as u64 * 1000
        };

        loop {
            // Vérifier les FDs enregistrés et collecter les événements prêts
            let ready_events = self.collect_ready_events()?;
            
            if !ready_events.is_empty() {
                let to_return = ready_events.len().min(events.len());
                events[..to_return].copy_from_slice(&ready_events[..to_return]);
                
                self.stats.events_delivered.fetch_add(to_return as u64, Ordering::Relaxed);
                log::debug!("[Epoll {}] Returning {} events", self.id, to_return);
                return Ok(to_return);
            }

            // Timeout ?
            if timeout_ms == 0 {
                return Ok(0); // Non-blocking
            }
            
            let elapsed = crate::time::monotonic_time() - start_time;
            if elapsed >= timeout_us {
                return Ok(0); // Timeout
            }

            // TODO: Sleep ou wait queue au lieu de spin
            core::hint::spin_loop();
        }
    }

    fn collect_ready_events(&self) -> Result<Vec<EpollEvent>, EpollError> {
        let mut ready_events = Vec::new();
        let registered = self.registered_fds.read();

        for entry in registered.values() {
            let current_events = self.get_socket_events(&entry.socket);
            let interested_events = entry.event.events & (EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLHUP);
            let ready = current_events & interested_events;

            if ready != 0 {
                // Edge-triggered: seulement si les événements ont changé
                if entry.edge_triggered && ready == entry.last_events {
                    continue;
                }

                let mut event = entry.event;
                event.events = ready;
                ready_events.push(event);

                // One-shot: retirer automatiquement
                if entry.one_shot {
                    drop(registered);
                    self.del_fd(entry.fd)?;
                    return Ok(ready_events);
                }
            }
        }

        Ok(ready_events)
    }

    fn get_socket_events(&self, socket: &Socket) -> u32 {
        let mut events = 0;

        // EPOLLIN: données disponibles à lire
        if !socket.recv_buffer.read().is_empty() {
            events |= EPOLLIN;
        }

        // EPOLLOUT: espace disponible pour écrire
        let opts = socket.options.read();
        let current_buffered: usize = socket.send_buffer.read().iter().map(|b| b.len()).sum();
        if current_buffered < opts.send_buffer {
            events |= EPOLLOUT;
        }

        // EPOLLERR: erreur sur le socket
        // TODO: Vérifier les erreurs réelles
        
        // EPOLLHUP: connexion fermée
        use super::SocketState;
        if *socket.state.read() == SocketState::Closed {
            events |= EPOLLHUP;
        }

        events
    }

    // ========================================================================
    // Statistics
    // ========================================================================
    
    pub fn stats(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.stats.add_count.load(Ordering::Relaxed),
            self.stats.del_count.load(Ordering::Relaxed),
            self.stats.mod_count.load(Ordering::Relaxed),
            self.stats.wait_count.load(Ordering::Relaxed),
            self.stats.events_delivered.load(Ordering::Relaxed),
        )
    }
}

// ============================================================================
// Epoll Manager
// ============================================================================

pub struct EpollManager {
    instances: RwLock<BTreeMap<u32, Arc<Epoll>>>,
}

impl EpollManager {
    pub const fn new() -> Self {
        Self {
            instances: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn create(&self) -> u32 {
        let epoll = Epoll::new();
        let epoll_id = epoll.id;
        self.instances.write().insert(epoll_id, epoll);
        epoll_id
    }

    pub fn get(&self, epoll_id: u32) -> Option<Arc<Epoll>> {
        self.instances.read().get(&epoll_id).cloned()
    }

    pub fn close(&self, epoll_id: u32) {
        self.instances.write().remove(&epoll_id);
    }
}

pub static EPOLL_MANAGER: EpollManager = EpollManager::new();

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpollError {
    InvalidArgument,
    BadFileDescriptor,
    Exists,
    NotFound,
    NoMemory,
}

// ============================================================================
// Syscall Wrappers
// ============================================================================

/// epoll_create1() - Crée une instance epoll
pub fn sys_epoll_create1(_flags: i32) -> Result<u32, EpollError> {
    Ok(EPOLL_MANAGER.create())
}

/// epoll_ctl() - Control epoll instance
pub fn sys_epoll_ctl(epfd: u32, op: i32, fd: u32, event: Option<EpollEvent>) -> Result<(), EpollError> {
    let epoll = EPOLL_MANAGER.get(epfd).ok_or(EpollError::BadFileDescriptor)?;
    
    let op = match op {
        1 => EpollOp::Add,
        2 => EpollOp::Del,
        3 => EpollOp::Mod,
        _ => return Err(EpollError::InvalidArgument),
    };

    epoll.ctl(op, fd, event)
}

/// epoll_wait() - Wait for events
pub fn sys_epoll_wait(epfd: u32, events: &mut [EpollEvent], timeout_ms: i32) -> Result<usize, EpollError> {
    let epoll = EPOLL_MANAGER.get(epfd).ok_or(EpollError::BadFileDescriptor)?;
    epoll.wait(events, timeout_ms)
}

/// epoll_close() - Close epoll instance
pub fn sys_epoll_close(epfd: u32) {
    EPOLL_MANAGER.close(epfd);
}
