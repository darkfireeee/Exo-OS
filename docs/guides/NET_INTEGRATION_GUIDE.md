# Guide d'Intégration - Pile Réseau Exo-OS

## Démarrage Rapide

### 1. Initialisation du Stack Réseau

```rust
use exo_os::net;

fn main() {
    // Initialiser le sous-système temps (requis)
    net::time::init();
    
    // Initialiser la pile réseau
    net::init().expect("Failed to initialize network stack");
    
    // Le stack est maintenant prêt!
}
```

### 2. Créer un Socket TCP

```rust
use exo_os::net::socket::{Socket, SocketDomain, SocketType};
use exo_os::net::ip::IpAddr;

// Créer un socket TCP
let socket = Socket::new(
    SocketDomain::Inet,    // IPv4
    SocketType::Stream,     // TCP
    0                       // Protocol (auto)
)?;

// Bind à une adresse locale
let local_addr = IpAddr::V4([127, 0, 0, 1]);
socket.bind(local_addr, 8080)?;

// Écouter les connexions
socket.listen(128)?;  // Backlog de 128 connexions

// Accepter une connexion
let (client, remote_addr) = socket.accept()?;
println!("Connexion depuis: {:?}", remote_addr);

// Recevoir des données
let mut buffer = vec![0u8; 4096];
let n = client.recv(&mut buffer)?;
println!("Reçu {} octets", n);

// Envoyer une réponse
let response = b"HTTP/1.1 200 OK\r\n\r\nHello, World!";
client.send(response)?;
```

### 3. Client TCP

```rust
// Créer un socket
let socket = Socket::new(SocketDomain::Inet, SocketType::Stream, 0)?;

// Connecter au serveur
let server_addr = IpAddr::V4([93, 184, 216, 34]);  // example.com
socket.connect(server_addr, 80)?;

// Envoyer une requête HTTP
let request = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
socket.send(request)?;

// Recevoir la réponse
let mut buffer = vec![0u8; 4096];
let n = socket.recv(&mut buffer)?;
println!("Réponse:\n{}", String::from_utf8_lossy(&buffer[..n]));
```

### 4. Socket UDP

```rust
// Créer un socket UDP
let socket = Socket::new(SocketDomain::Inet, SocketType::Dgram, 0)?;

// Bind (optionnel pour client)
socket.bind(IpAddr::V4([0, 0, 0, 0]), 5000)?;

// Envoyer un datagramme
let dest = IpAddr::V4([8, 8, 8, 8]);  // Google DNS
let query = b"\x12\x34\x01\x00\x00\x01...";  // DNS query
socket.send_to(query, dest, 53)?;

// Recevoir la réponse
let mut buffer = vec![0u8; 512];
let (n, from_addr, from_port) = socket.recv_from(&mut buffer)?;
println!("Reçu {} octets de {}:{}", n, from_addr, from_port);
```

## Configuration du Firewall

### 1. Règles Basiques

```rust
use exo_os::net::firewall::{FastRuleEngine, FiveTuple, Action};

// Créer le moteur de règles
let engine = FastRuleEngine::new(10_000);  // Cache de 10k entrées

// Ajouter une règle: accepter HTTP
let rule = CompiledRule {
    id: 1,
    priority: 100,
    bytecode: vec![
        Instruction::LoadDstPort,
        Instruction::EqImm16(80),
        Instruction::Match,
    ],
    action: Action::Accept,
    matches: AtomicU64::new(0),
};
engine.add_rule(rule);

// Matcher un paquet
let tuple = FiveTuple {
    src_ip: IpAddr::V4([192, 168, 1, 100]),
    dst_ip: IpAddr::V4([93, 184, 216, 34]),
    src_port: 12345,
    dst_port: 80,
    protocol: 6,  // TCP
};

let action = engine.match_packet(&tuple);
match action {
    Action::Accept => println!("Paquet accepté"),
    Action::Drop => println!("Paquet bloqué"),
    _ => {}
}
```

### 2. Connection Tracking

