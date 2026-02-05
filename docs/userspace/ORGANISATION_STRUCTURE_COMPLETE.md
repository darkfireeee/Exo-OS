# Organisation Structure Complète - Userspace & Libs

> **Date** : 2026-02-05  
> **Objectif** : Structure complète des modules avec tous fichiers/dossiers avant implémentation  
> **Statut** : 🟡 En attente validation

---

## 📊 État Actuel

### Dépôts externes clonés
```
/workspaces/Exo-OS/external_sources/
├── pqclean/         # Crypto post-quantique (C)
├── mimalloc/        # Allocateur Microsoft (C)
├── uutils/          # CoreUtils Rust
├── tracing/         # Logging ecosystem
├── metrics/         # Metrics ecosystem
├── config-rs/       # Configuration management
├── criterion/       # Benchmarking
├── proptest/        # Property-based testing
└── mdbook/          # Documentation
```

### Structure libs/ existante
```
/workspaces/Exo-OS/libs/
├── ai/              # IA modules (vide)
├── exo_crypto/      # 🟢 EXISTANT : chacha20, dilithium, kyber
├── exo_ipc/         # 🟢 EXISTANT : message, channel
├── exo_std/         # 🟢 EXISTANT : process, io, sync, thread, ipc, time, security
├── exo_types/       # 🟢 EXISTANT (à compléter)
├── fs/              # 🟡 PARTIEL : dir, file, metadata
├── io/              # 🟡 PARTIEL
├── ipc/             # 🟡 PARTIEL : async_channel, channel, shared_mem
├── net/             # 🟡 PARTIEL
├── process/         # 🟡 PARTIEL : command, exit, spawn
├── security/        # 🟡 PARTIEL
├── sync/            # 🟡 PARTIEL : atomic, mutex, rwlock
├── thread/          # 🟡 PARTIEL
├── time/            # 🟡 PARTIEL
└── lib.rs           # 🟢 Root module
```

### Structure userland/ existante
```
/workspaces/Exo-OS/userland/
├── ai_assistant/    # 🟡 Structure vide
├── ai_core/         # 🟡 Structure vide
├── ai_learn/        # 🟡 Structure vide
├── ai_res/          # 🟡 Structure vide
├── ai_sec/          # 🟡 Structure vide
├── ai_user/         # 🟡 Structure vide
├── driver_manager/  # 🟡 Structure vide
├── drivers/net/     # 🟡 Structure vide
├── fs_service/      # 🟡 Structure vide (vfs/, ext4/, fat32/)
├── init/            # 🟡 Structure vide
├── lib/musl/        # 🟢 Musl libc (externe)
├── net_service/     # 🟡 Structure vide (ip/, tcp/, udp/, ethernet/, core/, wireguard/)
├── services/        # 🟡 Structure vide
├── shell/           # 🟡 Structure vide
└── window_manager/  # 🟡 Structure vide
```

---

## 🎯 Structure Proposée - libs/ (Priorité 0-1)

### 1. exo_std (EXTENSION)
**Dossier** : `/workspaces/Exo-OS/libs/exo_std/`
```
exo_std/
├── Cargo.toml           # 🆕 Workspace member
├── README.md            # 🆕 Architecture overview
├── CHANGELOG.md         # 🆕 Version tracking
├── src/
│   ├── lib.rs           # 🟢 EXISTANT
│   ├── process.rs       # 🟢 EXISTANT
│   ├── io.rs            # 🟢 EXISTANT
│   ├── sync.rs          # 🟢 EXISTANT
│   ├── thread.rs        # 🟢 EXISTANT
│   ├── ipc.rs           # 🟢 EXISTANT
│   ├── time.rs          # 🟢 EXISTANT
│   ├── security.rs      # 🟢 EXISTANT
│   ├── collections/     # 🆕 NOUVEAU MODULE
│   │   ├── mod.rs
│   │   ├── ring_buffer.rs      # RingBuffer<T> lock-free
│   │   ├── bounded_vec.rs      # BoundedVec<T, N>
│   │   ├── intrusive_list.rs   # IntrusiveList<T>
│   │   └── radix_tree.rs       # RadixTree<K, V>
│   ├── alloc/           # 🆕 NOUVEAU MODULE (mimalloc integration)
│   │   ├── mod.rs
│   │   ├── slab.rs             # SlabAllocator
│   │   ├── bump.rs             # BumpAllocator
│   │   └── mimalloc.rs         # GlobalAlloc wrapper
│   └── error/           # 🆕 NOUVEAU MODULE
│       ├── mod.rs
│       └── errno.rs            # Error codes mapping
├── tests/
│   ├── collections_test.rs     # 🆕 Property tests
│   ├── alloc_test.rs           # 🆕 Allocator tests
│   └── integration_test.rs     # 🆕 Integration tests
├── benches/
│   ├── ring_buffer.rs          # 🆕 Criterion benchmarks
│   ├── allocator.rs            # 🆕 vs jemalloc/system
│   └── radix_tree.rs           # 🆕 Lookup benchmarks
├── examples/
│   ├── ring_buffer_usage.rs    # 🆕
│   ├── custom_allocator.rs     # 🆕
│   └── intrusive_list.rs       # 🆕
└── vendor/             # 🆕 Code externe adapté
    └── mimalloc/       # Source C de mimalloc (seuls .c/.h nécessaires)
        ├── include/
        └── src/
```

