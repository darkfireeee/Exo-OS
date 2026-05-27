# SPEC-EXO-OBSERVABILITY — Observabilité Système ExoOS
## log · tracing · monitor_server · ExoLedger Bridge

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0

---

## 1. Problème

Un OS à ~98% de maturité sans observabilité fiable est un OS aveugle. En v0.2.0, chaque sous-système doit être instrumenté de manière uniforme, structurée, et consultable en temps réel via `exo log`.

ExoOS a trois sources de données d'observabilité :

| Source | Producteur | Nature |
|--------|-----------|--------|
| **ExoLedger** | kernel/security | Audit de sécurité immuable (events de caps, accès refusés) |
| **Kernel logs** | kernel/all | Events système (boot, panic, IPC drop, scheduler) |
| **App traces** | Ring3 + Ring1 servers | Logs applicatifs structurés (tracing spans, log records) |

Ces trois sources convergent dans le `monitor_server` (Ring1) et sont consultables via `exo log`.

---

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ RING 0 — Kernel                                             │
│  klog::emit(level, msg)  ──────────────────────────────┐   │
│  ExoLedger::append(entry) ─────────────────────────┐   │   │
└────────────────────────────────────────────────────┼───┼───┘
                                                     │   │
                                    IPC direct Ring0→Ring1
                                                     │   │
┌────────────────────────────────────────────────────▼───▼───┐
│ RING 1 — monitor_server                                     │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ LedgerReader │  │  KernelLog   │  │  AppTraceReceiver│  │
│  │  (sécurité)  │  │  (système)   │  │  (Ring1+Ring3)   │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘  │
│         └─────────────────┼──────────────────-─┘           │
│                           │                                 │
│                    ┌──────▼──────┐                          │
│                    │  LogRouter  │                          │
│                    │  (filter,   │                          │
│                    │   format,   │                          │
│                    │   persist)  │                          │
│                    └──────┬──────┘                          │
│                           │                                 │
│              ┌────────────┼────────────┐                   │
│              │            │            │                    │
│         ExoFS blob    tty_server   IPC query               │
│         (persist)     (live display) (exo log cmd)         │
└─────────────────────────────────────────────────────────────┘
        ▲                              ▲
        │  IPC → monitor_server        │
┌───────┴──────────────────┐   ┌───────┴──────────────────┐
│ RING 1 servers           │   │ RING 3 apps              │
│ network_server (tracing) │   │ calendar, curl, exosh    │
│ crypto_server (tracing)  │   │ (log + tracing)          │
│ vfs_server (tracing)     │   │                          │
└──────────────────────────┘   └──────────────────────────┘
```

---

## 3. Façade `log` — Configuration Ring3

La crate `log` est une façade de logging légère compatible no_std. Elle dispatche vers un backend global enregistré au démarrage.

```rust
// exo-observability/src/log_backend.rs

use log::{Log, Metadata, Record, LevelFilter};

pub struct ExoLogBackend {
    monitor_cap: CapToken,  // Capability d'envoi vers monitor_server
}

impl Log for ExoLogBackend {
    fn enabled(&self, meta: &Metadata) -> bool {
        // Filtrer selon le niveau configuré (env var ou config ExoFS)
        meta.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) { return; }

        let entry = LogEntry {
            level:     record.level().into(),
            target:    record.target().into(),
            message:   format!("{}", record.args()),
            file:      record.file().map(Into::into),
            line:      record.line(),
            pid:       sys_getpid(),
            thread_id: sys_gettid(),
            ts_ns:     exo_ktime_now(),
        };

        // Envoi non-bloquant vers monitor_server (drop si plein)
        let _ = ipc_try_send(MonitorEndpoint::ID, MonitorMsg::Log(entry));
    }

    fn flush(&self) {
        let _ = ipc_send_recv(MonitorEndpoint::ID, MonitorMsg::Flush);
    }
}