```rust
use exo_os::net::firewall::{PerCpuConntrack, ConnKey};

// Créer le tracker (4 CPUs, 10M connexions max)
let tracker = PerCpuConntrack::new(4, 10_000_000);

// Tracker un paquet
let key = ConnKey {
    src_ip: [192, 168, 1, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    dst_ip: [93, 184, 216, 34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    src_port: 12345,
    dst_port: 80,
    protocol: 6,
};

let state = tracker.track(&key, 1500);  // 1500 octets
println!("État de connexion: {:?}", state);

// Statistiques
let stats = tracker.stats();
println!("Total connexions: {}", stats.total_connections);
println!("Total paquets: {}", stats.total_packets);

// Garbage collection (périodique)
tracker.gc(current_time());
```

## WiFi Configuration

### 1. Scanner les Réseaux

```rust
use exo_os::net::drivers::wifi::{WiFiDriver, ScanType};

// Obtenir le driver WiFi
let wifi = WiFiDriver::instance();

// Scanner les réseaux disponibles
let networks = wifi.scan(ScanType::Active)?;

for bss in networks {
    println!("SSID: {}", bss.ssid);
    println!("  BSSID: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bss.bssid[0], bss.bssid[1], bss.bssid[2],
        bss.bssid[3], bss.bssid[4], bss.bssid[5]);
    println!("  Signal: {} dBm", bss.signal_strength);
    println!("  Channel: {}", bss.channel);
    println!("  Security: {:?}", bss.security);
}
```

### 2. Se Connecter à un Réseau

```rust
// Connecter avec WPA2
wifi.connect("MonReseau", "motdepasse")?;

// Attendre la connexion
while wifi.state() != ConnectionState::Connected {
    thread::sleep(Duration::from_millis(100));
}

println!("Connecté!");

// Obtenir les statistiques
let stats = wifi.stats();
println!("TX: {} paquets, {} octets", stats.tx_packets, stats.tx_bytes);
println!("RX: {} paquets, {} octets", stats.rx_packets, stats.rx_bytes);
```

### 3. Configuration Avancée

```rust
// Définir le pays (régulations)
wifi.set_country(b"US")?;

// Activer les optimisations
wifi.set_ampdu_enabled(true);   // A-MPDU aggregation
wifi.set_amsdu_enabled(true);   // A-MSDU aggregation

// Configurer la puissance
wifi.set_power_save(PowerSaveMode::Dynamic)?;  // U-APSD

// WiFi 6 TWT (Target Wake Time)
wifi.set_power_save(PowerSaveMode::Twt)?;
```

## Cryptographie TLS

### 1. Chiffrement AES-GCM

```rust
use exo_os::net::protocols::tls::{AesGcm, KeySize};

// Créer le cipher
let key = [0u8; 16];  // 128-bit key
let cipher = AesGcm::new(&key)?;

// Chiffrer
let nonce = [0u8; 12];
let plaintext = b"Message secret";
let aad = b"additional authenticated data";

let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad)?;

// Déchiffrer
let decrypted = cipher.decrypt(&nonce, &ciphertext, aad, &tag)?;
assert_eq!(decrypted, plaintext);
```

### 2. ChaCha20-Poly1305

```rust
use exo_os::net::protocols::tls::ChaCha20Poly1305;

let key = [0u8; 32];
let cipher = ChaCha20Poly1305::new(&key);

let nonce = [0u8; 12];
let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad);
let decrypted = cipher.decrypt(&nonce, &ciphertext, aad, &tag)?;
```

### 3. Dérivation de Clés HKDF

```rust
use exo_os::net::protocols::tls::Hkdf;

let hkdf = Hkdf::sha256();

// Extract
let salt = b"random salt";
let ikm = b"input keying material";
let prk = hkdf.extract(Some(salt), ikm);

// Expand
let info = b"application context";
let okm = hkdf.expand(&prk, info, 32)?;  // 32 bytes de clé

// Ou combiner extract+expand
let key = hkdf.derive(Some(salt), ikm, info, 32)?;
```

## Gestion du Temps

### 1. Mesures de Temps

```rust
use exo_os::net::time::{current_time_ns, current_time_us, Instant};

// Temps courant
let now_ns = current_time_ns();   // Nanoseconds depuis boot
let now_us = current_time_us();   // Microseconds depuis boot
let now_sec = current_time();     // Seconds depuis boot

// Timestamp réel (Unix)
let unix_us = realtime_us();      // Microseconds
let unix_sec = realtime();        // Seconds

println!("Uptime: {} secondes", now_sec);
println!("Unix timestamp: {}", unix_sec);
```

### 2. Mesure de Durée

