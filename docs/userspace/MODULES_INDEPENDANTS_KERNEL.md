# Modules Userspace - Développement Parallèle

> **Objectif** : Modules développables PENDANT la correction des prérequis kernel  
> **Principe** : Code fonctionnel, pas de stubs/TODOs, documentation complète

---

## 🟦 PRIORITÉ 0 : Bibliothèques Rust Pures (0 dépendance kernel)

### 1. **exo_std::collections** - Structures de données avancées
- **Dépôt** : Développement interne
- **Modules** :
  - `RingBuffer<T>` : Buffer circulaire lock-free (pour IPC)
  - `BoundedVec<T, const N: usize>` : Vec à capacité fixe (no_std)
  - `IntrusiveList<T>` : Liste doublement chaînée pour scheduler
  - `RadixTree<K, V>` : Arbre pour cache/lookup rapide
- **Tests** : Fuzzing avec proptest, benchmarks vs std
- **Docs** : Exemples d'usage, complexité temporelle, garanties thread-safety

### 2. **exo_crypto** - Cryptographie post-quantique
- **Dépôts à cloner** :
  ```bash
  git clone https://github.com/PQClean/PQClean.git
  git clone https://github.com/rustpq/pqcrypto.git
  ```
- **Modules** :
  - `kyber::kem` : Key Encapsulation (CRYSTALS-Kyber)
  - `dilithium::signature` : Signatures digitales
  - `hash::blake3` : Hashing rapide
  - `constant_time` : Utilitaires timing-attack resistant
- **Adaptations** :
  - Wrappers Rust safe au-dessus de PQClean (FFI)
  - API uniforme style RustCrypto
  - Support no_std complet
- **Optimisations** :
  - AVX2/AVX-512 detection runtime
  - Vectorisation SIMD explicite
  - Cache-timing resistant implementation
- **Docs** : NIST standards reference, exemples key exchange

### 3. **exo_types** - Types système fondamentaux
- **Dépôt** : Développement interne
- **Modules** :
  - `pid::Pid` : Process ID typesafe (newtypes)
  - `fd::FileDescriptor` : File descriptor avec RAII
  - `errno::Error` : Codes d'erreur POSIX + Exo custom
  - `time::Timestamp` : Monotonic/realtime timestamps
  - `signal::Signal` : Signal numbers typesafe
- **Garanties** : 
  - Zero-copy serialization (bytemuck)
  - Repr(transparent) pour FFI
  - Debug/Display impls complets
- **Tests** : Property-based testing des conversions
- **Docs** : Table errno codes, signal mapping POSIX

### 4. **exo_allocator** - Allocateurs custom
- **Dépôt externe** :
  ```bash
  git clone https://github.com/microsoft/mimalloc.git
  ```
- **Modules** :
  - `slab::SlabAllocator` : Pour objets taille fixe (descriptors)
  - `bump::BumpAllocator` : Arena allocator (parsing temporaire)
  - `mimalloc_wrapper` : Binding Rust de mimalloc
- **Adaptations** :
  - GlobalAlloc trait impl
  - Telemetry hooks (allocation tracking)
  - OOM handler custom
- **Optimisations** :
  - Thread-local caches
  - Large page support (2MB pages)
  - NUMA-aware allocation
- **Docs** : Benchmarks vs jemalloc/tcmalloc, use-cases

---

## 🟨 PRIORITÉ 1 : Utilitaires CLI (syscalls minimaux)

### 5. **exo_coreutils** - Outils POSIX essentiels
- **Dépôt** :
  ```bash
  git clone https://github.com/uutils/coreutils.git
  ```
- **Commandes à adapter** :
  - `cat`, `echo`, `ls` (syscalls : open/read/write/close/getdents)
  - `cp`, `mv`, `rm` (+ rename/unlink)
  - `mkdir`, `rmdir` (+ mkdir/rmdir syscalls)
  - `chmod`, `chown` (+ fchmod/fchown)
  - `ps`, `top` (lecture /proc)
- **Adaptations** :
  - Remplacer libc par exo_std::fs
  - Error handling unifié avec exo_types::errno
  - Output formatters avec `termcolor`
- **Optimisations** :
  - Buffering I/O (4KB chunks minimum)
  - Bulk operations (copy_file_range syscall)
  - Parallel directory traversal (rayon)