**Actions** :
- [x] Cloner mimalloc
- [ ] Extraire sources C minimal (src/ + include/) vers vendor/mimalloc/
- [ ] Créer FFI bindings Rust (src/alloc/mimalloc.rs)
- [ ] Implémenter collections (ring_buffer, bounded_vec, intrusive_list, radix_tree)
- [ ] Écrire tests (proptest, fuzzing)
- [ ] Benchmarks vs std::collections, jemalloc

---

### 2. exo_crypto (EXTENSION)
**Dossier** : `/workspaces/Exo-OS/libs/exo_crypto/`
```
exo_crypto/
├── Cargo.toml
├── README.md            # 🆕 NIST standards reference
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs           # 🟢 EXISTANT
│   ├── chacha20.rs      # 🟢 EXISTANT
│   ├── dilithium.rs     # 🟢 EXISTANT (à compléter)
│   ├── kyber.rs         # 🟢 EXISTANT (à compléter)
│   ├── hash/            # 🆕 NOUVEAU
│   │   ├── mod.rs
│   │   ├── blake3.rs           # BLAKE3 hashing
│   │   └── sha3.rs             # SHA3 (Keccak)
│   ├── constant_time/   # 🆕 NOUVEAU
│   │   ├── mod.rs
│   │   ├── cmp.rs              # Timing-attack resistant compare
│   │   └── memcpy.rs           # Constant-time memcpy
│   └── simd/            # 🆕 NOUVEAU
│       ├── mod.rs
│       ├── avx2.rs             # AVX2 optimizations
│       └── runtime_detect.rs   # CPU feature detection
├── vendor/             # 🆕 PQClean sources
│   └── pqclean/
│       ├── crypto_kem/kyber/    # Only Kyber sources
│       └── crypto_sign/dilithium/ # Only Dilithium sources
├── tests/
│   ├── kyber_test.rs           # 🆕 KAT vectors
│   ├── dilithium_test.rs       # 🆕 KAT vectors
│   ├── hash_test.rs            # 🆕
│   └── timing_test.rs          # 🆕 Constant-time verification
├── benches/
│   ├── kyber_bench.rs          # 🆕 Keygen/Encaps/Decaps
│   ├── dilithium_bench.rs      # 🆕 Sign/Verify
│   └── simd_bench.rs           # 🆕 AVX2 vs scalar
└── examples/
    ├── key_exchange.rs         # 🆕 Kyber KEM example
    └── signature.rs            # 🆕 Dilithium signing
```

**Actions** :
- [x] Cloner pqclean
- [ ] Extraire Kyber + Dilithium sources C vers vendor/
- [ ] Créer build.rs pour compiler C (cc crate)
- [ ] Wrapper Rust safe au-dessus FFI
- [ ] Implémenter BLAKE3 (crate externe ou custom)
- [ ] Constant-time utilities
- [ ] SIMD optimizations avec runtime detection
- [ ] Tests KAT (Known Answer Tests) NIST
- [ ] Benchmarks avec perf counters

---

### 3. exo_types (EXTENSION)
**Dossier** : `/workspaces/Exo-OS/libs/exo_types/`
```
exo_types/
├── Cargo.toml
├── README.md            # 🆕 Type safety guide
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs           # 🟢 EXISTANT (à étendre)
│   ├── pid.rs           # 🆕 Process ID newtype
│   ├── fd.rs            # 🆕 File Descriptor RAII
│   ├── errno.rs         # 🆕 Error codes (POSIX + Exo custom)
│   ├── time.rs          # 🆕 Timestamp (Monotonic/Realtime)
│   ├── signal.rs        # 🆕 Signal numbers typesafe
│   ├── uid_gid.rs       # 🆕 User/Group IDs
│   └── syscall.rs       # 🆕 Syscall numbers
├── tests/
│   ├── newtypes_test.rs        # 🆕 Type safety tests
│   └── serde_test.rs           # 🆕 Serialization tests
├── benches/
│   └── conversion_bench.rs     # 🆕 Zero-cost checks
└── examples/
    ├── fd_raii.rs              # 🆕 Safe file descriptor
    └── errno_handling.rs       # 🆕 Error handling patterns
```

**Actions** :
- [ ] Implémenter newtypes (Pid, Fd, Uid, Gid)
- [ ] RAII wrappers (Drop impl pour Fd)
- [ ] Errno mapping complet (POSIX + Exo custom codes)
- [ ] Serde support (bytemuck pour zero-copy)
- [ ] Property-based tests (conversions)
- [ ] Documentation des codes errno avec table

