# SPEC-EXO-CRATES-STRATA — Crates Natives ExoOS
## exo-alloc · exo-net · exo-crypto · exo-fs · exo-runtime

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace SPEC-EXO-CRATES.md

---

## 1. exo-alloc

### 1.1 Rôle

Allocateur mémoire userland universel pour toutes les crates Ring3 ExoOS.
Zéro dépendance `libc`, zéro `malloc`. Tout passe par `SYS_MMAP`/`SYS_MUNMAP`/`SYS_MREMAP`.

### 1.2 Implémentation

```rust
// libs/exo-alloc/src/lib.rs
#![no_std]

pub struct ExoAllocator;

unsafe impl GlobalAlloc for ExoAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = align_up(layout.size(), layout.align());
        sys_mmap(0, size, PROT_READ | PROT_WRITE,
                 MAP_ANON | MAP_PRIVATE, !0, 0)
            .map(|p| p as *mut u8)
            .unwrap_or(core::ptr::null_mut())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = align_up(layout.size(), layout.align());
        let _ = sys_munmap(ptr as usize, size);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_aligned = align_up(new_size, layout.align());
        // mremap en premier (évite la copie si possible)
        if let Ok(p) = sys_mremap(ptr as usize, layout.size(), new_aligned, MREMAP_MAYMOVE) {
            return p as *mut u8;
        }
        // Fallback : alloc + copy + free
        let new_ptr = self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
        if !new_ptr.is_null() {
            core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: ExoAllocator = ExoAllocator;

fn align_up(size: usize, align: usize) -> usize {
    (size + align - 1) & !(align - 1)
}
```

### 1.3 Syscalls Utilisés

| Syscall | N° | Usage |
|---|---|---|
| `SYS_MMAP` | 9 | Réserver plage (MAP_ANON \| MAP_PRIVATE) |
| `SYS_MUNMAP` | 11 | Libérer plage |
| `SYS_MREMAP` | 25 | Redimensionner (évite copie) |
| `SYS_MPROTECT` | 10 | Modifier protections |

### 1.4 PhoenixSafe

`ExoAllocator` est stateless. Les pages `mmap` survivent à la bascule (appartiennent à l'espace d'adressage restauré depuis le SSR).

`on_pre_switch()` : rien.
`on_post_switch()` : pas d'état à restaurer — les allocations existantes sont valides.

### 1.5 Tests

```
exo_alloc_test::alloc_basic_sizes      PASS
exo_alloc_test::dealloc_correct        PASS
exo_alloc_test::realloc_grow           PASS
exo_alloc_test::alignment_respected    PASS
exo_alloc_test::concurrent_alloc_free  PASS
exo_alloc_test::no_libc_symbol         PASS  ← nm | grep malloc → vide
```

---

## 2. exo-net

### 2.1 Architecture

```
Ring3 App
    │  exo_net::TcpStream::connect("93.184.216.34:443")?
    ▼
exo-net (Ring3 client)
    │  IPC: NetRequest::Connect { addr, capability }
    │  → SpscRing → network_server (Ring1)
    ▼
network_server
    │  smoltcp: TcpSocket::connect()
    │  retourne: SocketHandle + CapToken
    ▼
ExoNet::Socket { cap: CapToken, handle: u32 }
```

### 2.2 API Ring3

```rust
// exo-net/src/lib.rs

pub struct TcpStream {
    cap:    CapToken,
    handle: SocketHandle,
}

impl TcpStream {
    pub fn connect(addr: SocketAddr) -> Result<Self, NetError> {
        let cap = process_get_net_capability()?; // vérif CapToken réseau
        let resp = ipc_net(NetRequest::Connect { addr, cap })?;
        Ok(TcpStream { cap: resp.socket_cap, handle: resp.handle })
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        let resp = ipc_net(NetRequest::Recv {
            handle: self.handle,
            cap: self.cap,
            max_len: buf.len(),
        })?;
        buf[..resp.data.len()].copy_from_slice(&resp.data);
        Ok(resp.data.len())
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, NetError> {
        ipc_net(NetRequest::Send {
            handle: self.handle,
            cap: self.cap,
            data: data.into(),
        }).map(|r| r.sent)
    }
}

// Marquage PhoenixSafe
impl PhoenixSafe for TcpStream {
    fn on_pre_switch(&mut self) -> Result<(), PhoenixError> {
        // Invalider le handle — le numéro de séquence TCP est perdu
        self.handle = SocketHandle::INVALID;
        Ok(())
    }
    fn on_post_switch(&mut self) -> Result<(), PhoenixError> {
        // Si socket marquée persistent : tenter reconnexion
        if self.persist_addr.is_some() {
            *self = TcpStream::connect(self.persist_addr.unwrap())?;
        }
        Ok(())
    }
}
```

### 2.3 Dépendances Requises

Zéro dépendance directe sur `std::net` ou `libc`. Tout via IPC network_server.

---

## 3. exo-crypto

### 3.1 Principe

Les clés privées **ne quittent jamais** `crypto_server`. Les opérations cryptographiques se font par IPC. Les résultats (ciphertexts, signatures) reviennent en clair.