- **Docs** : Man pages complètes, exemples d'usage

### 6. **exo_shell_completion** - Autocomplétion avancée
- **Dépôt** :
  ```bash
  git clone https://github.com/clap-rs/clap.git
  ```
- **Modules** :
  - `parser::CommandParser` : Parsing ligne de commande
  - `completion::FuzzyMatcher` : Fuzzy search (sublime_fuzzy)
  - `history::Database` : Historique avec scoring
- **Features** :
  - Complétion contextuelle (fichiers, flags, commandes)
  - Scoring intelligent (frecency)
  - Suggestions syntax-aware
- **Adaptations** :
  - Intégration avec cosmic-term/shell Exo
  - Cache de complétion (sqlite)
- **Docs** : Guide configuration, API hooks

### 7. **exo_text_processing** - Parsers et formatters
- **Dépôts** :
  ```bash
  git clone https://github.com/dtolnay/serde.git
  git clone https://github.com/serde-rs/json.git
  git clone https://github.com/toml-rs/toml.git
  ```
- **Modules** :
  - `json::parser` : JSON streaming parser
  - `toml::de` : TOML deserializer (configs)
  - `markdown::renderer` : Markdown → HTML/terminal
  - `diff::unified` : Unified diff generator
- **Optimisations** :
  - SIMD JSON parsing (simd-json)
  - Zero-copy deserialization (serde_zero_copy)
  - Incremental parsing (pour gros fichiers)
- **Docs** : Benchmarks, exemples streaming

---

## 🟩 PRIORITÉ 2 : Services Userspace Pure Logic

### 8. **exo_config_manager** - Gestionnaire de configuration
- **Dépôt** :
  ```bash
  git clone https://github.com/mehcode/config-rs.git
  ```
- **Modules** :
  - `loader::ConfigLoader` : Chargement multi-sources (TOML, env vars)
  - `validator::SchemaValidator` : Validation avec schemars
  - `watcher::FileWatcher` : Hot-reload configs
  - `merger::HierarchyMerger` : Merge configs (user > system > default)
- **Adaptations** :
  - Storage dans `/etc/exo-os/` et `~/.config/exo-os/`
  - Intégration avec exo_ipc pour notifications changements
  - Migration automatique anciennes configs
- **Optimisations** :
  - Cache parsing (memoization)
  - Lazy loading (sections non utilisées)
- **Docs** : Schema JSON, exemples hierarchy

### 9. **exo_logger** - Système de logging structuré
- **Dépôt** :
  ```bash
  git clone https://github.com/tokio-rs/tracing.git
  ```
- **Modules** :
  - `collector::Collector` : Collecte logs multi-sources
  - `formatter::JsonFormatter` : Output structured (JSON Lines)
  - `sink::FileSink` : Rotation automatique (10MB max)
  - `filtering::DynamicFilter` : Filtrage runtime
- **Features** :
  - Niveaux : TRACE/DEBUG/INFO/WARN/ERROR/FATAL
  - Contextes span-based (tracing spans)
  - Correlation IDs (request tracking)
  - Sampling (log 1/N pour high-volume)
- **Adaptations** :
  - Backend IPC vers syslog-ng daemon
  - Indexation pour recherche rapide (tantivy)
- **Optimisations** :
  - Buffering asynchrone (tokio channels)
  - Compression logs anciens (zstd)
  - MPSC channel lock-free
- **Docs** : Guide filtres, exemples spans

### 10. **exo_metrics** - Telemetry et métriques
- **Dépôt** :
  ```bash
  git clone https://github.com/metrics-rs/metrics.git
  ```
- **Modules** :
  - `registry::MetricsRegistry` : Enregistrement métriques
  - `exporters::PrometheusExporter` : Format Prometheus
  - `aggregator::Histogram` : Histogrammes (P50/P95/P99)
  - `timers::Timer` : Mesure latences
- **Métriques système** :
  - CPU usage, memory usage
  - Syscall counts (si POSIX-X expose stats)
  - I/O throughput (read/write bytes)
  - IPC message rates
- **Adaptations** :
  - Exposition HTTP endpoint `/metrics`
  - Dashboard web minimal (HTML statique + fetch API)
- **Optimisations** :
  - Thread-local counters (agrégation périodique)
  - Lock-free atomic metrics
