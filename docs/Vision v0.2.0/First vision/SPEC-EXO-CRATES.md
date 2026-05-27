# SPEC-EXO-CRATES — Spécification des Crates Natives ExoOS
## exo-alloc · exo-net · exo-crypto · exo-fs · exo-runtime

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0

---

# PARTIE 1 — exo-alloc

## 1.1 Rôle

Fournir l'allocateur mémoire userland standard d'ExoOS. Toutes les crates Ring3 qui ont besoin d'allouer de la mémoire dynamique dépendent d'`exo-alloc`.

## 1.2 Backends

| Backend | Condition | Notes |
|---------|-----------|-------|
| `snmalloc-rs` | Architectures supportées (x86_64 ✓) | Principal — pools par thread, sécurisé |
| `dlmalloc` | Fallback (cfg gate) | Portable, mature, simple |
| `jemallocator` | Opt-in uniquement pour binaires multi-thread intensifs | Jamais en Ring1 |

**Ne jamais utiliser en Ring1 :** Les arènes de `jemalloc` maintiennent un état global qui peut créer des corruptions cross-process lors d'un fork. `snmalloc` est conçu pour cela.

## 1.3 Interface

```rust
// exo-alloc/src/lib.rs
#![no_std]
extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};

pub struct ExoAllocator;

unsafe impl GlobalAlloc for ExoAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Taille arrondie au multiple de layout.align()
        let size = align_up(layout.size(), layout.align());
        exo_mmap_anon(size, layout.align())
            .map(|p| p.as_ptr())
            .unwrap_or(core::ptr::null_mut())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = align_up(layout.size(), layout.align());
        let _ = exo_munmap(ptr as usize, size);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // mremap si disponible, sinon alloc+copy+free
        let new_aligned = align_up(new_size, layout.align());
        exo_mremap(ptr as usize, layout.size(), new_aligned)
            .map(|p| p.as_ptr())
            .unwrap_or_else(|| {
                let new_ptr = self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
                if !new_ptr.is_null() {
                    core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                    self.dealloc(ptr, layout);
                }
                new_ptr
            })
    }
}

#[global_allocator]
static GLOBAL: ExoAllocator = ExoAllocator;

/// Arrondit `size` au multiple supérieur de `align`.
fn align_up(size: usize, align: usize) -> usize {
    (size + align - 1) & !(align - 1)
}
```

## 1.4 Syscalls utilisés

| Syscall | Numéro | Usage |
|---------|--------|-------|
| `SYS_MMAP` | 9 | Réserver une plage mémoire (MAP_ANON \| MAP_PRIVATE) |
| `SYS_MUNMAP` | 11 | Libérer une plage |
| `SYS_MREMAP` | 25 | Redimensionner une plage |
| `SYS_MPROTECT` | 10 | Modifier les protections |

## 1.5 ExoPhoenix-Safety

`ExoAllocator` est stateless — il n'y a pas d'état interne à sauvegarder. Après une bascule, les arènes snmalloc sont perdues mais les pages mmap survivent (elles appartiennent à l'espace d'adressage du processus, qui est restauré depuis le SSR).

---

# PARTIE 2 — exo-net

## 2.1 Architecture

```
Ring3 App
    │  exo-net::TcpStream::connect("93.184.216.34:443")?
    │
    ▼
exo-net (Ring3 client)
    │  IPC: NetRequest::Connect { addr, capability }
    │  via SpscRing → network_server
    │
    ▼
network_server (Ring1)
    │  smoltcp: socket::TcpSocket::connect()
    │  retourne: SocketHandle + CapToken
    │
    └─► ExoNet::Socket { cap: CapToken }
```

## 2.2 Interface Ring3 (exo-net)

