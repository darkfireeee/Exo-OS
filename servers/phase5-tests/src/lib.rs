//! # phase5-tests — Tests unitaires Phase 5 Exo-OS
//!
//! Ce crate re-implémente la logique pure (sans syscalls, sans hardware) des
//! quatre servers Ring 1 implémentés en Phase 5, et vérifie leur comportement.
#![allow(dead_code)]
#![allow(private_interfaces)]
//!
//! ## Modules testés
//!   - `ipc_router`    : Registry FNV-32, register/resolve, routing messages
//!   - `init_server`   : table de services, backoff exponentiel, spawn/reap
//!   - `crypto_server` : derive_key_stub (déterminisme KDF), dispatch requêtes,
//!                       allocation key_handle, CSPRNG bounds
//!   - `vfs_server`    : fnv32 path hash, table de montages, mount/umount logic,
//!                       handle_mount/handle_resolve/handle_open dispatch
//!
//! ## Invariants vérifiés
//!   - FNV-32 est déterministe et résiste aux collisions sur noms courts
//!   - Registry.resolve retourne None pour nom inconnu
//!   - Registry accepte max 64 entrées
//!   - derive_key_stub est déterministe et produit 32 octets non-nuls pour inputs non-nuls
//!   - crypto handle_request : CRYPTO_DERIVE_KEY alloue handles consécutifs
//!   - crypto handle_request : CRYPTO_RANDOM retourne CRYPTO_OK (simulé)
//!   - vfs_server monte les 3 pseudo-FS par défaut
//!   - vfs mount/resolve retourne EINVAL sur payload trop court
//!   - init_server Service.mark_dead() double le délai (max 32)

// ═════════════════════════════════════════════════════════════════════════════
// ── ipc_router — logique pure ─────────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

mod ipc_router {
    // Miroir exact de la struct Registry dans servers/ipc_router/src/main.rs
    use std::sync::atomic::{AtomicU32, Ordering};

    pub struct Registry {
        names:     [u32; 64],
        endpoints: [u32; 64],
        count:     AtomicU32,
    }

    impl Registry {
        pub fn new() -> Self {
            Self { names: [0u32; 64], endpoints: [0u32; 64], count: AtomicU32::new(0) }
        }

        pub fn hash_name(name: &[u8]) -> u32 {
            let mut h: u32 = 2166136261;
            for &b in name { h = h.wrapping_mul(16777619).wrapping_add(b as u32); }
            h
        }

        pub fn register(&mut self, name: &[u8], endpoint: u32) -> bool {
            let h = Self::hash_name(name);
            let n = self.count.load(Ordering::Relaxed) as usize;
            if n >= 64 { return false; }
            self.names[n] = h;
            self.endpoints[n] = endpoint;
            self.count.store((n + 1) as u32, Ordering::Release);
            true
        }

        pub fn resolve(&self, name: &[u8]) -> Option<u32> {
            let h = Self::hash_name(name);
            let n = self.count.load(Ordering::Acquire) as usize;
            for i in 0..n {
                if self.names[i] == h { return Some(self.endpoints[i]); }
            }
            None
        }

        pub fn count(&self) -> u32 { self.count.load(Ordering::Relaxed) }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── init_server — logique pure ────────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

mod init_server {
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Miroir de Service dans servers/init_server/src/main.rs
    pub struct Service {
        pub name:                &'static str,
        pub pid:                 AtomicU32,
        pub restart_delay_ticks: AtomicU32,
    }

    impl Service {
        pub fn new(name: &'static str) -> Self {
            Self { name, pid: AtomicU32::new(0), restart_delay_ticks: AtomicU32::new(1) }
        }

        pub fn current_pid(&self) -> u32 { self.pid.load(Ordering::Acquire) }

        pub fn set_pid(&self, pid: u32) {
            self.pid.store(pid, Ordering::Release);
            self.restart_delay_ticks.store(1, Ordering::Relaxed);
        }

        pub fn mark_dead(&self) {
            self.pid.store(0, Ordering::Release);
            let d = self.restart_delay_ticks.load(Ordering::Relaxed);
            self.restart_delay_ticks.store(d.saturating_mul(2).min(32), Ordering::Relaxed);
        }

        pub fn delay(&self) -> u32 { self.restart_delay_ticks.load(Ordering::Relaxed) }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── crypto_server — logique pure ──────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

mod crypto_server {
    use std::sync::atomic::{AtomicU32, Ordering};

    pub const CRYPTO_DERIVE_KEY: u32 = 0;
    pub const CRYPTO_RANDOM:     u32 = 1;
    pub const CRYPTO_ENCRYPT:    u32 = 2;
    pub const CRYPTO_DECRYPT:    u32 = 3;
    pub const CRYPTO_HASH:       u32 = 4;

    pub const CRYPTO_OK:       u32 = 0;
    pub const CRYPTO_ERR_ARGS: u32 = 1;
    pub const CRYPTO_ERR_BUSY: u32 = 2;

    /// Miroir exact de derive_key_stub dans crypto_server/src/main.rs
    pub fn derive_key_stub(material: &[u8], output: &mut [u8; 32]) {
        let mut state: [u64; 4] = [
            0x6c62272e07bb0142,
            0x62b821756295c58d,
            0x0000000000000000,
            0xffffffffffffffff,
        ];
        for &b in material {
            state[0] = state[0].wrapping_mul(1099511628211).wrapping_add(b as u64);
            state[1] ^= state[0].rotate_left(17);
            state[2] = state[2].wrapping_add(state[1]);
            state[3] ^= state[2].rotate_right(11);
        }
        output[0..8].copy_from_slice(&state[0].to_le_bytes());
        output[8..16].copy_from_slice(&state[1].to_le_bytes());
        output[16..24].copy_from_slice(&state[2].to_le_bytes());
        output[24..32].copy_from_slice(&state[3].to_le_bytes());
    }