```rust
use exo_os::net::time::Instant;

let start = Instant::now();

// ... opération à mesurer ...
perform_work();

let elapsed = start.elapsed_ns();
println!("Durée: {} ns", elapsed);

// Ou directement
println!("Durée: {} μs", start.elapsed_us());
println!("Durée: {} ms", start.elapsed_ms());
```

### 3. Timeouts

```rust
let timeout = Duration::from_secs(5);
let start = Instant::now();

loop {
    if start.elapsed() > timeout {
        return Err(Error::Timeout);
    }
    
    // Essayer l'opération
    if let Some(result) = try_operation() {
        return Ok(result);
    }
    
    thread::sleep(Duration::from_millis(10));
}
```

## Optimisations Avancées

### 1. Zero-Copy avec sendfile

```rust
use exo_os::net::utils::sendfile;

// Envoyer un fichier sans copie en userspace
let file_fd = open("/path/to/file", O_RDONLY)?;
let socket_fd = socket.fd();

// Transférer 1MB depuis l'offset 0
let sent = sendfile(socket_fd, file_fd, 0, 1024 * 1024)?;
println!("Envoyé {} octets", sent);
```

### 2. Batching avec io_uring (futur)

```rust
// À venir dans une version future
let ring = IoUring::new(256)?;

// Soumettre plusieurs opérations
ring.prep_send(socket_fd, &buffer1, MSG_DONTWAIT);
ring.prep_send(socket_fd, &buffer2, MSG_DONTWAIT);
ring.prep_send(socket_fd, &buffer3, MSG_DONTWAIT);

// Soumettre en batch
ring.submit()?;

// Attendre les complétions
let completions = ring.wait(3)?;
```

### 3. Configuration QoS

```rust
use exo_os::net::utils::qos::{QosPolicy, TrafficClass};

// Créer une politique QoS
let policy = QosPolicy::new()
    .set_bandwidth_limit(100_000_000)  // 100 Mbps
    .set_priority(TrafficClass::High)
    .set_burst_size(64 * 1024);        // 64 KB

// Appliquer au socket
socket.set_qos_policy(policy)?;
```

## Monitoring et Statistiques

### 1. Statistiques Socket

```rust
let stats = socket.stats()?;

println!("TX: {} paquets, {} octets", stats.tx_packets, stats.tx_bytes);
println!("RX: {} paquets, {} octets", stats.rx_packets, stats.rx_bytes);
println!("Retransmissions: {}", stats.retransmits);
println!("RTT moyen: {} μs", stats.avg_rtt_us);
```

### 2. Statistiques Firewall

```rust
let stats = engine.stats();

println!("Cache hits: {}", stats.cache_hits);
println!("Cache misses: {}", stats.cache_misses);
println!("Hash matches: {}", stats.hash_matches);
println!("Trie matches: {}", stats.trie_matches);

let hit_rate = (stats.cache_hits * 100) / (stats.cache_hits + stats.cache_misses);
println!("Taux de hit cache: {}%", hit_rate);
```

### 3. Monitoring WiFi

```rust
let stats = wifi.stats();
let link_quality = (stats.signal_strength + 100) * 100 / 60;  // 0-100%

println!("Qualité du lien: {}%", link_quality);
println!("Débit TX: {} Mbps", (stats.tx_bytes * 8) / 1_000_000);
println!("Débit RX: {} Mbps", (stats.rx_bytes * 8) / 1_000_000);
println!("Taux d'erreur: {}%", (stats.rx_errors * 100) / stats.rx_packets);
```

## Débogage et Tracing

### 1. Activer le Debug Logging

```rust
use exo_os::net::debug;

// Activer le logging détaillé
debug::set_log_level(LogLevel::Debug);

// Tracer un socket
socket.set_trace(true);

// Les opérations vont maintenant logger:
// [DEBUG] socket: bind(127.0.0.1:8080)
// [DEBUG] socket: listen(backlog=128)
// [DEBUG] tcp: SYN received from 192.168.1.100:12345
```

### 2. Performance Counters

```rust
use exo_os::net::perf;

// Démarrer le profiling
let profiler = perf::Profiler::new();

// Votre code
perform_network_operations();

// Obtenir le rapport
let report = profiler.report();
println!("{}", report);

// Output:
// Function               Calls    Total (μs)  Avg (μs)
// tcp_process            10000    5000        0.5
// firewall_check         10000    4000        0.4
// socket_recv            1000     15000       15.0
```