/// Initialiser le backend log au démarrage d'un processus Ring3
pub fn init_log(cap: CapToken) {
    let backend = Box::new(ExoLogBackend { monitor_cap: cap });
    log::set_boxed_logger(backend).unwrap();
    log::set_max_level(LevelFilter::Info);  // configurable
}
```

---

## 4. `tracing` — Instrumentation Structurée

`tracing` est plus riche que `log` : il supporte les spans (durées), les champs structurés (key=value), et la propagation de contexte cross-async.

```rust
// exo-observability/src/tracing_backend.rs

use tracing_core::{Collect, Event, Metadata, span};

pub struct ExoTracingCollector {
    monitor_cap: CapToken,
    span_stack:  RefCell<Vec<SpanData>>,
}

impl Collect for ExoTracingCollector {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        meta.level() <= &tracing_core::Level::DEBUG
    }

    fn new_span(&self, attrs: &span::Attributes<'_>) -> span::Id {
        let id = SpanId::next();
        let data = SpanData {
            id:       id.clone(),
            name:     attrs.metadata().name().into(),
            target:   attrs.metadata().target().into(),
            fields:   collect_fields(attrs),
            start_ns: exo_ktime_now(),
            parent:   self.current_span(),
        };
        let _ = ipc_try_send(MonitorEndpoint::ID, MonitorMsg::SpanStart(data));
        id
    }

    fn record(&self, span: &span::Id, values: &span::Record<'_>) {
        let fields = collect_record_fields(values);
        let _ = ipc_try_send(MonitorEndpoint::ID, MonitorMsg::SpanRecord {
            id: span.clone(), fields
        });
    }

    fn event(&self, event: &Event<'_>) {
        let entry = TraceEvent {
            level:   *event.metadata().level(),
            target:  event.metadata().target().into(),
            fields:  collect_event_fields(event),
            span_id: self.current_span(),
            ts_ns:   exo_ktime_now(),
            pid:     sys_getpid(),
        };
        let _ = ipc_try_send(MonitorEndpoint::ID, MonitorMsg::Event(entry));
    }

    fn exit(&self, span: &span::Id) {
        let _ = ipc_try_send(MonitorEndpoint::ID, MonitorMsg::SpanEnd {
            id:      span.clone(),
            end_ns:  exo_ktime_now(),
        });
    }

    // ... autres méthodes requises ...
}

/// Instrumenter un serveur Ring1 avec tracing
pub fn init_tracing(cap: CapToken) {
    let collector = ExoTracingCollector::new(cap);
    tracing_core::dispatch::set_global_default(
        tracing_core::Dispatch::new(collector)
    ).unwrap();
}
```

**Usage dans les serveurs Ring1 :**
```rust
// Dans network_server — exemple d'instrumentation
use tracing::{info, warn, instrument, span, Level};

#[instrument(name = "tcp_connect", fields(addr = %addr))]
async fn handle_connect(addr: SocketAddr) -> Result<SocketHandle, NetError> {
    let span = span!(Level::DEBUG, "smoltcp_connect");
    let _guard = span.enter();

    info!(target: "network_server", addr = %addr, "Nouvelle connexion TCP");

    match iface.connect(addr) {
        Ok(h)  => { info!("Connexion établie"); Ok(h) }
        Err(e) => { warn!(error = ?e, "Échec connexion"); Err(e.into()) }
    }
}
```

---

## 5. monitor_server — Ring1

```rust
// monitor_server/src/main.rs (Ring1)

pub struct MonitorServer {
    log_buf:    RingBuffer<LogEntry, 4096>,  // Buffer circulaire (4096 entrées)
    trace_buf:  RingBuffer<TraceEvent, 8192>,
    span_map:   HashMap<SpanId, SpanData>,
    persist_cap: CapToken,  // Capability ExoFS pour la persistence
    tty_cap:     CapToken,  // Capability tty_server pour l'affichage live
    
    // Configuration de filtrage
    filter_level:  log::LevelFilter,
    filter_target: Option<String>,
}