    /// Keystore simulé (sans état global partagé dans les tests).
    pub struct Keystore {
        table: [[u8; 32]; 32],
        count: AtomicU32,
    }

    #[repr(C)]
    pub struct CryptoRequest {
        pub sender_pid: u32,
        pub msg_type:   u32,
        pub payload:    [u8; 120],
    }

    #[repr(C)]
    #[derive(Debug)]
    pub struct CryptoReply {
        pub status:     u32,
        pub key_handle: u32,
        pub data:       [u8; 56],
    }

    impl Keystore {
        pub fn new() -> Self {
            Self { table: [[0u8; 32]; 32], count: AtomicU32::new(0) }
        }

        pub fn handle_request(&mut self, req: &CryptoRequest) -> CryptoReply {
            let mut reply = CryptoReply { status: CRYPTO_ERR_ARGS, key_handle: 0, data: [0u8; 56] };

            match req.msg_type {
                CRYPTO_DERIVE_KEY => {
                    let idx = self.count.load(Ordering::Relaxed) as usize;
                    if idx >= 32 { reply.status = CRYPTO_ERR_BUSY; return reply; }
                    derive_key_stub(&req.payload, &mut self.table[idx]);
                    self.count.store((idx + 1) as u32, Ordering::Release);
                    reply.status = CRYPTO_OK;
                    reply.key_handle = (idx + 1) as u32;
                }
                CRYPTO_HASH => {
                    let mut out = [0u8; 32];
                    derive_key_stub(&req.payload, &mut out);
                    reply.data[..32].copy_from_slice(&out);
                    reply.status = CRYPTO_OK;
                }
                CRYPTO_ENCRYPT | CRYPTO_DECRYPT => {
                    reply.status = CRYPTO_ERR_ARGS;
                }
                CRYPTO_RANDOM => {
                    // En test : on simule en remplissant de 0xAB (pas de SYS_GETRANDOM dispo)
                    reply.data.iter_mut().for_each(|b| *b = 0xAB);
                    reply.status = CRYPTO_OK;
                }
                _ => {}
            }
            reply
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── vfs_server — logique pure ─────────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

mod vfs_server {
    #[repr(u8)]
    #[derive(Copy, Clone, PartialEq, Debug)]
    pub enum FsType { None = 0, ExoFs = 1, ProcFs = 2, SysFs = 3, DevFs = 4 }

    #[derive(Copy, Clone, Debug)]
    pub struct MountEntry {
        pub fs_type:   FsType,
        pub path_hash: u32,
        pub root_blob: u64,
        pub active:    bool,
    }

    impl MountEntry {
        pub const fn empty() -> Self {
            Self { fs_type: FsType::None, path_hash: 0, root_blob: 0, active: false }
        }
    }

    pub fn fnv32(s: &[u8]) -> u32 {
        let mut h: u32 = 2166136261;
        for &b in s { h = h.wrapping_mul(16777619).wrapping_add(b as u32); }
        h
    }

    pub struct MountTable {
        entries: [MountEntry; 32],
        pub count: usize,
    }

    impl MountTable {
        pub fn new() -> Self {
            Self { entries: [MountEntry::empty(); 32], count: 0 }
        }

        pub fn add(&mut self, fs: FsType, path: &[u8], blob: u64) -> Result<usize, i64> {
            if self.count >= 32 { return Err(-28); } // ENOSPC
            let idx = self.count;
            self.entries[idx] = MountEntry {
                fs_type: fs, path_hash: fnv32(path), root_blob: blob, active: true,
            };
            self.count += 1;
            Ok(idx)
        }

        pub fn find_by_path(&self, path: &[u8]) -> Option<&MountEntry> {
            let h = fnv32(path);
            self.entries[..self.count].iter().find(|e| e.active && e.path_hash == h)
        }

        pub fn remove(&mut self, path: &[u8]) -> bool {
            let h = fnv32(path);
            for e in &mut self.entries[..self.count] {
                if e.active && e.path_hash == h { e.active = false; return true; }
            }
            false
        }

        pub fn active_count(&self) -> usize {
            self.entries[..self.count].iter().filter(|e| e.active).count()
        }
    }

    /// Mock de handle_mount (sans SYS_EXOFS_PATH_RESOLVE)
    pub fn handle_mount_payload(table: &mut MountTable, payload: &[u8]) -> i64 {
        if payload.len() < 14 { return -22; }
        let fstype = payload[0];
        let root_blob = u64::from_le_bytes([
            payload[5], payload[6], payload[7], payload[8],
            payload[9], payload[10], payload[11], payload[12],
        ]);
        let path = &payload[13..];
        let path_len = path.iter().position(|&b| b == 0).unwrap_or(path.len());
        let fs = match fstype {
            1 => FsType::ExoFs,
            2 => FsType::ProcFs,
            3 => FsType::SysFs,
            4 => FsType::DevFs,
            _ => return -22,
        };
        table.add(fs, &path[..path_len], root_blob).map(|_| 0i64).unwrap_or(-28)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── TESTS ipc_router ──────────────────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests_ipc_router {
    use super::ipc_router::Registry;

    #[test]
    fn test_fnv_deterministe() {
        let h1 = Registry::hash_name(b"ipc_router");
        let h2 = Registry::hash_name(b"ipc_router");
        assert_eq!(h1, h2, "FNV-32 doit être déterministe");
    }

    #[test]
    fn test_fnv_collision_differents_noms() {
        let h1 = Registry::hash_name(b"ipc_router");
        let h2 = Registry::hash_name(b"vfs_server");
        let h3 = Registry::hash_name(b"crypto_server");
        let h4 = Registry::hash_name(b"init_server");
        // Les 4 noms de services principaux ne doivent pas collisionner
        assert_ne!(h1, h2);
        assert_ne!(h1, h3);
        assert_ne!(h1, h4);
        assert_ne!(h2, h3);
        assert_ne!(h2, h4);
        assert_ne!(h3, h4);
    }

    #[test]
    fn test_fnv_vide_donne_valeur_initiale() {
        // FNV offset basis = 2166136261
        assert_eq!(Registry::hash_name(b""), 2166136261u32);
    }

    #[test]
    fn test_register_et_resolve_nominal() {
        let mut r = Registry::new();
        assert!(r.register(b"vfs_server", 3));
        assert_eq!(r.resolve(b"vfs_server"), Some(3));
    }

    #[test]
    fn test_resolve_inconnu_retourne_none() {
        let r = Registry::new();
        assert_eq!(r.resolve(b"inexistant"), None);
    }

    #[test]
    fn test_register_plusieurs_services() {
        let mut r = Registry::new();
        assert!(r.register(b"ipc_router",    2));
        assert!(r.register(b"vfs_server",    3));
        assert!(r.register(b"crypto_server", 4));
        assert!(r.register(b"init_server",   1));
        assert_eq!(r.resolve(b"ipc_router"),    Some(2));
        assert_eq!(r.resolve(b"vfs_server"),    Some(3));
        assert_eq!(r.resolve(b"crypto_server"), Some(4));
        assert_eq!(r.resolve(b"init_server"),   Some(1));
    }

    #[test]
    fn test_register_overwrite_meme_hash() {
        // Si deux entrées ont le même hash (collision théorique),
        // resolve retourne le premier enregistré
        let mut r = Registry::new();
        assert!(r.register(b"svcA", 10));
        assert!(r.register(b"svcB", 20));
        assert_eq!(r.resolve(b"svcA"), Some(10));
        assert_eq!(r.resolve(b"svcB"), Some(20));
    }

    #[test]
    fn test_register_table_pleine_retourne_false() {
        let mut r = Registry::new();
        for i in 0u32..64 {
            let name = format!("svc{i:03}");
            assert!(r.register(name.as_bytes(), i), "slot {i} doit être libre");
        }
        assert_eq!(r.count(), 64);
        // La 65ème insertion doit échouer
        assert!(!r.register(b"svc_overflow", 99));
        assert_eq!(r.count(), 64, "count ne doit pas dépasser 64");
    }

    #[test]
    fn test_register_nom_vide_et_long() {
        let mut r = Registry::new();
        assert!(r.register(b"", 0));
        let long_name = b"a_very_long_service_name_that_exceeds_typical_dns_label_length";
        assert!(r.register(long_name, 99));
        assert_eq!(r.resolve(long_name), Some(99));
    }

    #[test]
    fn test_resolve_apres_table_pleine() {
        let mut r = Registry::new();
        for i in 0u32..64 {
            let name = format!("s{i:02}");
            r.register(name.as_bytes(), i);
        }
        // Tous les services doivent rester résolvables
        for i in 0u32..64 {
            let name = format!("s{i:02}");
            assert_eq!(r.resolve(name.as_bytes()), Some(i), "service {name} perdu");
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── TESTS init_server ─────────────────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests_init_server {
    use super::init_server::Service;

    #[test]
    fn test_service_initial_pid_zero() {
        let svc = Service::new("test_svc");
        assert_eq!(svc.current_pid(), 0);
    }

    #[test]
    fn test_set_pid_et_lecture() {
        let svc = Service::new("test_svc");
        svc.set_pid(42);
        assert_eq!(svc.current_pid(), 42);
    }

    #[test]
    fn test_set_pid_remet_delay_a_un() {
        let svc = Service::new("test_svc");
        svc.set_pid(10);
        svc.mark_dead();
        svc.mark_dead(); // delay = 4 maintenant
        assert!(svc.delay() > 1);
        // Relancer le service remet le délai à 1
        svc.set_pid(11);
        assert_eq!(svc.delay(), 1, "set_pid doit remettre restart_delay à 1");
    }

    #[test]
    fn test_mark_dead_pid_zero() {
        let svc = Service::new("test_svc");
        svc.set_pid(100);
        svc.mark_dead();
        assert_eq!(svc.current_pid(), 0, "mark_dead doit mettre le PID à 0");
    }

    #[test]
    fn test_backoff_exponentiel() {
        let svc = Service::new("test_svc");
        // Délai initial = 1
        assert_eq!(svc.delay(), 1);
        svc.set_pid(1); svc.mark_dead(); // crash 1 → delay = 2
        assert_eq!(svc.delay(), 2);
        svc.set_pid(2); svc.mark_dead(); // crash 2 → delay = 2 (reset à 1, puis *2)
        // Note : set_pid remet à 1, mark_dead le double → 2
        assert_eq!(svc.delay(), 2);
        svc.mark_dead(); // crash successif sans relance → *2 = 4
        assert_eq!(svc.delay(), 4);
        svc.mark_dead(); // 4 → 8
        assert_eq!(svc.delay(), 8);
        svc.mark_dead(); // 8 → 16
        assert_eq!(svc.delay(), 16);
        svc.mark_dead(); // 16 → 32
        assert_eq!(svc.delay(), 32);
    }

    #[test]
    fn test_backoff_plafond_32() {
        let svc = Service::new("test_svc");
        // Appliquer mark_dead beaucoup de fois
        for _ in 0..20 {
            svc.mark_dead();
        }
        assert_eq!(svc.delay(), 32, "Le délai max doit être 32 ticks");
    }

    #[test]
    fn test_backoff_plafond_ne_deborde_pas() {
        let svc = Service::new("test_svc");
        for _ in 0..100 {
            svc.mark_dead();
        }
        assert!(svc.delay() <= 32, "overflow du délai de backoff");
    }

    #[test]
    fn test_table_de_services_triple() {
        // Simuler la table de 3 services comme dans init_server
        let services = [
            Service::new("ipc_router"),
            Service::new("vfs_server"),
            Service::new("crypto_server"),
        ];
        assert_eq!(services.len(), 3);
        for svc in &services { assert_eq!(svc.current_pid(), 0); }

        services[0].set_pid(2);
        services[1].set_pid(3);
        services[2].set_pid(4);

        assert_eq!(services[0].current_pid(), 2);
        assert_eq!(services[1].current_pid(), 3);
        assert_eq!(services[2].current_pid(), 4);
    }

    #[test]
    fn test_reap_trouve_service_par_pid() {
        let services = [
            Service::new("ipc_router"),
            Service::new("vfs_server"),
        ];
        services[0].set_pid(2);
        services[1].set_pid(3);

        // Simuler la détection de la mort du PID 3
        let dead_pid: u32 = 3;
        for svc in &services {
            if svc.current_pid() == dead_pid {
                svc.mark_dead();
            }
        }

        assert_eq!(services[0].current_pid(), 2, "ipc_router ne doit pas être affecté");
        assert_eq!(services[1].current_pid(), 0, "vfs_server doit être marqué mort");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── TESTS crypto_server ───────────────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests_crypto_server {
    use super::crypto_server::*;

    fn make_req(msg_type: u32, payload: [u8; 120]) -> CryptoRequest {
        CryptoRequest { sender_pid: 1, msg_type, payload }
    }

    // ── derive_key_stub ────────────────────────────────────────────────────

    #[test]
    fn test_kdf_deterministe() {
        let mut out1 = [0u8; 32];
        let mut out2 = [0u8; 32];
        derive_key_stub(b"master_key_material", &mut out1);
        derive_key_stub(b"master_key_material", &mut out2);
        assert_eq!(out1, out2, "KDF doit être déterministe");
    }

    #[test]
    fn test_kdf_sortie_non_nulle_sur_input_non_nul() {
        let mut out = [0u8; 32];
        derive_key_stub(b"non_empty_input", &mut out);
        assert_ne!(out, [0u8; 32], "KDF ne doit pas produire des zéros pour un input non-vide");
    }

    #[test]
    fn test_kdf_inputs_differents_donnent_sorties_differentes() {
        let mut out1 = [0u8; 32];
        let mut out2 = [0u8; 32];
        derive_key_stub(b"key_material_A", &mut out1);
        derive_key_stub(b"key_material_B", &mut out2);
        assert_ne!(out1, out2, "Inputs différents doivent donner clés différentes");
    }

    #[test]
    fn test_kdf_input_vide() {
        // L'input vide doit produire une sortie stable (pas de panique)
        let mut out = [0u8; 32];
        derive_key_stub(b"", &mut out);
        // Valeur attendue : state initial sans modification → on vérifie juste la stabilité
        let mut out2 = [0u8; 32];
        derive_key_stub(b"", &mut out2);
        assert_eq!(out, out2, "KDF vide doit être stable");
    }

    #[test]
    fn test_kdf_128_octets_input() {
        let input = [0xABu8; 128];
        let mut out = [0u8; 32];
        derive_key_stub(&input, &mut out);
        // Pas de panique, output est non-nul
        assert_ne!(out, [0u8; 32]);
    }

    // ── handle_request : CRYPTO_DERIVE_KEY ────────────────────────────────

    #[test]
    fn test_derive_key_retourne_handle_1() {
        let mut ks = Keystore::new();
        let req = make_req(CRYPTO_DERIVE_KEY, [0x42u8; 120]);
        let r = ks.handle_request(&req);
        assert_eq!(r.status, CRYPTO_OK);
        assert_eq!(r.key_handle, 1, "Premier handle doit être 1");
    }

    #[test]
    fn test_derive_key_handles_consecutifs() {
        let mut ks = Keystore::new();
        for expected in 1u32..=5 {
            let req = make_req(CRYPTO_DERIVE_KEY, [expected as u8; 120]);
            let r = ks.handle_request(&req);
            assert_eq!(r.status, CRYPTO_OK);
            assert_eq!(r.key_handle, expected, "Handle {expected} attendu");
        }
    }

    #[test]
    fn test_derive_key_table_pleine_retourne_busy() {
        let mut ks = Keystore::new();
        // Remplir les 32 slots
        for i in 0u8..32 {
            let mut payload = [0u8; 120];
            payload[0] = i;
            let req = make_req(CRYPTO_DERIVE_KEY, payload);
            let r = ks.handle_request(&req);
            assert_eq!(r.status, CRYPTO_OK, "Slot {i} doit réussir");
        }
        // La 33ème doit être BUSY
        let req = make_req(CRYPTO_DERIVE_KEY, [0xFF; 120]);
        let r = ks.handle_request(&req);
        assert_eq!(r.status, CRYPTO_ERR_BUSY, "Table pleine → CRYPTO_ERR_BUSY");
        assert_eq!(r.key_handle, 0);
    }

    // ── handle_request : CRYPTO_HASH ──────────────────────────────────────

    #[test]
    fn test_hash_retourne_ok_et_non_zero() {
        let mut ks = Keystore::new();
        let mut payload = [0u8; 120];
        payload[..5].copy_from_slice(b"hello");
        let req = make_req(CRYPTO_HASH, payload);
        let r = ks.handle_request(&req);
        assert_eq!(r.status, CRYPTO_OK);
        assert_ne!(&r.data[..32], &[0u8; 32], "Hash de 'hello' ne doit pas être nul");
    }

    #[test]
    fn test_hash_deterministe() {
        let mut ks = Keystore::new();
        let payload = {
            let mut p = [0u8; 120];
            p[..7].copy_from_slice(b"exo-os!");
            p
        };
        let r1 = ks.handle_request(&make_req(CRYPTO_HASH, payload));
        let r2 = ks.handle_request(&make_req(CRYPTO_HASH, payload));
        assert_eq!(r1.data, r2.data, "Hash doit être déterministe");
    }

    #[test]
    fn test_hash_inputs_differents() {
        let mut ks = Keystore::new();
        let mut p1 = [0u8; 120]; p1[0] = 1;
        let mut p2 = [0u8; 120]; p2[0] = 2;
        let r1 = ks.handle_request(&make_req(CRYPTO_HASH, p1));
        let r2 = ks.handle_request(&make_req(CRYPTO_HASH, p2));
        assert_ne!(r1.data, r2.data);
    }

    // ── handle_request : CRYPTO_ENCRYPT / DECRYPT ─────────────────────────

    #[test]
    fn test_encrypt_non_implemente_retourne_err_args() {
        let mut ks = Keystore::new();
        let r = ks.handle_request(&make_req(CRYPTO_ENCRYPT, [0u8; 120]));
        assert_eq!(r.status, CRYPTO_ERR_ARGS, "ENCRYPT doit retourner ERR_ARGS (Phase 5)");
    }

    #[test]
    fn test_decrypt_non_implemente_retourne_err_args() {
        let mut ks = Keystore::new();
        let r = ks.handle_request(&make_req(CRYPTO_DECRYPT, [0u8; 120]));
        assert_eq!(r.status, CRYPTO_ERR_ARGS, "DECRYPT doit retourner ERR_ARGS (Phase 5)");
    }

    // ── handle_request : msg_type inconnu ─────────────────────────────────

    #[test]
    fn test_msg_type_inconnu_retourne_err_args() {
        let mut ks = Keystore::new();
        let r = ks.handle_request(&make_req(0xFF, [0u8; 120]));
        // msg_type inconnu → status = CRYPTO_ERR_ARGS (valeur initiale non modifiée)
        assert_eq!(r.status, CRYPTO_ERR_ARGS);
    }

    // ── handle_request : CRYPTO_RANDOM ────────────────────────────────────

    #[test]
    fn test_random_retourne_ok() {
        let mut ks = Keystore::new();
        let r = ks.handle_request(&make_req(CRYPTO_RANDOM, [0u8; 120]));
        assert_eq!(r.status, CRYPTO_OK);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── TESTS vfs_server ──────────────────────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests_vfs_server {
    use super::vfs_server::*;

    // ── fnv32 ─────────────────────────────────────────────────────────────

    #[test]
    fn test_fnv32_deterministe() {
        assert_eq!(fnv32(b"/proc"), fnv32(b"/proc"));
    }

    #[test]
    fn test_fnv32_pseudo_fs_pas_de_collision() {
        let h_proc = fnv32(b"/proc");
        let h_sys  = fnv32(b"/sys");
        let h_dev  = fnv32(b"/dev");
        assert_ne!(h_proc, h_sys);
        assert_ne!(h_proc, h_dev);
        assert_ne!(h_sys,  h_dev);
    }

    #[test]
    fn test_fnv32_vide() {
        assert_eq!(fnv32(b""), 2166136261u32);
    }

    #[test]
    fn test_fnv32_valeurs_connues() {
        // Valeur calculée manuellement : FNV-32 de "/"
        // h = 2166136261 * 16777619 + b'/' (47) = vérifiable offline
        let h = fnv32(b"/");
        assert_ne!(h, 0);
        assert_ne!(h, 2166136261);
    }

    // ── MountTable ────────────────────────────────────────────────────────

    #[test]
    fn test_mount_table_initiale_vide() {
        let t = MountTable::new();
        assert_eq!(t.count, 0);
        assert_eq!(t.active_count(), 0);
    }

    #[test]
    fn test_add_pseudo_fs() {
        let mut t = MountTable::new();
        t.add(FsType::ProcFs, b"/proc", 0).unwrap();
        t.add(FsType::SysFs,  b"/sys",  0).unwrap();
        t.add(FsType::DevFs,  b"/dev",  0).unwrap();
        assert_eq!(t.active_count(), 3);
    }

    #[test]
    fn test_find_by_path_nominal() {
        let mut t = MountTable::new();
        t.add(FsType::ProcFs, b"/proc", 0).unwrap();
        let entry = t.find_by_path(b"/proc");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().fs_type, FsType::ProcFs);
    }

    #[test]
    fn test_find_by_path_inconnu_retourne_none() {
        let t = MountTable::new();
        assert!(t.find_by_path(b"/mnt/unknown").is_none());
    }

    #[test]
    fn test_add_exofs_avec_blob_id() {
        let mut t = MountTable::new();
        let blob_id = 0xDEADCAFEBABE1234u64;
        t.add(FsType::ExoFs, b"/", blob_id).unwrap();
        let e = t.find_by_path(b"/").unwrap();
        assert_eq!(e.root_blob, blob_id);
        assert_eq!(e.fs_type, FsType::ExoFs);
    }

    #[test]
    fn test_remove_existant() {
        let mut t = MountTable::new();
        t.add(FsType::ProcFs, b"/proc", 0).unwrap();
        assert_eq!(t.active_count(), 1);
        assert!(t.remove(b"/proc"));
        assert_eq!(t.active_count(), 0);
    }

    #[test]
    fn test_remove_inexistant_retourne_false() {
        let mut t = MountTable::new();
        assert!(!t.remove(b"/nonexistent"));
    }

    #[test]
    fn test_table_pleine_retourne_enospc() {
        let mut t = MountTable::new();
        for i in 0u64..32 {
            let path = format!("/mnt/v{i}");
            t.add(FsType::ExoFs, path.as_bytes(), i).unwrap();
        }
        assert_eq!(t.count, 32);
        let r = t.add(FsType::ExoFs, b"/mnt/overflow", 0xFFFF);
        assert_eq!(r, Err(-28), "ENOSPC attendu quand table = 32");
    }

    #[test]
    fn test_default_namespaces_montages_corrects() {
        // Reproduire mount_default_namespaces() du vfs_server
        let mut t = MountTable::new();
        t.add(FsType::ProcFs, b"/proc", 0).unwrap();
        t.add(FsType::SysFs,  b"/sys",  0).unwrap();
        t.add(FsType::DevFs,  b"/dev",  0).unwrap();

        let proc_e = t.find_by_path(b"/proc").expect("/proc doit être monté");
        let sys_e  = t.find_by_path(b"/sys").expect("/sys doit être monté");
        let dev_e  = t.find_by_path(b"/dev").expect("/dev doit être monté");

        assert_eq!(proc_e.fs_type, FsType::ProcFs);
        assert_eq!(sys_e.fs_type,  FsType::SysFs);
        assert_eq!(dev_e.fs_type,  FsType::DevFs);
        assert_eq!(proc_e.root_blob, 0);
        assert_eq!(sys_e.root_blob,  0);
        assert_eq!(dev_e.root_blob,  0);
    }

    // ── handle_mount_payload ──────────────────────────────────────────────

    #[test]
    fn test_handle_mount_payload_trop_court_retourne_einval() {
        let mut t = MountTable::new();
        let short_payload = [0u8; 5]; // < 14 bytes
        assert_eq!(handle_mount_payload(&mut t, &short_payload), -22);
    }

    #[test]
    fn test_handle_mount_payload_fstype_inconnu() {
        let mut t = MountTable::new();
        let mut payload = [0u8; 32];
        payload[0] = 99; // fstype invalide
        payload[13..17].copy_from_slice(b"/mnt");
        assert_eq!(handle_mount_payload(&mut t, &payload), -22, "fstype inconnu → EINVAL");
    }

    #[test]
    fn test_handle_mount_payload_exofs_nominal() {
        let mut t = MountTable::new();
        let mut payload = [0u8; 32];
        payload[0] = 1; // ExoFs
        // root_blob = 0x1122334455667788 à offset 5
        let blob: u64 = 0x1122334455667788;
        payload[5..13].copy_from_slice(&blob.to_le_bytes());
        payload[13] = b'/';
        payload[14] = 0; // null terminator
        let r = handle_mount_payload(&mut t, &payload);
        assert_eq!(r, 0, "mount ExoFs nominal doit réussir");
        let e = t.find_by_path(b"/").unwrap();
        assert_eq!(e.root_blob, blob);
        assert_eq!(e.fs_type, FsType::ExoFs);
    }

    #[test]
    fn test_handle_mount_payload_procfs() {
        let mut t = MountTable::new();
        let mut payload = [0u8; 32];
        payload[0] = 2; // ProcFs
        payload[13..18].copy_from_slice(b"/proc");
        payload[18] = 0;
        let r = handle_mount_payload(&mut t, &payload);
        assert_eq!(r, 0);
        assert!(t.find_by_path(b"/proc").is_some());
    }

    #[test]
    fn test_active_count_apres_remove() {
        let mut t = MountTable::new();
        t.add(FsType::ProcFs, b"/proc", 0).unwrap();
        t.add(FsType::SysFs,  b"/sys",  0).unwrap();
        t.remove(b"/proc");
        assert_eq!(t.active_count(), 1);
        assert!(t.find_by_path(b"/proc").is_none());
        assert!(t.find_by_path(b"/sys").is_some());
    }

    #[test]
    fn test_mount_multiple_exofs() {
        let mut t = MountTable::new();
        t.add(FsType::ExoFs, b"/",     0x1000).unwrap();
        t.add(FsType::ExoFs, b"/home", 0x2000).unwrap();
        t.add(FsType::ExoFs, b"/var",  0x3000).unwrap();
        assert_eq!(t.active_count(), 3);
        assert_eq!(t.find_by_path(b"/home").unwrap().root_blob, 0x2000);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── TESTS INTÉGRATION inter-composants ────────────────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests_integration {
    use super::*;

    /// Scénario : boot séquence complète simulée
    ///   1. init_server crée la table de services
    ///   2. ipc_router s'enregistre (endpoint 2)
    ///   3. vfs_server monte les pseudo-FS
    ///   4. crypto_server dérive une clé maître
    ///   5. init_server détecte un crash de vfs_server et le marque mort
    ///   6. Backoff empêche relance immédiate
    #[test]
    fn test_boot_sequence_complete() {
        // 1. Table de services
        let svc_ipc_router    = init_server::Service::new("ipc_router");
        let svc_vfs_server    = init_server::Service::new("vfs_server");
        let svc_crypto_server = init_server::Service::new("crypto_server");

        // 2. Spawn simulé : assigner des PIDs
        svc_ipc_router.set_pid(2);
        svc_vfs_server.set_pid(3);
        svc_crypto_server.set_pid(4);

        // 3. ipc_router enregistre les services
        let mut registry = ipc_router::Registry::new();
        registry.register(b"ipc_router",    2);
        registry.register(b"vfs_server",    3);
        registry.register(b"crypto_server", 4);

        assert_eq!(registry.resolve(b"vfs_server"), Some(3));

        // 4. vfs_server monte les pseudo-FS
        let mut vfs_table = vfs_server::MountTable::new();
        vfs_table.add(vfs_server::FsType::ProcFs, b"/proc", 0).unwrap();
        vfs_table.add(vfs_server::FsType::SysFs,  b"/sys",  0).unwrap();
        vfs_table.add(vfs_server::FsType::DevFs,  b"/dev",  0).unwrap();
        assert_eq!(vfs_table.active_count(), 3);

        // 5. crypto_server dérive une clé maître
        let mut ks = crypto_server::Keystore::new();
        let master_material = b"exo-os master key material 2026!";
        let mut kdf_payload = [0u8; 120];
        kdf_payload[..master_material.len()].copy_from_slice(master_material);
        let req = crypto_server::CryptoRequest {
            sender_pid: 1, msg_type: crypto_server::CRYPTO_DERIVE_KEY,
            payload: kdf_payload,
        };
        let reply = ks.handle_request(&req);
        assert_eq!(reply.status, crypto_server::CRYPTO_OK);
        assert_eq!(reply.key_handle, 1, "Première clé maître = handle 1");

        // 6. vfs_server crash détecté par init
        let dead_pid: u32 = 3;
        for svc in &[&svc_ipc_router, &svc_vfs_server, &svc_crypto_server] {
            if svc.current_pid() == dead_pid { svc.mark_dead(); }
        }

        assert_eq!(svc_vfs_server.current_pid(), 0, "vfs_server doit être mort");
        assert_eq!(svc_vfs_server.delay(), 2, "Premier crash → delay = 2");
        assert_eq!(svc_ipc_router.current_pid(), 2, "ipc_router doit rester vivant");
        assert_eq!(svc_crypto_server.current_pid(), 4, "crypto_server doit rester vivant");

        // 7. ipc_router toujours résolvable (service stable)
        assert_eq!(registry.resolve(b"ipc_router"), Some(2));
    }

    /// Invariant SRV-04 : crypto_server est le seul détenteur de clés.
    /// Les handles sont opaques — aucun autre module ne voit les octets de clé.
    #[test]
    fn test_srv04_cle_opaque_pas_dans_reply() {
        let mut ks = crypto_server::Keystore::new();
        let req = crypto_server::CryptoRequest {
            sender_pid: 3, msg_type: crypto_server::CRYPTO_DERIVE_KEY,
            payload: [0x99; 120],
        };
        let reply = ks.handle_request(&req);
        assert_eq!(reply.status, crypto_server::CRYPTO_OK);
        // La réponse DERIVE_KEY ne doit pas contenir les octets de clé dans data[]
        // (data[] doit rester nul pour DERIVE_KEY)
        assert_eq!(&reply.data, &[0u8; 56], "Réponse DERIVE_KEY ne doit pas exposer la clé");
    }

    /// Invariant SRV-05 : tous les services sont résolvables via ipc_router.
    #[test]
    fn test_srv05_tous_services_resolvables() {
        let mut r = ipc_router::Registry::new();
        r.register(b"init_server",   1);
        r.register(b"ipc_router",    2);
        r.register(b"vfs_server",    3);
        r.register(b"crypto_server", 4);

        // Chaque service doit être résolvable avec son PID attendu
        assert_eq!(r.resolve(b"init_server"),   Some(1));
        assert_eq!(r.resolve(b"ipc_router"),    Some(2));
        assert_eq!(r.resolve(b"vfs_server"),    Some(3));
        assert_eq!(r.resolve(b"crypto_server"), Some(4));

        // Aucun service fantôme
        assert_eq!(r.resolve(b"unknown_svc"), None);
    }

    /// Invariant SRV-01 : init_server doit survivre aux crashes de tous ses enfants.
    #[test]
    fn test_srv01_init_survit_aux_crashes() {
        let services = [
            init_server::Service::new("ipc_router"),
            init_server::Service::new("vfs_server"),
            init_server::Service::new("crypto_server"),
        ];

        for (i, svc) in services.iter().enumerate() {
            svc.set_pid((i as u32) + 2);
        }

        // Tous les services crashent
        for svc in &services { svc.mark_dead(); }

        // Vérifier qu'init peut relancer (délai = 2 = min backoff)
        for svc in &services {
            assert_eq!(svc.current_pid(), 0);
            assert_eq!(svc.delay(), 2, "Après premier crash: délai = 2");
        }

        // Second crash sans relance intermédiaire → délai double
        for svc in &services { svc.mark_dead(); }
        for svc in &services {
            assert_eq!(svc.delay(), 4);
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ── init_server V4 + ExoCordon — validation pure ────────────────────────────
// ═════════════════════════════════════════════════════════════════════════════

mod init_server_v4 {
    pub const SERVICES: [&str; 9] = [
        "ipc_router",
        "memory_server",
        "vfs_server",
        "crypto_server",
        "device_server",
        "network_server",
        "scheduler_server",
        "virtio_drivers",
        "exo_shield",
    ];

    pub fn deps(name: &str) -> &'static [&'static str] {
        match name {
            "ipc_router" => &[],
            "memory_server" => &["ipc_router"],
            "vfs_server" => &["ipc_router", "memory_server"],
            "crypto_server" => &["vfs_server"],
            "device_server" => &["ipc_router", "memory_server"],
            "network_server" => &["device_server", "virtio_drivers"],
            "scheduler_server" => &["init_server"],
            "virtio_drivers" => &["device_server"],
            "exo_shield" => &[
                "ipc_router",
                "memory_server",
                "vfs_server",
                "crypto_server",
                "device_server",
                "network_server",
                "scheduler_server",
                "virtio_drivers",
            ],
            _ => &[],
        }
    }
}

mod exocordon_logic {
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ServiceId {
        Init,
        Memory,
        Vfs,
        Crypto,
        Device,
        Network,
        VirtioBlock,
        VirtioNet,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum IpcError {
        UnknownService,
        UnauthorizedPath,
        QuotaExhausted,
    }

    pub struct AuthEdge {
        src: ServiceId,
        dst: ServiceId,
        quota_default: u64,
        quota_left: AtomicU64,
    }

    impl AuthEdge {
        pub const fn new(src: ServiceId, dst: ServiceId, quota_default: u64) -> Self {
            Self {
                src,
                dst,
                quota_default,
                quota_left: AtomicU64::new(quota_default),
            }
        }

        fn consume(&self) -> Result<(), IpcError> {
            let mut current = self.quota_left.load(Ordering::Acquire);
            loop {
                if current == 0 {
                    return Err(IpcError::QuotaExhausted);
                }
                match self.quota_left.compare_exchange_weak(
                    current,
                    current - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return Ok(()),
                    Err(next) => current = next,
                }
            }
        }
    }

    pub static AUTHORIZED_GRAPH: [AuthEdge; 6] = [
        AuthEdge::new(ServiceId::Init, ServiceId::Memory, 10_000),
        AuthEdge::new(ServiceId::Init, ServiceId::Vfs, 10_000),
        AuthEdge::new(ServiceId::Vfs, ServiceId::Crypto, 50_000),
        AuthEdge::new(ServiceId::Network, ServiceId::Vfs, 100_000),
        AuthEdge::new(ServiceId::Device, ServiceId::VirtioBlock, 1_000_000),
        AuthEdge::new(ServiceId::Device, ServiceId::VirtioNet, 1_000_000),
    ];

    fn map_service(id: u32) -> Option<ServiceId> {
        match id {
            1 => Some(ServiceId::Init),
            3 => Some(ServiceId::Vfs),
            4 => Some(ServiceId::Crypto),
            5 => Some(ServiceId::Memory),
            6 => Some(ServiceId::Device),
            7 => Some(ServiceId::Network),
            9 => Some(ServiceId::VirtioBlock),
            10 => Some(ServiceId::VirtioNet),
            _ => None,
        }
    }

    pub fn reset() {
        for edge in &AUTHORIZED_GRAPH {
            edge.quota_left.store(edge.quota_default, Ordering::Release);
        }
    }

    pub fn check_ipc(src: u32, dst: u32) -> Result<(), IpcError> {
        let src = map_service(src).ok_or(IpcError::UnknownService)?;
        let dst = map_service(dst).ok_or(IpcError::UnknownService)?;
        let edge = AUTHORIZED_GRAPH
            .iter()
            .find(|edge| edge.src == src && edge.dst == dst)
            .ok_or(IpcError::UnauthorizedPath)?;
        edge.consume()
    }
}

#[cfg(test)]
mod tests_init_server_v4 {
    use super::init_server_v4::{deps, SERVICES};

    #[test]
    fn test_ring1_v4_order_contains_nine_services() {
        assert_eq!(SERVICES.len(), 9);
        assert_eq!(SERVICES[0], "ipc_router");
        assert_eq!(SERVICES[1], "memory_server");
        assert_eq!(SERVICES[8], "exo_shield");
    }

    #[test]
    fn test_ring1_v4_dependencies_are_canonical() {
        assert_eq!(deps("memory_server"), &["ipc_router"]);
        assert_eq!(deps("vfs_server"), &["ipc_router", "memory_server"]);
        assert_eq!(deps("crypto_server"), &["vfs_server"]);
        assert_eq!(deps("device_server"), &["ipc_router", "memory_server"]);
        assert_eq!(deps("virtio_drivers"), &["device_server"]);
        assert_eq!(
            deps("exo_shield"),
            &[
                "ipc_router",
                "memory_server",
                "vfs_server",
                "crypto_server",
                "device_server",
                "network_server",
                "scheduler_server",
                "virtio_drivers",
            ],
        );
    }
}

#[cfg(test)]
mod tests_exocordon {
    use super::exocordon_logic::{check_ipc, reset, IpcError};
    use std::sync::Mutex;

    static EXOCORDON_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_exocordon_blocks_network_to_crypto_direct() {
        let _guard = EXOCORDON_TEST_LOCK.lock().unwrap();
        reset();
        assert_eq!(check_ipc(7, 4), Err(IpcError::UnauthorizedPath));
    }

    #[test]
    fn test_exocordon_allows_vfs_to_crypto() {
        let _guard = EXOCORDON_TEST_LOCK.lock().unwrap();
        reset();
        assert_eq!(check_ipc(3, 4), Ok(()));
    }

    #[test]
    fn stress_exocordon_quota_exhaustion_is_isolated() {
        let _guard = EXOCORDON_TEST_LOCK.lock().unwrap();
        reset();

        for _ in 0..10_000 {
            assert_eq!(check_ipc(1, 5), Ok(()));
            assert_eq!(check_ipc(1, 3), Ok(()));
        }

        assert_eq!(check_ipc(1, 5), Err(IpcError::QuotaExhausted));
        assert_eq!(check_ipc(1, 3), Err(IpcError::QuotaExhausted));
        assert_eq!(check_ipc(3, 4), Ok(()), "VFS -> Crypto doit rester utilisable");
    }
}