- **Docs** : Métriques disponibles, Grafana setup

---

## 🟪 PRIORITÉ 3 : IPC et Communication (stub kernel minimal)

### 11. **exo_ipc_types** - Types IPC pure Rust
- **Dépôt** : Développement interne
- **Modules** :
  - `message::Message` : Structure message IPC
  - `channel::ChannelId` : Identifiants canaux
  - `serializer::BincodeSerializer` : Sérialisation rapide
  - `protocol::HandshakeProtocol` : Handshake client/server
- **Format** :
  - Header : `[u32 msg_id, u32 size, u64 timestamp]`
  - Payload : Bincode serde
  - Checksum : CRC32C (sse4.2)
- **Tests** :
  - Roundtrip serialization
  - Fuzzing deserializer (cargo-fuzz)
- **Docs** : Schéma protocole, format wire

### 12. **exo_service_registry** - Registre de services
- **Dépôt** :
  ```bash
  git clone https://github.com/smol-rs/async-channel.git
  ```
- **Modules** :
  - `registry::ServiceRegistry` : HashMap<ServiceName, Endpoint>
  - `discovery::DiscoveryClient` : Client lookup services
  - `health::HealthChecker` : Ping/pong heartbeat
- **Features** :
  - Enregistrement : `register("fs_service", "/tmp/fs.sock")`
  - Lookup : `find("fs_service") -> Option<Endpoint>`
  - Hot-reload services (detection crashes + restart)
- **Adaptations** :
  - Storage persistent (sqlite ou fichier TOML)
  - Intégration init system
- **Optimisations** :
  - Cache lookup (LRU)
  - Bloom filter pour fast negative lookup
- **Docs** : API examples, service conventions

---

## 🟥 PRIORITÉ 4 : Documentation et Outillage

### 13. **exo_docs** - Système de documentation
- **Dépôt** :
  ```bash
  git clone https://github.com/rust-lang/mdBook.git
  ```
- **Structure** :
  ```
  docs/
  ├── book/         # mdBook principal
  │   ├── SUMMARY.md
  │   ├── userspace/
  │   ├── kernel/
  │   └── api/
  ├── rustdoc/      # API docs (cargo doc)
  └── man/          # Man pages (groff format)
  ```
- **Modules documentation** :
  - Architecture overview (diagrammes Mermaid)
  - Guides tutoriels (quick start, développement)
  - API reference complète
  - Troubleshooting guides
- **Génération** :
  - CI/CD auto-build sur commit
  - Hosting statique (GitHub Pages ou interne)
- **Docs** : Guide contribution, style guide

### 14. **exo_test_framework** - Framework de tests
- **Dépôts** :
  ```bash
  git clone https://github.com/rust-fuzz/cargo-fuzz.git
  git clone https://github.com/proptest-rs/proptest.git
  ```
- **Modules** :
  - `harness::TestHarness` : Runner tests userspace
  - `mock::MockKernel` : Stub kernel pour tests isolés
  - `fixtures::Fixtures` : Données de test
  - `coverage::CoverageCollector` : Code coverage (tarpaulin)
- **Types de tests** :
  - Unit tests (cargo test)
  - Integration tests (multi-process)
  - Fuzzing (cargo-fuzz sur parsers)
  - Property-based (proptest)
  - Performance tests (criterion)
- **Mock kernel** :
  - Syscalls stub retournant données prédictibles
  - Filesystem virtuel en mémoire (tmpfs)
  - Time control (mock clock)
- **Docs** : Guide tests, CI setup

### 15. **exo_benchmarks** - Suite de benchmarks
- **Dépôt** :
  ```bash
  git clone https://github.com/bheisler/criterion.rs.git
  ```
- **Benchmarks** :
  - IPC throughput (messages/sec)
  - Syscall overhead (si POSIX-X ready)
  - Memory allocators (alloc/dealloc cycles)
  - Crypto operations (encrypt/decrypt)
  - Parsing (JSON/TOML)
- **Comparaisons** :
  - Exo-OS vs Linux (même hardware)
  - Différents allocators (mimalloc vs jemalloc)
  - Avant/après optimisations
- **Output** :
  - HTML reports (criterion)
  - Graphs flamechart (perf + inferno)
- **Docs** : Méthodologie benchmarking, résultats historiques

---

## 🔧 Plan d'Exécution