---

### 4. exo_allocator (NOUVEAU)
**Dossier** : `/workspaces/Exo-OS/libs/exo_allocator/`
```
exo_allocator/
├── Cargo.toml           # 🆕 NEW CRATE
├── README.md            # 🆕 Allocator comparison, use-cases
├── CHANGELOG.md         # 🆕
├── build.rs             # 🆕 Compile mimalloc C sources
├── src/
│   ├── lib.rs
│   ├── slab.rs                 # Slab allocator for fixed-size
│   ├── bump.rs                 # Bump/Arena allocator
│   ├── mimalloc.rs             # Mimalloc GlobalAlloc wrapper
│   ├── telemetry.rs            # Allocation tracking hooks
│   └── oom.rs                  # OOM handler
├── vendor/             # 🆕 mimalloc sources minimales
│   └── mimalloc/
│       ├── include/mimalloc.h
│       └── src/        # Uniquement .c nécessaires
├── tests/
│   ├── slab_test.rs            # 🆕
│   ├── bump_test.rs            # 🆕
│   └── oom_test.rs             # 🆕 OOM simulation
├── benches/
│   ├── allocator_bench.rs      # 🆕 vs jemalloc/tcmalloc/system
│   └── fragmentation_bench.rs  # 🆕
└── examples/
    ├── slab_usage.rs           # 🆕 Object pool pattern
    └── bump_parser.rs          # 🆕 Temporary data arena
```

**Actions** :
- [x] Cloner mimalloc
- [ ] Extraire sources C minimal
- [ ] build.rs avec cc crate pour compilation
- [ ] GlobalAlloc trait impl pour mimalloc
- [ ] Slab allocator pour objects taille fixe (file descriptors)
- [ ] Bump allocator pour arenas
- [ ] Telemetry hooks (allocation tracking)
- [ ] OOM handler custom
- [ ] Benchmarks exhaustifs vs alternatives

---

### 5. exo_text (NOUVEAU)
**Dossier** : `/workspaces/Exo-OS/libs/exo_text/`
```
exo_text/
├── Cargo.toml           # 🆕 NEW CRATE
├── README.md            # 🆕
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs
│   ├── json/
│   │   ├── mod.rs
│   │   ├── parser.rs           # Streaming JSON parser (simd-json)
│   │   └── serializer.rs
│   ├── toml/
│   │   ├── mod.rs
│   │   └── de.rs               # TOML deserializer
│   ├── markdown/
│   │   ├── mod.rs
│   │   ├── parser.rs
│   │   └── renderer.rs         # HTML + Terminal output
│   └── diff/
│       ├── mod.rs
│       └── unified.rs          # Unified diff generator
├── tests/
│   ├── json_test.rs            # 🆕 Fuzzing with proptest
│   ├── toml_test.rs            # 🆕
│   └── markdown_test.rs        # 🆕
├── benches/
│   ├── json_bench.rs           # 🆕 SIMD vs scalar
│   └── streaming_bench.rs      # 🆕 Large files
└── examples/
    ├── json_streaming.rs       # 🆕
    └── markdown_render.rs      # 🆕
```

**Actions** :
- [ ] Intégrer simd-json ou custom parser
- [ ] TOML parser (serde-based)
- [ ] Markdown parser/renderer (pulldown-cmark ou custom)
- [ ] Unified diff generator
- [ ] Streaming parsers pour gros fichiers
- [ ] Fuzzing des parsers (cargo-fuzz)
- [ ] Benchmarks SIMD vs scalar

---

### 6. exo_config (NOUVEAU)
**Dossier** : `/workspaces/Exo-OS/libs/exo_config/`
```
exo_config/
├── Cargo.toml           # 🆕 NEW CRATE
├── README.md            # 🆕 Hierarchy schema
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs
│   ├── loader.rs               # Multi-source config loading
│   ├── validator.rs            # Schema validation (schemars)
│   ├── watcher.rs              # Hot-reload (inotify/kqueue)
│   ├── merger.rs               # Hierarchical merge (user > system > default)
│   └── migrate.rs              # Auto-migration old configs
├── tests/
│   ├── hierarchy_test.rs       # 🆕 Merge logic
│   ├── validation_test.rs      # 🆕
│   └── hotreload_test.rs       # 🆕
├── benches/
│   └── parse_bench.rs          # 🆕 Cache memoization
└── examples/
    ├── basic_config.rs         # 🆕
    └── hot_reload.rs           # 🆕
```

**Actions** :
- [x] Cloner config-rs (référence)
- [ ] Adapter pour Exo-OS paths (/etc/exo-os/, ~/.config/exo-os/)
- [ ] File watcher (inotify Linux, kqueue BSD)
- [ ] Schema validation avec schemars
- [ ] Hierarchy merge logic
- [ ] Migration automatique anciennes versions
- [ ] Cache parsing (memoization)

---