```rust
// exo-net/src/lib.rs — API Ring3

pub struct TcpStream {
    cap: CapToken,          // capability réseau délivrée par network_server
    socket_handle: u32,     // identifiant opaque côté serveur
}

impl TcpStream {
    pub fn connect(addr: SocketAddr) -> Result<Self, NetError> {
        let cap = ipc_send_recv(
            NetEndpoint::ID,
            NetRequest::Connect { addr, mode: ConnectMode::Blocking },
        )?;
        Ok(TcpStream { cap: cap.token, socket_handle: cap.handle })
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        let resp = ipc_send_recv(
            NetEndpoint::ID,
            NetRequest::Read { handle: self.socket_handle, cap: self.cap, len: buf.len() },
        )?;
        buf[..resp.data.len()].copy_from_slice(&resp.data);
        Ok(resp.data.len())
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, NetError> {
        ipc_send_recv(
            NetEndpoint::ID,
            NetRequest::Write { handle: self.socket_handle, cap: self.cap, data: data.to_vec() },
        ).map(|r| r.bytes_written)
    }
}

pub struct UdpSocket { cap: CapToken, handle: u32 }

/// DNS — via hickory-dns intégré dans network_server
pub fn resolve(name: &str) -> Result<Vec<IpAddr>, NetError> {
    ipc_send_recv(NetEndpoint::ID, NetRequest::Dns { name: name.into() })
        .map(|r| r.addresses)
}
```

## 2.3 network_server interne (Ring1 — smoltcp)

```rust
// network_server/src/main.rs (Ring1)

fn main() {
    // 1. Récupérer la capability NIC depuis device_server
    let nic_cap = ipc_send_recv(DevEndpoint::ID, DevRequest::ClaimNic).unwrap();

    // 2. Créer l'interface smoltcp
    let device = ExoDevice::from_cap(nic_cap);
    let mut iface = Interface::new(device, Config::default());

    // Configurer via DHCP (dhcp4r)
    let ip = dhcp4r::lease(&mut iface).unwrap();
    iface.set_ip_addresses([IpCidr::new(ip.addr, ip.prefix)]);
    iface.set_routes(... ip.gateway ...);

    // 3. Boucle principale (jamais de panic!, toujours propager les erreurs)
    let mut sockets = SocketSet::new(alloc::vec![]);
    loop {
        let timestamp = exo_ktime_now();
        match iface.poll(&mut sockets, timestamp) {
            Ok(_) => {}
            Err(smoltcp::Error::Exhausted) => {}
            Err(e) => log::warn!("smoltcp: {:?}", e),  // pas de panic!
        }

        // Traiter les requêtes IPC entrantes
        while let Ok(req) = ipc_try_recv(NetEndpoint::ID) {
            handle_request(&mut sockets, &mut iface, req);
        }

        // Yield si pas d'événement (ne jamais busy-loop)
        if !iface.poll_delay(&sockets, timestamp).is_zero() {
            sys_sched_yield();
        }
    }
}
```

## 2.4 TLS via rustls

```rust
// exo-tls/src/lib.rs — TLS au-dessus d'exo-net

pub struct TlsStream {
    inner: exo_net::TcpStream,
    conn:  rustls::ClientConnection,
}

impl TlsStream {
    pub fn connect(addr: SocketAddr, server_name: &str) -> Result<Self, TlsError> {
        let tcp = exo_net::TcpStream::connect(addr)?;
        
        // Certificats racine depuis le keystore (crypto_server)
        let root_certs = exo_crypto::load_root_certs()?;
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_certs)
            .with_no_client_auth();
        
        let conn = rustls::ClientConnection::new(Arc::new(config), server_name.try_into()?)?;
        Ok(TlsStream { inner: tcp, conn })
    }
    
    // Les opérations crypto (ECDHE, AES-GCM) peuvent être déléguées
    // au crypto_server via une implémentation custom de CryptoProvider
}
```

## 2.5 ExoPhoenix-Safety

`on_pre_switch` : invalider tous les `TcpStream` actifs (les numéros de séquence TCP ne survivent pas à une bascule de kernel).
`on_post_switch` : les connexions TCP doivent être rétablies par l'application. Les sockets UDP avec `KeepAlive` peuvent être recréées automatiquement.

---

# PARTIE 3 — exo-crypto

## 3.1 Architecture