```
Ring3 App
    │  exo_crypto::sign(data, key_handle)?
    ▼
exo-crypto (Ring3 client)
    │  IPC: CryptoRequest::Sign { data, key_handle, cap }
    ▼
crypto_server (Ring1)
    │  rustcrypto-elliptic-curves: sign(data, private_key)
    │  Clé privée reste en mémoire Ring1
    │  retourne: signature
    ▼
exo-crypto: signature bytes
```

### 3.2 API Ring3

```rust
pub struct KeyHandle(u32);  // Opaque — référence clé dans crypto_server

pub fn generate_key(algo: KeyAlgo) -> Result<KeyHandle, CryptoError> {
    ipc_crypto(CryptoRequest::GenerateKey { algo })
        .map(|r| KeyHandle(r.handle))
}

pub fn sign(data: &[u8], key: KeyHandle, algo: SignAlgo)
    -> Result<Vec<u8>, CryptoError>
{
    ipc_crypto(CryptoRequest::Sign { data: data.into(), key: key.0, algo })
        .map(|r| r.signature)
}

pub fn verify(data: &[u8], sig: &[u8], pubkey: &[u8], algo: SignAlgo)
    -> Result<bool, CryptoError>
{
    // La vérification peut se faire en Ring3 — clé publique seulement
    match algo {
        SignAlgo::Ed25519 => ed25519_dalek::verify(data, sig, pubkey),
        SignAlgo::EcdsaP256 => p256::verify(data, sig, pubkey),
    }
}

pub fn encrypt_aead(data: &[u8], key: KeyHandle, algo: AeadAlgo)
    -> Result<(Vec<u8>, [u8; 12]), CryptoError>  // (ciphertext, nonce)
{
    ipc_crypto(CryptoRequest::Encrypt { data: data.into(), key: key.0, algo })
        .map(|r| (r.ciphertext, r.nonce))
}
```

### 3.3 PhoenixSafe

```rust
impl PhoenixSafe for ExoCryptoContext {
    fn on_pre_switch(&mut self) -> Result<(), PhoenixError> {
        // Évincer le cache local de clés publiques
        self.pubkey_cache.clear();
        Ok(())
    }
    fn on_post_switch(&mut self) -> Result<(), PhoenixError> {
        // Les KeyHandles restent valides — crypto_server restaure ses clés depuis ExoFS
        // Rien d'autre à faire
        Ok(())
    }
}
```

---

## 4. exo-fs

### 4.1 Primitives Natives ExoFS

| Primitive | Description | Avantage vs POSIX |
|---|---|---|
| `blob_id()` | Identifiant content-addressed BLAKE3 | Déduplication automatique |
| `epoch_commit()` | Commit atomique d'un ensemble de modifications | Snapshots O(1) |
| `relation_create()` | Lien sémantique typé entre objets | Graphe, pas juste hiérarchie |
| `snapshot_create()` | Snapshot instantané d'un répertoire | Rollback garanti |
| `object_move()` | Déplacement O(1) quelle que soit la localisation | Pas de copie cross-partition |

### 4.2 API Native Ring3

```rust
// exo-fs/src/lib.rs

pub struct ExoFile {
    cap:    CapToken,
    handle: ObjectId,
    cursor: u64,  // Pour compatibilité POSIX par-dessus
}

impl ExoFile {
    pub fn open(path: &str, rights: FsRights) -> Result<Self, FsError> {
        ipc_vfs(VfsRequest::Open { path: path.into(), rights })
            .map(|r| ExoFile { cap: r.cap, handle: r.object_id, cursor: 0 })
    }

    /// Lecture à offset explicite (style ExoFS — pas de curseur implicite)
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        ipc_vfs(VfsRequest::ReadAt {
            handle: self.handle, cap: self.cap, offset, len: buf.len()
        }).map(|r| { buf[..r.data.len()].copy_from_slice(&r.data); r.data.len() })
    }

    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        ipc_vfs(VfsRequest::WriteAt {
            handle: self.handle, cap: self.cap, offset, data: data.into()
        }).map(|r| r.written)
    }

    /// Snapshot de l'état courant
    pub fn snapshot(&self) -> Result<EpochId, FsError> {
        ipc_vfs(VfsRequest::SnapshotCreate { handle: self.handle, cap: self.cap })
            .map(|r| r.epoch_id)
    }

    /// Hash content-addressé BLAKE3
    pub fn content_hash(&self) -> Result<[u8; 32], FsError> {
        ipc_vfs(VfsRequest::GetContentHash { handle: self.handle })
            .map(|r| r.hash)
    }
}

/// Relation typée entre deux objets ExoFS
pub fn create_relation(from: ObjectId, to: ObjectId,
                        kind: RelationType, cap: CapToken)
    -> Result<RelationHandle, FsError>
{
    ipc_vfs(VfsRequest::RelationCreate { from, to, kind, cap })
        .map(|r| r.rel_handle)
}
```

### 4.3 Couche Compatibilité POSIX (par-dessus exo-fs natif)