### 7. exo_logger (NOUVEAU)
**Dossier** : `/workspaces/Exo-OS/libs/exo_logger/`
```
exo_logger/
├── Cargo.toml           # 🆕 NEW CRATE
├── README.md            # 🆕 Filter guide, span examples
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs
│   ├── collector.rs            # Multi-source log collector
│   ├── formatter/
│   │   ├── mod.rs
│   │   ├── json.rs             # JSON Lines format
│   │   └── pretty.rs           # Human-readable
│   ├── sink/
│   │   ├── mod.rs
│   │   ├── file.rs             # File sink + rotation
│   │   └── ipc.rs              # IPC to syslog-ng
│   ├── filter.rs               # Dynamic runtime filtering
│   └── span.rs                 # Span-based contexts
├── tests/
│   ├── filtering_test.rs       # 🆕
│   └── rotation_test.rs        # 🆕
├── benches/
│   ├── throughput_bench.rs     # 🆕 Async buffering
│   └── compression_bench.rs    # 🆕 zstd compression
└── examples/
    ├── basic_logging.rs        # 🆕
    └── span_context.rs         # 🆕
```

**Actions** :
- [x] Cloner tracing (référence)
- [ ] Intégrer tracing-subscriber
- [ ] JSON Lines formatter
- [ ] File sink avec rotation (10MB max)
- [ ] Compression logs anciens (zstd)
- [ ] Filtrage dynamique runtime
- [ ] IPC backend vers syslog-ng
- [ ] Async buffering (tokio channels)
- [ ] Sampling (log 1/N pour high-volume)

---

### 8. exo_metrics (NOUVEAU)
**Dossier** : `/workspaces/Exo-OS/libs/exo_metrics/`
```
exo_metrics/
├── Cargo.toml           # 🆕 NEW CRATE
├── README.md            # 🆕 Available metrics, Grafana setup
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs
│   ├── registry.rs             # Metrics registration
│   ├── exporters/
│   │   ├── mod.rs
│   │   └── prometheus.rs       # Prometheus format
│   ├── aggregator.rs           # Histogram (P50/P95/P99)
│   ├── timer.rs                # Latency measurement
│   └── system.rs               # System metrics (CPU, mem, I/O)
├── tests/
│   ├── histogram_test.rs       # 🆕
│   └── export_test.rs          # 🆕
├── benches/
│   └── atomic_bench.rs         # 🆕 Lock-free counters
└── examples/
    ├── http_endpoint.rs        # 🆕 /metrics endpoint
    └── dashboard.rs            # 🆕 Simple HTML dashboard
```

**Actions** :
- [x] Cloner metrics-rs (référence)
- [ ] Registry avec thread-local counters
- [ ] Prometheus exporter
- [ ] Histogrammes (HdrHistogram)
- [ ] Timers pour latences
- [ ] Métriques système (CPU, mem, I/O)
- [ ] HTTP endpoint `/metrics`
- [ ] Dashboard web minimal (HTML + fetch API)
- [ ] Lock-free atomic metrics

---

### 9. exo_ipc_types (EXTENSION exo_ipc)
**Dossier** : `/workspaces/Exo-OS/libs/exo_ipc/`
```
exo_ipc/
├── Cargo.toml           # 🟢 EXISTANT
├── README.md            # 🆕 Protocol spec
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs
│   ├── message.rs       # 🟢 EXISTANT (à étendre)
│   ├── channel.rs       # 🟢 EXISTANT (à étendre)
│   ├── types/           # 🆕 NOUVEAU
│   │   ├── mod.rs
│   │   ├── channel_id.rs       # ChannelId newtype
│   │   └── endpoint.rs         # Endpoint addressing
│   ├── serializer.rs    # 🆕 Bincode wrapper
│   ├── protocol.rs      # 🆕 Handshake protocol
│   └── checksum.rs      # 🆕 CRC32C (sse4.2)
├── tests/
│   ├── roundtrip_test.rs       # 🆕
│   └── fuzz_test.rs            # 🆕 cargo-fuzz
├── benches/
│   ├── serialize_bench.rs      # 🆕
│   └── checksum_bench.rs       # 🆕
└── examples/
    ├── client_server.rs        # 🆕
    └── streaming.rs            # 🆕
```

**Actions** :
- [ ] Définir protocol wire format (header + payload + checksum)
- [ ] Bincode serialization
- [ ] CRC32C checksum (sse4.2)
- [ ] Handshake protocol
- [ ] Roundtrip tests
- [ ] Fuzzing deserializer
- [ ] Benchmarks serialization

---

### 10. exo_service_registry (NOUVEAU)
**Dossier** : `/workspaces/Exo-OS/libs/exo_service_registry/`
```
exo_service_registry/
├── Cargo.toml           # 🆕 NEW CRATE
├── README.md            # 🆕 Service conventions
├── CHANGELOG.md         # 🆕
├── src/
│   ├── lib.rs
│   ├── registry.rs             # HashMap<ServiceName, Endpoint>
│   ├── discovery.rs            # Client lookup
│   ├── health.rs               # Heartbeat ping/pong
│   └── storage.rs              # Persistent storage (sqlite)
├── tests/
│   ├── registry_test.rs        # 🆕
│   └── discovery_test.rs       # 🆕
├── benches/
│   └── lookup_bench.rs         # 🆕 LRU cache
└── examples/
    ├── register_service.rs     # 🆕
    └── find_service.rs         # 🆕
```