impl MonitorServer {
    fn main_loop(&mut self) {
        loop {
            // 1. Recevoir les messages des producteurs
            while let Ok(msg) = ipc_try_recv(MonitorEndpoint::ID) {
                match msg {
                    MonitorMsg::Log(entry) => self.handle_log(entry),
                    MonitorMsg::Event(ev)  => self.handle_trace(ev),
                    MonitorMsg::SpanStart(s) => { self.span_map.insert(s.id.clone(), s); }
                    MonitorMsg::SpanEnd { id, end_ns } => self.close_span(id, end_ns),
                    MonitorMsg::Flush    => self.flush_to_disk(),
                    MonitorMsg::Query(q) => {
                        let result = self.query(q);
                        ipc_reply(MonitorResponse::Entries(result));
                    }
                }
            }

            // 2. Flush périodique (toutes les 5s ou si buffer > 80%)
            if self.should_flush() {
                self.flush_to_disk();
            }

            sys_sched_yield();
        }
    }

    fn handle_log(&mut self, entry: LogEntry) {
        // Affichage live si niveau >= WARN
        if entry.level <= log::Level::Warn {
            self.display_live(&entry);
        }
        // Stocker dans le ring buffer
        self.log_buf.push(entry);
    }

    fn display_live(&self, entry: &LogEntry) {
        // Format : 2026-05-14T22:31:05Z  [WARN]  network_server(5)  message
        let line = format_log_line(entry);
        let _ = ipc_try_send(TtyEndpoint::ID, TtyMsg::Print(line));
    }

    fn flush_to_disk(&self) {
        // Sérialiser le buffer et écrire dans ExoFS
        let blob = serialize_log_buf(&self.log_buf);
        let _ = ipc_send_recv(VfsEndpoint::ID, VfsRequest::WriteAt {
            handle:  self.log_handle,
            cap:     self.persist_cap,
            offset:  self.log_offset,
            data:    blob,
        });
    }

    fn query(&self, q: LogQuery) -> Vec<LogEntry> {
        // Filtrer le buffer selon les critères de la query
        self.log_buf.iter()
            .filter(|e| q.matches(e))
            .take(q.limit.unwrap_or(50))
            .cloned()
            .collect()
    }
}
```

---

## 6. Commande `exo log`

```
$ exo log

2026-05-14T22:31:05Z  [INFO]   network_server(5)   smoltcp:tcp_connect  93.184.216.34:443
2026-05-14T22:31:05Z  [INFO]   crypto_server(4)    tls:handshake  TLS1.3  ok
2026-05-14T22:31:05Z  [AUDIT]  security(0)         cap:denied  curl(43):fs:write:/etc/  →  ledger#4422
2026-05-14T22:31:06Z  [WARN]   network_server(5)   dns:timeout  retry 1/3
2026-05-14T22:31:07Z  [INFO]   network_server(5)   dns:resolved  example.com
2026-05-14T22:31:08Z  [INFO]   vfs_server(3)       exofs:epoch  commit ep:43  ✓
2026-05-14T22:31:09Z  [INFO]   exophoenix(0)       heartbeat  kernel-A  ssr:ok  0.2ms
2026-05-14T22:31:09Z  [INFO]   exoledger(0)        flush  4423 entries  signed  ep:43

$ exo log --level warn

2026-05-14T22:31:05Z  [AUDIT]  security(0)  cap:denied  curl(43):fs:write:/etc/  →  ledger#4422
2026-05-14T22:31:06Z  [WARN]   network(5)   dns:timeout  retry 1/3

$ exo log --pid 43

2026-05-14T22:30:58Z  [INFO]   curl(43)   start  cap@7a1e  net:r  fs:rw
2026-05-14T22:31:00Z  [INFO]   curl(43)   tcp:connect  93.184.216.34:443
2026-05-14T22:31:05Z  [AUDIT]  curl(43)   cap:denied  fs:write:/etc/  →  ledger#4422
2026-05-14T22:31:07Z  [INFO]   curl(43)   http:200  13.4 KiB  1245ms

$ exo log --trace --pid 43