```
Ring3 App
    │  exo_crypto::aead::encrypt(key_cap, nonce, data)
    │
    ▼
exo-crypto (Ring3 client)
    │  IPC: CryptoRequest::Encrypt { key_cap, nonce, aad, data }
    │  Les clés ne sortent JAMAIS du crypto_server
    │
    ▼
crypto_server (Ring1)
    │  RustCrypto: AesGcm::encrypt(key_bytes, nonce, data)
    │  ring: pour les primitives ASM (RSA, ECDSA)
    │  TRNG: security/crypto/rng.rs (RDRAND/RDSEED)
    │
    └─► CryptoResponse::Ciphertext(data)
```

## 3.2 Interface Ring3

```rust
// exo-crypto/src/lib.rs

/// Clé symétrique référencée par capability.
/// La valeur brute de la clé ne sort jamais de crypto_server.
pub struct SymKey(pub CapToken);

/// Clé asymétrique (paire publique/privée) référencée par capability.
pub struct AsymKey {
    pub cap:     CapToken,
    pub pub_key: Vec<u8>,  // La clé publique PEUT être exportée
}

// ── AEAD ──────────────────────────────────────────────────────────
pub mod aead {
    pub fn encrypt(key: &SymKey, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8])
        -> Result<Vec<u8>, CryptoError>
    {
        ipc_crypto(CryptoRequest::Encrypt {
            key: key.0, nonce: *nonce, aad: aad.to_vec(), data: plaintext.to_vec()
        })
    }

    pub fn decrypt(key: &SymKey, nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8])
        -> Result<Vec<u8>, CryptoError>
    {
        ipc_crypto(CryptoRequest::Decrypt {
            key: key.0, nonce: *nonce, aad: aad.to_vec(), data: ciphertext.to_vec()
        })
    }
}

// ── HASH ──────────────────────────────────────────────────────────
pub mod hash {
    /// Hachage local (aucun secret impliqué — pas besoin d'IPC)
    pub fn blake3(data: &[u8]) -> [u8; 32] {
        // RustCrypto blake3 — exécuté localement en Ring3
        blake3::hash(data).into()
    }

    pub fn sha256(data: &[u8]) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        Sha256::digest(data).into()
    }
}

// ── KDF ───────────────────────────────────────────────────────────
pub mod kdf {
    pub fn hkdf_sha256(ikm: &SymKey, salt: &[u8], info: &[u8], len: usize)
        -> Result<SymKey, CryptoError>
    {
        // Dérivation dans crypto_server (le matériel clé reste dans le serveur)
        ipc_crypto(CryptoRequest::DeriveKey {
            base_key: ikm.0, algorithm: KdfAlgo::HkdfSha256,
            salt: salt.to_vec(), info: info.to_vec(), output_len: len
        }).map(|resp| SymKey(resp.new_key_cap))
    }
}

// ── PASSWORD HASH ─────────────────────────────────────────────────
pub mod password {
    pub fn argon2id(password: &[u8], salt: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
        ipc_crypto(CryptoRequest::PasswordHash {
            algorithm: PwHashAlgo::Argon2id { m: 65536, t: 3, p: 4 },
            password: password.to_vec(), salt: *salt
        })
    }
}

// ── SIGNATURE ─────────────────────────────────────────────────────
pub mod sign {
    pub fn ed25519_sign(key: &AsymKey, msg: &[u8]) -> Result<[u8; 64], CryptoError> {
        ipc_crypto(CryptoRequest::Sign {
            key: key.cap, algorithm: SignAlgo::Ed25519, msg: msg.to_vec()
        })
    }

    pub fn ed25519_verify(pub_key: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
        // Vérification locale (clé publique) — pas besoin de crypto_server
        use ed25519_dalek::{Verifier, VerifyingKey, Signature};
        let vk = VerifyingKey::from_bytes(pub_key).unwrap();
        let sig = Signature::from_bytes(sig);
        vk.verify(msg, &sig).is_ok()
    }
}

// ── KEY MANAGEMENT ────────────────────────────────────────────────
pub mod keys {
    pub fn generate_sym(algo: SymAlgo) -> Result<SymKey, CryptoError> {
        ipc_crypto(CryptoRequest::GenerateKey { algo: KeyAlgo::Sym(algo) })
            .map(|r| SymKey(r.key_cap))
    }

    pub fn generate_asym(algo: AsymAlgo) -> Result<AsymKey, CryptoError> {
        ipc_crypto(CryptoRequest::GenerateKey { algo: KeyAlgo::Asym(algo) })
            .map(|r| AsymKey { cap: r.key_cap, pub_key: r.pub_key_bytes })
    }

    pub fn load_root_certs() -> Result<rustls::RootCertStore, CryptoError> {
        // Certificats racine stockés dans le keystore du crypto_server
        ipc_crypto(CryptoRequest::GetRootCerts)
            .map(|r| r.cert_store)
    }
}
```