**Actions** :
- [ ] Registry HashMap avec RwLock
- [ ] Discovery client
- [ ] Health checker (ping/pong heartbeat)
- [ ] Persistent storage (sqlite ou TOML file)
- [ ] Cache lookup (LRU)
- [ ] Bloom filter pour fast negative lookup
- [ ] Integration avec init system

---

## 🎯 Structure Proposée - userland/ (Priorité 1-2)

### 11. exo_coreutils
**Dossier** : `/workspaces/Exo-OS/userland/coreutils/`
```
coreutils/
├── Cargo.toml           # 🆕 Workspace avec sous-crates
├── README.md            # 🆕
├── src/
│   └── lib.rs          # Common utilities
├── cat/
│   ├── Cargo.toml
│   └── src/main.rs     # cat command
├── echo/
│   ├── Cargo.toml
│   └── src/main.rs     # echo command
├── ls/
│   ├── Cargo.toml
│   └── src/main.rs     # ls command
├── cp/
│   ├── Cargo.toml
│   └── src/main.rs     # cp command
├── mv/
│   ├── Cargo.toml
│   └── src/main.rs     # mv command
├── rm/
│   ├── Cargo.toml
│   └── src/main.rs     # rm command
├── mkdir/
│   ├── Cargo.toml
│   └── src/main.rs     # mkdir command
├── ps/
│   ├── Cargo.toml
│   └── src/main.rs     # ps command (read /proc)
├── vendor/             # 🆕 uutils sources adaptées
│   └── uutils/         # Uniquement sources Rust nécessaires
└── man/                # 🆕 Man pages
    ├── cat.1
    ├── ls.1
    └── ...
```

**Actions** :
- [x] Cloner uutils
- [ ] Extraire sources pertinentes (cat, ls, cp, mv, rm, mkdir, ps)
- [ ] Adapter syscalls (remplacer libc par exo_std::fs)
- [ ] Error handling unifié (exo_types::errno)
- [ ] Buffering I/O (4KB chunks)
- [ ] Man pages complètes
- [ ] Tests unitaires + integration

---

### 12. services/config_manager
**Dossier** : `/workspaces/Exo-OS/userland/services/config_manager/`
```
config_manager/
├── Cargo.toml           # 🆕
├── README.md            # 🆕
├── src/
│   ├── main.rs                 # Service daemon
│   ├── loader.rs               # Use exo_config
│   └── watcher.rs              # File watching
├── tests/
│   └── integration_test.rs     # 🆕
└── configs/            # 🆕 Example configs
    ├── system.toml
    └── desktop.toml
```

---

### 13. services/logger
**Dossier** : `/workspaces/Exo-OS/userland/services/logger/`
```
logger/
├── Cargo.toml           # 🆕
├── README.md            # 🆕
├── src/
│   ├── main.rs                 # Logging daemon
│   ├── collector.rs            # Use exo_logger
│   └── indexer.rs              # Tantivy indexing
├── tests/
│   └── integration_test.rs     # 🆕
└── scripts/
    └── query_logs.sh           # 🆕 Log query helper
```

---

### 14. services/metrics
**Dossier** : `/workspaces/Exo-OS/userland/services/metrics/`
```
metrics/
├── Cargo.toml           # 🆕
├── README.md            # 🆕
├── src/
│   ├── main.rs                 # Metrics daemon
│   ├── collector.rs            # Use exo_metrics
│   └── http.rs                 # HTTP /metrics endpoint
├── tests/
│   └── integration_test.rs     # 🆕
└── dashboard/          # 🆕 Web dashboard
    ├── index.html
    └── app.js
```

---

### 15. services/registry
**Dossier** : `/workspaces/Exo-OS/userland/services/registry/`
```
registry/
├── Cargo.toml           # 🆕
├── README.md            # 🆕
├── src/
│   ├── main.rs                 # Registry daemon
│   └── server.rs               # Use exo_service_registry
├── tests/
│   └── integration_test.rs     # 🆕
└── data/
    └── services.db             # 🆕 SQLite database
```

---

## 🔧 Plan d'Exécution Détaillé