```rust
// exo-fs/src/posix.rs

pub struct File(ExoFile);

impl File {
    pub fn open_posix(path: &str, flags: OpenFlags) -> Result<Self, FsError> {
        ExoFile::open(path, flags.to_exo_rights()).map(File)
    }
}

impl std::io::Read for File {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read_at(self.0.cursor, buf)
            .map_err(|e| std::io::Error::from(e))?;
        self.0.cursor += n as u64;
        Ok(n)
    }
}

impl std::io::Write for File {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let n = self.0.write_at(self.0.cursor, data)
            .map_err(|e| std::io::Error::from(e))?;
        self.0.cursor += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        ipc_vfs(VfsRequest::Sync { handle: self.0.handle })
            .map_err(|e| std::io::Error::from(e))
    }
}
```

### 4.4 PhoenixSafe

```rust
impl PhoenixSafe for ExoFile {
    fn on_pre_switch(&mut self) -> Result<(), PhoenixError> {
        // Flush write cache + s'assurer que l'epoch est commitée
        ipc_vfs(VfsRequest::Sync { handle: self.handle }).ok();
        Ok(())
    }
    fn on_post_switch(&mut self) -> Result<(), PhoenixError> {
        // Rouvrir le handle (l'ObjectId est stable, le CapToken survit via SSR)
        // Vérifier que le handle est toujours valide
        if ipc_vfs(VfsRequest::Stat { handle: self.handle }).is_err() {
            // Tenter de rouvrir par chemin si disponible
            if let Some(path) = &self.original_path {
                *self = ExoFile::open(path, self.original_rights)?;
            } else {
                return Err(PhoenixError::FileHandleLost);
            }
        }
        Ok(())
    }
}
```

---

## 5. exo-runtime

### 5.1 Rôle

Exécuteur `async/await` `no_std` basé sur le scheduler ExoOS. Remplace tokio. Zéro busy-loop.

### 5.2 Architecture

```rust
// libs/exo-runtime/src/lib.rs

pub struct ExoExecutor {
    tasks:  VecDeque<Task>,
    timers: BinaryHeap<TimerEntry>,
    wakers: BTreeMap<TaskId, Waker>,
}

impl ExoExecutor {
    pub fn new() -> Self { Self::default() }

    pub fn spawn<F: Future<Output=()> + 'static>(&mut self, fut: F) -> TaskId {
        let id = TaskId::next();
        self.tasks.push_back(Task::new(id, Box::pin(fut)));
        id
    }

    pub fn run(&mut self) -> ! {
        loop {
            // 1. Timers expirés → wake
            let now = exo_ktime_now();
            while let Some(entry) = self.timers.peek() {
                if entry.deadline > now { break; }
                let entry = self.timers.pop().unwrap();
                if let Some(waker) = self.wakers.remove(&entry.task_id) {
                    waker.wake();
                }
            }

            // 2. Poll les tâches prêtes
            let mut ran = false;
            for _ in 0..self.tasks.len() {
                let mut task = self.tasks.pop_front().unwrap();
                match task.poll() {
                    Poll::Ready(()) => { ran = true; }
                    Poll::Pending => { self.tasks.push_back(task); }
                }
            }

            // 3. Yield ou sleep — jamais de busy-loop
            if !ran {
                match self.timers.peek() {
                    None => sys_sched_yield(),
                    Some(next) => {
                        let wait = next.deadline.saturating_sub(exo_ktime_now());
                        sys_nanosleep(wait);
                    }
                }
            }
        }
    }
}

/// Future sleep — non-bloquante
pub async fn sleep(duration: Duration) {
    Sleep { deadline: exo_ktime_now() + duration.as_nanos() as u64 }.await
}
```

### 5.3 Usage Typique

```rust
fn main() {
    let mut exec = exo_runtime::ExoExecutor::new();

    exec.spawn(async {
        let mut stream = exo_net::TcpStream::connect("93.184.216.34:80".parse().unwrap())
            .await.unwrap();
        stream.write(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        println!("{}", core::str::from_utf8(&buf[..n]).unwrap());
    });

    exec.run()
}
```

### 5.4 PhoenixSafe

```rust
impl PhoenixSafe for ExoExecutor {
    fn on_pre_switch(&mut self) -> Result<(), PhoenixError> {
        // Compléter ou abandonner les futures en vol
        // Les timers sont perdus — ils seront réarmés post-switch si nécessaire
        self.timers.clear();
        self.wakers.clear();
        Ok(())
    }
    fn on_post_switch(&mut self) -> Result<(), PhoenixError> {
        // Relancer l'executor — les tâches persistentes se réenregistrent elles-mêmes
        Ok(())
    }
}
```

---

## 6. Dépendances Communes — Règles

| Règle | Détail |
|---|---|
| Zéro `libc` dans exo-alloc | Vérifiable : `nm exo-alloc.rlib | grep -E 'malloc|free|sbrk'` → vide |
| Zéro `tokio` | `cargo deny check` → violation si présent |
| Zéro `std::net` | Tout via IPC network_server |
| `exo-alloc` est le seul `#[global_allocator]` | Pas de jemalloc, pas de system alloc |
| Toutes les crates implémentent `PhoenixSafe` | Trait obligatoire pour crates à état |

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-EXO-CRATES-STRATA.md*