## 3.3 crypto_server interne (Ring1)

Le `crypto_server` doit utiliser le TRNG matériel comme source d'entropie primaire, **avant** ring::SystemRandom :

```rust
// Dans crypto_server — initialisation du RNG
fn init_rng() -> impl CryptoRng {
    // Priorité 1: RDRAND/RDSEED (kernel/src/security/crypto/rng.rs)
    if cpu_supports_rdrand() {
        ExoTrngRng::new()  // Utilise le TRNG du kernel
    } else {
        // Fallback: ring::SystemRandom (utilise getrandom via musl-exo)
        ring::rand::SystemRandom::new()
    }
}
```

---

# PARTIE 4 — exo-fs

## 4.1 Ce qui rend ExoFS unique (et que les libs doivent exposer)

| Primitive | Description | Avantage vs POSIX |
|-----------|-------------|-------------------|
| `blob_id()` | Identifiant content-addressed (BLAKE3) | Déduplication automatique |
| `epoch_commit()` | Commit atomique d'un ensemble de modifications | Snapshots O(1) |
| `relation_create()` | Lien sémantique typé entre objets | Graphe d'objets, pas juste une hiérarchie |
| `snapshot_create()` | Snapshot instantané d'un répertoire | Rollback garanti |
| `object_move()` | Déplacement O(1) quelle que soit la localisation | Pas de copie cross-partition |

## 4.2 Interface Native Ring3 (exo-fs)

```rust
// exo-fs/src/lib.rs — API native ExoFS

pub struct ExoFile {
    cap:    CapToken,
    handle: ObjectId,
}

impl ExoFile {
    /// Ouvrir un blob par chemin
    pub fn open(path: &str, rights: FsRights) -> Result<Self, FsError> {
        ipc_vfs(VfsRequest::Open { path: path.into(), rights })
            .map(|r| ExoFile { cap: r.cap, handle: r.object_id })
    }
    
    /// Lire à un offset donné (pas de curseur implicite — ExoFS style)
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        ipc_vfs(VfsRequest::ReadAt { handle: self.handle, cap: self.cap, offset, len: buf.len() })
            .map(|r| { buf[..r.data.len()].copy_from_slice(&r.data); r.data.len() })
    }

    /// Créer un snapshot de l'état actuel
    pub fn snapshot(&self) -> Result<EpochId, FsError> {
        ipc_vfs(VfsRequest::SnapshotCreate { handle: self.handle, cap: self.cap })
            .map(|r| r.epoch_id)
    }
    
    /// Hash content-addressé du blob
    pub fn content_hash(&self) -> Result<[u8; 32], FsError> {
        ipc_vfs(VfsRequest::GetContentHash { handle: self.handle })
            .map(|r| r.hash)
    }
}

/// Relation typée entre deux objets ExoFS
pub struct Relation {
    pub from: ObjectId,
    pub to:   ObjectId,
    pub kind: RelationType,
    pub cap:  CapToken,
}

pub fn create_relation(from: ObjectId, to: ObjectId, kind: RelationType, cap: CapToken)
    -> Result<Relation, FsError>
{
    ipc_vfs(VfsRequest::RelationCreate { from, to, kind, cap })
        .map(|r| Relation { from, to, kind, cap: r.rel_cap })
}
```