### Phase A : Setup Infrastructure (Jour 1)
```bash
cd /workspaces/Exo-OS

# 1. Créer Cargo workspace pour libs
cat > libs/Cargo.toml << 'EOF'
[workspace]
members = [
    "exo_std",
    "exo_crypto",
    "exo_types",
    "exo_ipc",
    "exo_allocator",
    "exo_text",
    "exo_config",
    "exo_logger",
    "exo_metrics",
    "exo_service_registry",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Exo-OS Team"]
license = "MIT OR Apache-2.0"

[workspace.dependencies]
serde = { version = "1.0", default-features = false, features = ["derive"] }
bincode = "1.3"
bytemuck = "1.14"
EOF

# 2. Créer Cargo workspace pour userland
cat > userland/Cargo.toml << 'EOF'
[workspace]
members = [
    "coreutils",
    "services/config_manager",
    "services/logger",
    "services/metrics",
    "services/registry",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Exo-OS Team"]

[workspace.dependencies]
exo_std = { path = "../libs/exo_std" }
exo_crypto = { path = "../libs/exo_crypto" }
exo_types = { path = "../libs/exo_types" }
exo_ipc = { path = "../libs/exo_ipc" }
exo_config = { path = "../libs/exo_config" }
exo_logger = { path = "../libs/exo_logger" }
exo_metrics = { path = "../libs/exo_metrics" }
EOF
```

### Phase B : Extraction Sources Externes (Jour 2-3)

#### B.1 : mimalloc
```bash
# Extraire sources C minimales
mkdir -p /workspaces/Exo-OS/libs/exo_allocator/vendor/mimalloc
cd /workspaces/Exo-OS/external_sources/mimalloc

# Copier headers
cp -r include /workspaces/Exo-OS/libs/exo_allocator/vendor/mimalloc/

# Copier sources C nécessaires (uniquement .c/.h core)
mkdir -p /workspaces/Exo-OS/libs/exo_allocator/vendor/mimalloc/src
cp src/alloc.c \
   src/alloc-aligned.c \
   src/heap.c \
   src/options.c \
   src/page.c \
   src/segment.c \
   src/stats.c \
   /workspaces/Exo-OS/libs/exo_allocator/vendor/mimalloc/src/

# PAS de .git, .md, tests, benchmarks
```

#### B.2 : pqclean (Kyber + Dilithium)
```bash
# Extraire Kyber
mkdir -p /workspaces/Exo-OS/libs/exo_crypto/vendor/pqclean/crypto_kem/kyber
cd /workspaces/Exo-OS/external_sources/pqclean

# Kyber512 clean implementation (sans AVX2 d'abord)
cp -r crypto_kem/kyber512/clean/* \
   /workspaces/Exo-OS/libs/exo_crypto/vendor/pqclean/crypto_kem/kyber/

# Dilithium2 clean implementation
mkdir -p /workspaces/Exo-OS/libs/exo_crypto/vendor/pqclean/crypto_sign/dilithium
cp -r crypto_sign/dilithium2/clean/* \
   /workspaces/Exo-OS/libs/exo_crypto/vendor/pqclean/crypto_sign/dilithium/

# Cleanup (pas de .md, tests upstream)
find /workspaces/Exo-OS/libs/exo_crypto/vendor -name "*.md" -delete
find /workspaces/Exo-OS/libs/exo_crypto/vendor -name "test_*" -delete
```

#### B.3 : uutils (coreutils)
```bash
# Extraire crates nécessaires
mkdir -p /workspaces/Exo-OS/userland/coreutils/vendor/uutils
cd /workspaces/Exo-OS/external_sources/uutils/src

# Copier crates individuelles (cat, ls, cp, mv, rm, mkdir)
for cmd in cat ls cp mv rm mkdir; do
    cp -r uu${cmd} /workspaces/Exo-OS/userland/coreutils/vendor/uutils/
done

# Copier uucore (shared utilities)
cp -r uucore /workspaces/Exo-OS/userland/coreutils/vendor/uutils/

# Cleanup
find /workspaces/Exo-OS/userland/coreutils/vendor -name "*.md" -delete
find /workspaces/Exo-OS/userland/coreutils/vendor -name ".git*" -delete
```

#### B.4 : Autres crates (références uniquement)
```bash
# tracing, metrics, config-rs, criterion, proptest, mdbook
# → Ces crates seront utilisés via Cargo.toml dependencies
# → PAS de copie de sources, juste référence dans Cargo.toml
```

### Phase C : Création Structure Modules (Jour 4-5)