### Phase A : Bibliothèques Fondamentales (Semaine 1-2)
```bash
# Cloner dépôts externes
cd /workspaces/Exo-OS/libs
git clone --depth 1 https://github.com/PQClean/PQClean.git external/PQClean
git clone --depth 1 https://github.com/microsoft/mimalloc.git external/mimalloc

# Créer modules Rust
cargo new exo_std/collections --lib
cargo new exo_crypto --lib
cargo new exo_types --lib
cargo new exo_allocator --lib

# Tests immédiatement
cargo test --workspace
cargo bench --workspace
```

### Phase B : Utilitaires et Services (Semaine 3-4)
```bash
# Cloner uutils coreutils
cd /workspaces/Exo-OS/userland
git clone https://github.com/uutils/coreutils.git external/uutils

# Créer services
cargo new services/config_manager --bin
cargo new services/logger --bin
cargo new services/metrics --bin
cargo new services/registry --bin

# Intégration IPC types
cargo new libs/exo_ipc/types --lib
```

### Phase C : Documentation et Validation (Semaine 5-6)
```bash
# Setup mdBook
cargo install mdbook mdbook-mermaid
mdbook init docs/book

# Framework tests
cargo new tests/framework --lib
cargo install cargo-fuzz cargo-tarpaulin

# Benchmarks
cargo new benches --lib
cargo install cargo-criterion

# Générer docs
cargo doc --workspace --no-deps --open
mdbook build docs/book
```

---

## 📋 Checklist Qualité (Golden Rules)

Pour CHAQUE module :

- [ ] **Code complet** : Implémentation fonctionnelle, pas de `unimplemented!()` ni `todo!()`
- [ ] **Tests** : 
  - [ ] Unit tests (coverage > 80%)
  - [ ] Integration tests si applicable
  - [ ] Property-based tests pour parsers/crypto
- [ ] **Documentation** :
  - [ ] Doc comments (/// ) sur toutes fonctions publiques
  - [ ] Exemples d'usage (```rust dans docs)
  - [ ] README.md du module avec architecture
  - [ ] CHANGELOG.md pour tracking versions
- [ ] **Optimisation** :
  - [ ] Profiling fait (perf/flamegraph)
  - [ ] Allocations minimisées (cargo-flamegraph)
  - [ ] Benchmarks vs alternatives
- [ ] **Sécurité** :
  - [ ] Pas d'unsafe non documenté
  - [ ] Input validation complète
  - [ ] Error handling exhaustif (pas de unwrap en prod)
- [ ] **CI/CD** :
  - [ ] Compilation warnings = 0
  - [ ] Clippy lints passed
  - [ ] rustfmt appliqué
  - [ ] Tests passing

---

## 🎯 Délivrables

### Livraison continue (chaque module)
1. **Code source** dans `/workspaces/Exo-OS/libs/<module>` ou `/userland/<module>`
2. **Tests** dans `<module>/tests/`
3. **Docs** dans `<module>/README.md` + rustdoc
4. **Benchmarks** dans `<module>/benches/`
5. **Exemples** dans `<module>/examples/`

### Documentation globale
- `docs/book/` : Guide complet mdBook
- `docs/rustdoc/` : API reference HTML
- `README.md` de chaque module avec badges (build status, coverage, crates.io)

### Validation
- Suite de tests complète : `cargo test --workspace` passing
- Benchmarks baseline établis
- Documentation accessible via `http://localhost:3000` (mdbook serve)

---

## 🔗 Dépendances Externes Maîtrisées

| Crate          | Version | Usage                  | Alternative interne |
|----------------|---------|------------------------|---------------------|
| serde          | 1.0     | Serialization          | Possible custom     |
| tokio          | 1.0     | Async runtime          | smol (plus léger)   |
| tracing        | 0.1     | Logging                | Custom collector    |
| criterion      | 0.5     | Benchmarking           | Aucune nécessaire   |
| proptest       | 1.0     | Property testing       | Custom generator    |
| mimalloc       | 0.1     | Allocation             | System allocator    |

**Principe** : Utiliser écosystème Rust mature MAIS garder possibilité de remplacer par implémentation custom si nécessaire (vendor locking évité).

---

**Résumé** : 15 modules développables immédiatement, 0 blocage kernel, documentation exhaustive, qualité production.