## 4.3 Compatibilité POSIX (couche au-dessus)

```rust
// exo-fs/src/posix.rs — POSIX optionnel par-dessus ExoFS natif

pub struct File(ExoFile);

impl File {
    /// API POSIX — `open()` standard
    pub fn open_posix(path: &str, flags: OpenFlags) -> Result<Self, FsError> {
        let rights = flags.to_exo_rights();
        ExoFile::open(path, rights).map(File)
    }
}

impl std::io::Read for File {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Utilise un curseur interne + read_at
        self.0.read_at(self.cursor, buf)
            .map(|n| { self.cursor += n as u64; n })
            .map_err(|e| e.into())
    }
}
```

---

# PARTIE 5 — exo-runtime

## 5.1 Rôle

Fournir un exécuteur asynchrone `no_std` basé sur le scheduler ExoOS. Les applications peuvent utiliser `async/await` sans dépendre de tokio.

## 5.2 Architecture

```rust
// exo-runtime/src/lib.rs

/// Exécuteur minimaliste basé sur sched_yield
pub struct ExoExecutor {
    tasks:   VecDeque<Task>,
    timers:  BinaryHeap<TimerEntry>,
    wakers:  HashMap<TaskId, Waker>,
}

impl ExoExecutor {
    pub fn new() -> Self { ... }

    pub fn spawn<F: Future<Output = ()> + 'static>(&mut self, fut: F) -> TaskId {
        let id = TaskId::next();
        self.tasks.push_back(Task::new(id, Box::pin(fut)));
        id
    }

    pub fn run(&mut self) -> ! {
        loop {
            // 1. Dépiler les timers expirés
            let now = exo_ktime_now();
            while let Some(entry) = self.timers.peek() {
                if entry.deadline <= now {
                    let entry = self.timers.pop().unwrap();
                    if let Some(waker) = self.wakers.remove(&entry.task_id) {
                        waker.wake();
                    }
                } else { break; }
            }

            // 2. Exécuter les tâches prêtes
            let mut ran_any = false;
            for _ in 0..self.tasks.len() {
                let mut task = self.tasks.pop_front().unwrap();
                let waker = task.waker();
                let mut cx = Context::from_waker(&waker);
                match task.poll(&mut cx) {
                    Poll::Ready(()) => {}  // tâche terminée
                    Poll::Pending => {
                        self.tasks.push_back(task);
                        ran_any = true;
                    }
                }
            }

            // 3. Yield si rien à faire (anti-busy-loop)
            if !ran_any && self.timers.is_empty() {
                sys_sched_yield();
            } else if !ran_any {
                // Calculer le prochain timer et sleep
                let next = self.timers.peek().unwrap().deadline;
                sys_nanosleep(next.saturating_sub(exo_ktime_now()));
            }
        }
    }
}

/// Timer future — Sleep pendant `duration`
pub struct Sleep { deadline: u64 }

impl Future for Sleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if exo_ktime_now() >= self.deadline {
            Poll::Ready(())
        } else {
            // Enregistrer le waker dans l'executor (via thread-local)
            CURRENT_EXECUTOR.with(|ex| ex.register_timer(self.deadline, cx.waker().clone()));
            Poll::Pending
        }
    }
}

pub async fn sleep(duration: Duration) {
    Sleep { deadline: exo_ktime_now() + duration.as_nanos() as u64 }.await
}
```

## 5.3 Usage

```rust
// Dans une application Ring3
fn main() {
    let mut executor = exo_runtime::ExoExecutor::new();
    
    executor.spawn(async {
        let mut stream = exo_net::TcpStream::connect("93.184.216.34:80".parse().unwrap())
            .await.unwrap();
        stream.write(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        exo_println!("{}", core::str::from_utf8(&buf[..n]).unwrap());
    });
    
    executor.run()
}
```

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-CRATES.md*