#### C.1 : exo_allocator (exemple complet)
```bash
cd /workspaces/Exo-OS/libs

# Créer crate
cargo new exo_allocator --lib

# Structure complète
mkdir -p exo_allocator/{src,tests,benches,examples,vendor}
touch exo_allocator/{README.md,CHANGELOG.md,build.rs}

# Cargo.toml
cat > exo_allocator/Cargo.toml << 'EOF'
[package]
name = "exo_allocator"
version = "0.1.0"
edition = "2021"
authors = ["Exo-OS Team"]
license = "MIT OR Apache-2.0"
description = "Custom allocators for Exo-OS (slab, bump, mimalloc)"

[dependencies]
exo_types = { path = "../exo_types" }

[dev-dependencies]
criterion = "0.5"
proptest = "1.0"

[build-dependencies]
cc = "1.0"

[[bench]]
name = "allocator_bench"
harness = false

[lib]
name = "exo_allocator"
path = "src/lib.rs"
EOF

# build.rs pour compiler mimalloc
cat > exo_allocator/build.rs << 'EOF'
use cc;

fn main() {
    cc::Build::new()
        .include("vendor/mimalloc/include")
        .files(&[
            "vendor/mimalloc/src/alloc.c",
            "vendor/mimalloc/src/alloc-aligned.c",
            "vendor/mimalloc/src/heap.c",
            "vendor/mimalloc/src/options.c",
            "vendor/mimalloc/src/page.c",
            "vendor/mimalloc/src/segment.c",
            "vendor/mimalloc/src/stats.c",
        ])
        .flag("-O3")
        .flag("-march=native")
        .compile("mimalloc");

    println!("cargo:rerun-if-changed=vendor/mimalloc");
}
EOF

# README.md template
cat > exo_allocator/README.md << 'EOF'
# exo_allocator

Custom memory allocators for Exo-OS.

## Features

- **SlabAllocator** : Fixed-size object pools (file descriptors, tasks)
- **BumpAllocator** : Arena allocator for temporary data (parsing)
- **MimallocWrapper** : GlobalAlloc impl using Microsoft mimalloc

## Benchmarks

```bash
cargo bench --package exo_allocator
```

## Usage

```rust
use exo_allocator::Slab;

let slab = Slab::new(64); // 64-byte objects
let ptr = slab.alloc();
slab.free(ptr);
```
EOF

# CHANGELOG.md
cat > exo_allocator/CHANGELOG.md << 'EOF'
# Changelog

## [Unreleased]

### Added
- Initial SlabAllocator implementation
- BumpAllocator for arenas
- Mimalloc FFI bindings
- GlobalAlloc trait impl
- Telemetry hooks
- OOM handler

### Benchmarks
- vs jemalloc, tcmalloc, system allocator
EOF
```

#### C.2 : Répéter pour tous les modules
```bash
# Pour chaque module dans la liste (exo_text, exo_config, exo_logger, etc.)
# → Même structure :
#   - Cargo.toml avec [dependencies], [dev-dependencies], [build-dependencies]
#   - README.md avec overview, features, usage, benchmarks
#   - CHANGELOG.md avec versioning
#   - src/ avec modules organisés
#   - tests/ avec tests unitaires + integration
#   - benches/ avec criterion benchmarks
#   - examples/ avec exemples d'usage
#   - vendor/ si sources C/externes nécessaires
```

### Phase D : Validation Structure (Jour 6)
```bash
# Vérifier workspace compilation
cd /workspaces/Exo-OS/libs
cargo check --workspace

cd /workspaces/Exo-OS/userland
cargo check --workspace

# Lister structure finale
find /workspaces/Exo-OS/libs -name "Cargo.toml" | head -20
find /workspaces/Exo-OS/userland -name "Cargo.toml" | head -20

# Générer tree documentation
tree -L 3 -I 'target|vendor' /workspaces/Exo-OS/libs > /tmp/libs_structure.txt
tree -L 3 -I 'target|vendor' /workspaces/Exo-OS/userland > /tmp/userland_structure.txt
```

---

## 📋 Checklist Golden Rules

### Pour CHAQUE module créé :

- [ ] **Structure complète** :
  - [ ] Cargo.toml avec metadata complètes
  - [ ] README.md avec overview, features, usage, benchmarks
  - [ ] CHANGELOG.md pour tracking versions
  - [ ] src/ avec modules organisés (mod.rs, sous-modules)
  - [ ] tests/ avec tests unitaires + integration
  - [ ] benches/ avec criterion benchmarks (si applicable)
  - [ ] examples/ avec au moins 2 exemples d'usage
  - [ ] vendor/ avec sources externes minimales (PAS de .git, .md superflus)

- [ ] **Cargo.toml qualité** :
  - [ ] `[package]` : name, version, edition, authors, license, description
  - [ ] `[dependencies]` : versions fixées, features explicites
  - [ ] `[dev-dependencies]` : criterion, proptest si applicable
  - [ ] `[build-dependencies]` : cc si compilation C
  - [ ] `[[bench]]` avec harness = false pour criterion
  - [ ] `[lib]` avec name et path explicites

- [ ] **Documentation** :
  - [ ] README.md avec badges (build status, coverage)
  - [ ] Architecture overview (diagrammes si complexe)
  - [ ] Exemples d'usage avec code snippets
  - [ ] Benchmarks comparatifs (vs alternatives)
  - [ ] Troubleshooting section
  - [ ] CHANGELOG.md avec format Keep a Changelog