## Patterns Communs

### 1. Serveur HTTP Simple

```rust
fn http_server() -> Result<()> {
    let listener = Socket::new(SocketDomain::Inet, SocketType::Stream, 0)?;
    listener.bind(IpAddr::V4([0, 0, 0, 0]), 80)?;
    listener.listen(128)?;
    
    loop {
        let (client, _) = listener.accept()?;
        
        thread::spawn(move || {
            handle_client(client)
        });
    }
}

fn handle_client(client: Socket) -> Result<()> {
    let mut buffer = vec![0u8; 4096];
    let n = client.recv(&mut buffer)?;
    
    // Parse HTTP request...
    
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!";
    client.send(response)?;
    
    Ok(())
}
```

### 2. Client DNS

```rust
fn dns_query(domain: &str) -> Result<IpAddr> {
    let socket = Socket::new(SocketDomain::Inet, SocketType::Dgram, 0)?;
    
    // Construire la requête DNS
    let query = build_dns_query(domain)?;
    
    // Envoyer à 8.8.8.8:53
    socket.send_to(&query, IpAddr::V4([8, 8, 8, 8]), 53)?;
    
    // Recevoir la réponse
    let mut buffer = vec![0u8; 512];
    let (n, _, _) = socket.recv_from(&mut buffer)?;
    
    // Parser la réponse
    parse_dns_response(&buffer[..n])
}
```

### 3. Client HTTPS

```rust
fn https_get(url: &str) -> Result<String> {
    // Parser l'URL
    let (host, port, path) = parse_url(url)?;
    
    // Créer le socket
    let socket = Socket::new(SocketDomain::Inet, SocketType::Stream, 0)?;
    
    // Résoudre le DNS
    let ip = dns_query(host)?;
    
    // Connecter
    socket.connect(ip, port)?;
    
    // Handshake TLS 1.3
    let tls = TlsConnection::new(socket)?;
    tls.handshake(host)?;
    
    // Envoyer la requête HTTP
    let request = format!("GET {} HTTP/1.1\r\nHost: {}\r\n\r\n", path, host);
    tls.send(request.as_bytes())?;
    
    // Recevoir la réponse
    let mut response = Vec::new();
    let mut buffer = vec![0u8; 4096];
    loop {
        let n = tls.recv(&mut buffer)?;
        if n == 0 { break; }
        response.extend_from_slice(&buffer[..n]);
    }
    
    Ok(String::from_utf8(response)?)
}
```

## Erreurs Communes

### 1. Adresse déjà utilisée

```rust
// ❌ Erreur
let socket = Socket::new(...)?;
socket.bind(addr, 8080)?;  // Error: Address already in use

// ✅ Solution
let socket = Socket::new(...)?;
socket.set_reuse_addr(true)?;  // Activer SO_REUSEADDR
socket.bind(addr, 8080)?;
```

### 2. Timeout de connexion

```rust
// ❌ Sans timeout
socket.connect(addr, port)?;  // Peut bloquer longtemps

// ✅ Avec timeout
socket.set_timeout(Duration::from_secs(5))?;
match socket.connect(addr, port) {
    Ok(_) => println!("Connecté"),
    Err(Error::Timeout) => println!("Timeout"),
    Err(e) => println!("Erreur: {}", e),
}
```

### 3. Buffer trop petit

```rust
// ❌ Buffer fixe
let mut buffer = [0u8; 1024];
let n = socket.recv(&mut buffer)?;  // Peut perdre des données

// ✅ Buffer dynamique
let mut buffer = vec![0u8; 65536];  // 64 KB
let n = socket.recv(&mut buffer)?;
buffer.truncate(n);
```

## Ressources Supplémentaires

- **Architecture**: Voir `docs/architecture/NET_ARCHITECTURE.md`
- **Status**: Voir `docs/current/NET_FINAL_COMPLETE.md`
- **WiFi**: Voir `kernel/src/net/drivers/wifi/README.md`
- **Tests**: Voir `tests/integration/net_tests.rs`

## Support

Pour questions ou problèmes:
- GitHub Issues: https://github.com/exo-os/exo-os/issues
- Documentation: https://docs.exo-os.org/network
- Discord: https://discord.gg/exo-os
