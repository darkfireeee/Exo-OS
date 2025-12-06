/// TCP Fast Open (TFO) - RFC 7413
/// 
/// Allows data to be sent during the TCP handshake, reducing latency
/// for subsequent connections by 1 RTT.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::net::tcp::TcpConnection;
use core::time::Duration;

/// TCP Fast Open cookie size (128 bits)
pub const TFO_COOKIE_SIZE: usize = 16;

/// TCP Fast Open cookie
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfoCookie {
    /// Cookie data (128 bits)
    data: [u8; TFO_COOKIE_SIZE],
}

impl TfoCookie {
    /// Create a new TFO cookie
    pub fn new(data: [u8; TFO_COOKIE_SIZE]) -> Self {
        Self { data }
    }

    /// Get the cookie data
    pub fn data(&self) -> &[u8; TFO_COOKIE_SIZE] {
        &self.data
    }

    /// Generate a cookie for a client address
    /// 
    /// In production, this would use a secret key and HMAC
    /// For now, we use a simple hash of the address
    pub fn generate(client_addr: &[u8; 16], secret_key: &[u8; 32]) -> Self {
        let mut cookie = [0u8; TFO_COOKIE_SIZE];
        
        // Simple hash: XOR address with key
        for i in 0..TFO_COOKIE_SIZE {
            cookie[i] = client_addr[i % 16] ^ secret_key[i] ^ secret_key[i + 16];
        }

        Self { data: cookie }
    }

    /// Verify a cookie against a client address
    pub fn verify(&self, client_addr: &[u8; 16], secret_key: &[u8; 32]) -> bool {
        let expected = Self::generate(client_addr, secret_key);
        self.data == expected.data
    }
}

/// TCP Fast Open Manager
/// 
/// Manages TFO cookies for both client and server sides
pub struct TfoManager {
    /// Secret key for cookie generation (server-side)
    secret_key: [u8; 32],
    /// Client-side cookie cache: remote_addr -> cookie
    client_cookies: Mutex<BTreeMap<[u8; 16], TfoCookie>>,
    /// Server-side: enable/disable TFO
    server_enabled: bool,
    /// Client-side: enable/disable TFO
    client_enabled: bool,
    /// Maximum number of cached cookies
    max_cached_cookies: usize,
}

impl TfoManager {
    /// Create a new TFO manager
    pub fn new() -> Self {
        Self {
            secret_key: Self::generate_secret_key(),
            client_cookies: Mutex::new(BTreeMap::new()),
            server_enabled: true,
            client_enabled: true,
            max_cached_cookies: 1000,
        }
    }

    /// Generate a random secret key
    /// 
    /// In production, this would use a cryptographically secure RNG
    fn generate_secret_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        // TODO: Use proper RNG
        for i in 0..32 {
            key[i] = (i as u8).wrapping_mul(7).wrapping_add(13);
        }
        key
    }

    /// Enable/disable server-side TFO
    pub fn set_server_enabled(&mut self, enabled: bool) {
        self.server_enabled = enabled;
    }

    /// Enable/disable client-side TFO
    pub fn set_client_enabled(&mut self, enabled: bool) {
        self.client_enabled = enabled;
    }

    /// Check if server-side TFO is enabled
    pub fn is_server_enabled(&self) -> bool {
        self.server_enabled
    }

    /// Check if client-side TFO is enabled
    pub fn is_client_enabled(&self) -> bool {
        self.client_enabled
    }

    /// Generate a TFO cookie for a client (server-side)
    pub fn generate_cookie(&self, client_addr: &[u8; 16]) -> TfoCookie {
        TfoCookie::generate(client_addr, &self.secret_key)
    }

    /// Verify a TFO cookie from a client (server-side)
    pub fn verify_cookie(&self, cookie: &TfoCookie, client_addr: &[u8; 16]) -> bool {
        if !self.server_enabled {
            return false;
        }
        cookie.verify(client_addr, &self.secret_key)
    }

    /// Cache a TFO cookie received from server (client-side)
    pub fn cache_cookie(&self, server_addr: [u8; 16], cookie: TfoCookie) {
        if !self.client_enabled {
            return;
        }

        let mut cache = self.client_cookies.lock();
        
        // Evict oldest if cache is full
        if cache.len() >= self.max_cached_cookies {
            if let Some(key) = cache.keys().next().copied() {
                cache.remove(&key);
            }
        }

        cache.insert(server_addr, cookie);
    }

    /// Get a cached TFO cookie for a server (client-side)
    pub fn get_cached_cookie(&self, server_addr: &[u8; 16]) -> Option<TfoCookie> {
        if !self.client_enabled {
            return None;
        }

        let cache = self.client_cookies.lock();
        cache.get(server_addr).copied()
    }

    /// Remove a cached cookie (e.g., if it was rejected)
    pub fn remove_cached_cookie(&self, server_addr: &[u8; 16]) {
        let mut cache = self.client_cookies.lock();
        cache.remove(server_addr);
    }

    /// Clear all cached cookies
    pub fn clear_cache(&self) {
        let mut cache = self.client_cookies.lock();
        cache.clear();
    }

    /// Get the number of cached cookies
    pub fn cached_count(&self) -> usize {
        self.client_cookies.lock().len()
    }
}

/// Global TFO manager instance
static TFO_MANAGER: Mutex<Option<TfoManager>> = Mutex::new(None);

/// Initialize the global TFO manager
pub fn init() {
    *TFO_MANAGER.lock() = Some(TfoManager::new());
}

/// Get a reference to the global TFO manager
pub fn get_manager() -> Option<TfoManager> {
    TFO_MANAGER.lock().as_ref().map(|m| {
        // Clone the manager (this is not ideal, but works for now)
        TfoManager {
            secret_key: m.secret_key,
            client_cookies: Mutex::new(m.client_cookies.lock().clone()),
            server_enabled: m.server_enabled,
            client_enabled: m.client_enabled,
            max_cached_cookies: m.max_cached_cookies,
        }
    })
}

/// TCP Fast Open statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct TfoStats {
    /// Number of TFO SYNs sent (client)
    pub syns_sent: u64,
    /// Number of TFO SYNs received (server)
    pub syns_received: u64,
    /// Number of TFO SYNs accepted (server)
    pub syns_accepted: u64,
    /// Number of TFO SYNs rejected (server)
    pub syns_rejected: u64,
    /// Number of TFO cookies requested (client)
    pub cookies_requested: u64,
    /// Number of TFO cookies received (client)
    pub cookies_received: u64,
    /// Number of data bytes sent in SYN (client)
    pub data_bytes_sent: u64,
    /// Number of data bytes received in SYN (server)
    pub data_bytes_received: u64,
}

impl TfoStats {
    /// Create new TFO statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all statistics
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Global TFO statistics
static TFO_STATS: Mutex<TfoStats> = Mutex::new(TfoStats {
    syns_sent: 0,
    syns_received: 0,
    syns_accepted: 0,
    syns_rejected: 0,
    cookies_requested: 0,
    cookies_received: 0,
    data_bytes_sent: 0,
    data_bytes_received: 0,
});

/// Get the current TFO statistics
pub fn get_stats() -> TfoStats {
    *TFO_STATS.lock()
}

/// Reset TFO statistics
pub fn reset_stats() {
    TFO_STATS.lock().reset();
}