- [ ] **Code organisation** :
  - [ ] src/lib.rs avec pub mod exports clairs
  - [ ] Modules séparés par responsabilité (1 fichier = 1 concept)
  - [ ] Doc comments (///) sur toutes fonctions publiques
  - [ ] Exemples dans doc comments (```rust)
  - [ ] Pas de code mort, pas de commented code

- [ ] **Tests** :
  - [ ] tests/ avec au moins 1 test unitaire par module
  - [ ] Property-based tests (proptest) pour parsers/crypto
  - [ ] Fuzzing setup (cargo-fuzz) si applicable
  - [ ] Coverage > 80% (cargo-tarpaulin)

- [ ] **Benchmarks** :
  - [ ] benches/ avec criterion
  - [ ] Baseline établi (commit results)
  - [ ] Comparaison vs alternatives (std, ecosystem crates)
  - [ ] Flamegraphs générés (perf + inferno)

- [ ] **Build** :
  - [ ] cargo build --release passing
  - [ ] cargo clippy --all-targets -- -D warnings passing
  - [ ] cargo fmt --all -- --check passing
  - [ ] cargo test --all-features passing
  - [ ] cargo bench --no-run passing (verify benchmarks compile)

- [ ] **Vendor cleanup** :
  - [ ] Uniquement sources nécessaires (.c, .h, .rs)
  - [ ] PAS de .git, .github, .md, LICENSE, tests upstream
  - [ ] Attribution claire dans README.md (license, authors)

---

## 🎯 Livrables Attendus

### 1. Structure libs/ complète
```
/workspaces/Exo-OS/libs/
├── Cargo.toml                   # 🆕 Workspace root
├── exo_std/                     # 🟢 EXISTANT + extensions
│   ├── Cargo.toml
│   ├── README.md
│   ├── CHANGELOG.md
│   ├── src/ (collections, alloc, error)
│   ├── tests/
│   ├── benches/
│   ├── examples/
│   └── vendor/mimalloc/
├── exo_crypto/                  # 🟢 EXISTANT + extensions
│   ├── Cargo.toml
│   ├── README.md
│   ├── CHANGELOG.md
│   ├── src/ (hash, constant_time, simd)
│   ├── tests/ (KAT vectors)
│   ├── benches/
│   ├── examples/
│   └── vendor/pqclean/
├── exo_types/                   # 🟢 EXISTANT + extensions
│   ├── src/ (pid, fd, errno, signal, time)
│   └── ...
├── exo_ipc/                     # 🟢 EXISTANT + extensions
│   ├── src/ (types, serializer, protocol, checksum)
│   └── ...
├── exo_allocator/               # 🆕 NOUVEAU
│   ├── Cargo.toml
│   ├── build.rs
│   ├── src/ (slab, bump, mimalloc, telemetry, oom)
│   ├── vendor/mimalloc/
│   └── ...
├── exo_text/                    # 🆕 NOUVEAU
│   ├── src/ (json, toml, markdown, diff)
│   └── ...
├── exo_config/                  # 🆕 NOUVEAU
│   ├── src/ (loader, validator, watcher, merger, migrate)
│   └── ...
├── exo_logger/                  # 🆕 NOUVEAU
│   ├── src/ (collector, formatter, sink, filter, span)
│   └── ...
├── exo_metrics/                 # 🆕 NOUVEAU
│   ├── src/ (registry, exporters, aggregator, timer, system)
│   └── ...
└── exo_service_registry/        # 🆕 NOUVEAU
    ├── src/ (registry, discovery, health, storage)
    └── ...
```

### 2. Structure userland/ complète
```
/workspaces/Exo-OS/userland/
├── Cargo.toml                   # 🆕 Workspace root
├── coreutils/                   # 🆕 NOUVEAU
│   ├── Cargo.toml
│   ├── cat/, echo/, ls/, cp/, mv/, rm/, mkdir/, ps/
│   ├── vendor/uutils/
│   └── man/
├── services/
│   ├── config_manager/          # 🆕 NOUVEAU
│   ├── logger/                  # 🆕 NOUVEAU
│   ├── metrics/                 # 🆕 NOUVEAU
│   └── registry/                # 🆕 NOUVEAU
└── [existing services unchanged]
```

### 3. Documentation générée
- `cargo doc --workspace --no-deps` → HTML API docs
- `README.md` de chaque module avec exemples
- `CHANGELOG.md` tracking versions

### 4. Validation
- `cargo check --workspace` passing partout
- Structure prête pour implémentation
- Tous les Cargo.toml configurés correctement
- Vendor cleanup effectué (pas de fichiers superflus)

---

## ✅ Validation Finale

**Questions pour vous** :

1. ✅ **Structure proposée approuvée ?** (libs/ + userland/)
2. ✅ **Extraction sources approuvée ?** (mimalloc, pqclean, uutils uniquement sources nécessaires)
3. ✅ **Organisation modules approuvée ?** (Cargo.toml, README, tests, benches, examples, vendor)
4. ✅ **Prêt à procéder Phase A ?** (créer Cargo workspaces)

**Si validation OK** :
- Je procède Phase A (créer workspaces)
- Puis Phase B (extraction sources externes)
- Puis Phase C (création structure complète tous modules)
- Puis Phase D (validation compilations)
- ENSUITE (avec accord) → implémentation code réel

**Votre GO ?** 🚦