[SPAN]  tcp_connect  addr=93.184.216.34:443  duration=245ms
  [EVENT]  smoltcp_connect  ok
  [SPAN]   tls_handshake    duration=187ms
    [EVENT]  tls:cert_verify  ok  issuer="Let's Encrypt"
    [EVENT]  tls:cipher       TLS_CHACHA20_POLY1305_SHA256
  [EVENT]  http_request     method=GET  path=/  bytes_sent=87
  [EVENT]  http_response    status=200  bytes_recv=13721
```

---

## 7. Niveaux de Log et Cibles Standard

| Niveau | Utilisation |
|--------|------------|
| `ERROR` | Erreur irrécupérable dans un serveur (ring restart nécessaire) |
| `WARN` | Comportement anormal non critique (retry, timeout, cap denied) |
| `INFO` | Événements normaux significatifs (connexion établie, fichier ouvert) |
| `DEBUG` | Détails d'implémentation (IPC reçu, allocation réussie) |
| `TRACE` | Profiling (chaque span smoltcp poll, chaque alloc) |

**Cibles standard** (utilisées dans `target: "..."`) :

| Cible | Producteur |
|-------|-----------|
| `"kernel"` | Ring0 kernel messages |
| `"exophoenix"` | Événements bascule A↔B |
| `"exoledger"` | Entrées d'audit sécurité |
| `"vfs_server"` | VFS et ExoFS |
| `"network_server"` | Réseau (smoltcp, DNS, DHCP) |
| `"crypto_server"` | Opérations cryptographiques |
| `"device_server"` | PCI, drivers |
| `"fb_server"` | Framebuffer et événements HID |
| `"exo_pkg"` | Gestionnaire de paquets |
| `"<app_name>"` | Applications Ring3 |

---

## 8. Métriques Système (exo metrics)

En plus des logs, le `monitor_server` expose des métriques :

```
$ exo metrics

ExoOS v0.2.0 — Métriques Système
══════════════════════════════════════════════════════

CPU (par core)
  Core 0: 12.3%  [kernel: 1.2%  ring1: 8.4%  ring3: 2.7%]
  Core 1: 8.7%   [kernel: 0.9%  ring1: 6.1%  ring3: 1.7%]

Mémoire
  Total:    4096 MiB
  Used:     412 MiB   (kernel: 48 MiB  ring1: 298 MiB  ring3: 66 MiB)
  Free:     3684 MiB
  Swap:     0 MiB      (ExoFS swap non configuré)

IPC
  SpscRing msgs/s: 2,847,492  (peak: 51,204,011)
  Latence avg:     3.2µs
  Drops (30s):     0

ExoFS
  Epoch courant:   43
  Blobs:           1,247  (dedup: 312 économisés)
  Taille totale:   2.8 GiB
  Snapshots:       8  (dernière: ep:40 → 2026-05-14T20:12:00Z)

Réseau
  Interface:   eth0  192.168.1.42/24
  RX:          1.24 GiB total  (142 KiB/s)
  TX:          324 MiB total   (38 KiB/s)
  Connexions:  3 actives (TCP)

Sécurité
  CapTokens actifs:  247
  Révocations (24h): 12
  Accès refusés (1h): 3   [→ exo audit pour détails]
  ExoKairos kills:    0
  ExoPhoenix:         kernel-A  ep:43  bascules: 2  (dernier: 1h ago)
```

---

## 9. Intégration dans les Serveurs Ring1

Chaque serveur Ring1 doit initialiser `tracing` et `log` en début de `main()` :

```rust
// Patron d'initialisation obligatoire pour tous les serveurs Ring1

fn main() {
    // 1. Obtenir la capability monitor_server via ipc_broker
    let monitor_cap = ipc_send_recv(BrokerEndpoint::ID, BrokerRequest::GetCap {
        service: ServiceId::Monitor,
        rights:  MonitorRights::Send,
    }).unwrap();

    // 2. Initialiser log + tracing
    exo_observability::init_log(monitor_cap);
    exo_observability::init_tracing(monitor_cap);

    // 3. Log de démarrage
    log::info!(target: "network_server", "Démarrage network_server");

    // 4. Suite du serveur...
}
```

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-OBSERVABILITY.md*
