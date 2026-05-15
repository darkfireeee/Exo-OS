Feuille de route d'adaptation réseau (Exo‑Net v2)

Exo‑OS adopte un modèle micro‑noyau : les pilotes réseau résident en ring 1 et les applications communiquent avec eux via des messages IPC. Pour offrir une pile réseau comparable à celle des systèmes Unix/Windows, tout en conservant le modèle de capabilities, nous allons intégrer des bibliothèques réseau Rust telles que smoltcp, hickory‑dns, dhcp4r, rtnetlink, hyper, axum et rustls.

Bibliothèques et fonctionnalités clés
Groupe	Bibliothèques	Fonctionnalités principales
Pile TCP/IP (bas niveau)	smoltcp	Pile autonome et événementielle sans allocation de tas, supportant IPv4/IPv6, TCP, UDP et ICMP. Elle est conçue pour des systèmes temps réel et atteint un débit proche du Gbps en mode loopback.
Configuration réseau	rtnetlink	Permet d’envoyer des messages Netlink (ajout/suppression d’interface, table de routage) depuis un serveur réseau.
Découverte et résolution	hickory‑dns, dhcp4r	Fournissent un serveur/client DNS asynchrone complet et une implémentation DHCPv4 pour configurer une interface (adresse IP, DNS, passerelle).
Transport HTTP et REST	hyper, axum	hyper implémente HTTP/1.1 et HTTP/2 côté client et serveur ; axum fournit un framework web basé sur tokio permettant de définir des routes, des middlewares et un routeur performant.
Chiffrement TLS	rustls	Bibliothèque TLS moderne, pure Rust, compatible TLS 1.2/1.3 et utilisée par hyper pour le HTTPS.
Adaptation au modèle Exo‑OS
1. Créer un crate exo-net

Le cœur de l’intégration consiste à encapsuler smoltcp dans un crate exo-net qui expose des abstractions compatibles avec le protocole IPC du network_server. Ce crate doit :

Compilations no_std : désactiver les fonctionnalités dépendantes de la bibliothèque standard et activer la fonctionnalité alloc lorsque nécessaire. Utiliser l’allocateur exo-alloc comme allocateur global.
Device trait personnalisé : implémenter smoltcp::phy::Device en utilisant des appels IPC pour envoyer et recevoir des paquets. Le serveur réseau délivre une capability pour une interface (par exemple CapNetIf), qui est utilisée par le trait. Exemple :
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken};
use smoltcp::time::Instant;

/// Représente une interface Exo‑OS reliée au network_server.
pub struct ExoDevice {
    iface_cap: CapNetIf,
}

impl<'a> Device<'a> for ExoDevice {
    type RxToken = ExoRxToken;
    type TxToken = ExoTxToken;
    fn receive(&'a mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        // Envoyer un message IPC pour recevoir un paquet.
        match ipc_recv_packet(self.iface_cap) {
            Ok(pkt) => Some((ExoRxToken { data: pkt }, ExoTxToken { iface_cap: self.iface_cap })),
            Err(IpcError::WouldBlock) => None,
            Err(e) => panic!("Erreur réseau : {:?}", e),
        }
    }
    fn transmit(&'a mut self) -> Option<Self::TxToken> {
        Some(ExoTxToken { iface_cap: self.iface_cap })
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(1);
        caps
    }
}

pub struct ExoRxToken { data: Vec<u8> }
pub struct ExoTxToken { iface_cap: CapNetIf }

impl RxToken for ExoRxToken {
    fn consume<R, F>(self, _timestamp: Instant, f: F) -> Result<R, smoltcp::Error>
    where
        F: FnOnce(&[u8]) -> Result<R, smoltcp::Error>,
    {
        f(&self.data)
    }
}

impl TxToken for ExoTxToken {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> Result<R, smoltcp::Error>
    where
        F: FnOnce(&mut [u8]) -> Result<R, smoltcp::Error>,
    {
        let mut buffer = vec![0u8; len];
        let result = f(&mut buffer[..]);
        if result.is_ok() {
            // envoyer le paquet via IPC
            ipc_send_packet(self.iface_cap, &buffer).map_err(|_| smoltcp::Error::Illegal)?;
        }
        result
    }
}

Cette structure illustre comment traduire les appels de smoltcp en messages IPC. Attention : il ne faut jamais bloquer dans la fonction receive() ; si aucune trame n’est disponible, renvoyer None et laisser la boucle réseau se reposer via sched_yield(). Ne pas oublier de vérifier les capacités du flux.

2. Intégrer exo-net dans network_server

Le network_server doit devenir l’exécutant de la pile. Les étapes principales sont :

Initialiser l’interface : lors de l’initialisation, récupérer la capability de l’interface Ethernet (fournie par le driver) et créer un ExoDevice à partir de celle‑ci.
Créer smoltcp::iface::EthernetInterface : configurer l’adresse MAC, l’adresse IP, la table de routage. Le serveur peut solliciter dhcp4r pour obtenir une adresse IP via DHCP.
Boucle principale : exécuter périodiquement iface.poll() pour traiter les timers et les paquets entrants. Utiliser le scheduler pour dormir quand aucun événement n’est disponible :
loop {
    // Poll smoltcp stack
    let timestamp = Instant::now();
    match iface.poll(&mut sockets, timestamp) {
        Ok(()) => (),
        Err(e) => log::error!("smoltcp error: {:?}", e),
    }
    // Attendre un événement ou un timeout
    if !has_pending_events() {
        sched_yield();
    }
}
Gestion des sockets userland : lors de la création d’un socket par un client, le serveur réseau crée un SocketHandle et renvoie un capability qui permet d’identifier ce socket. Les appels connect, send, recv sont traduits en messages IPC ; le serveur manipule les sockets smoltcp::socket::TcpSocket ou UdpSocket correspondants.
3. Intégrer les services DNS et DHCP

hickory-dns et dhcp4r doivent être exécutés dans le contexte du serveur réseau ou dans des serveurs dédiés. Quelques conseils :

Compilations no_std : désactiver les fonctionnalités std et activer async/tokio uniquement si vous intégrez un runtime. Préférer un exécuteur léger basé sur le scheduler Exo‑OS.
DNS : cache : hickory-dns propose un cache DNS. Assurez‑vous de le dimensionner pour limiter la consommation mémoire, et d’invalider les entrées lors de la résolution d’erreurs.
DHCP : timeout : la demande DHCP doit être retriée en cas de perte. Implémenter un mécanisme de backoff exponentiel pour éviter de saturer le réseau.
4. Utilisation de hyper/axum en userland

Les applications HTTP/REST peuvent s’appuyer sur les crates hyper et axum compilées avec no_std + tokio désactivé. Pour cela :

Créer un executor : tokio n’est pas disponible dans Exo‑OS par défaut. Utilisez le scheduler userland (voir exo_runtime_adaptation_v2.md) pour exécuter les futures.
Adapteurs I/O : remplacez tokio::net::TcpStream par un type ExoTcpStream qui envoie et reçoit via exo-net. Exemple :
pub struct ExoTcpStream {
    sock_cap: CapSocket,
}

impl AsyncRead for ExoTcpStream {
    // implémentation utilisant ipc_recv pour lire
}

impl AsyncWrite for ExoTcpStream {
    // implémentation utilisant ipc_send pour écrire
}

// Dans axum :
let listener = ExoTcpListener::bind("0.0.0.0:80").await?;
axum::Server::builder(listener).serve(app.into_make_service()).await?;
TLS : rustls peut être compilé en no_std et utilisé via hyper-rustls. Cependant, Exo‑OS dispose d’un crypto_server ; il est recommandé d’implémenter un connecteur TLS qui délègue les primitives cryptographiques (échange de clés, AEAD) au crypto_server afin que les secrets restent dans le keystore.
5. Pièges et erreurs courantes
Oublier no_std : si les bibliothèques sont compilées avec la feature std, elles utiliseront des appels de système non disponibles sur Exo‑OS, provoquant des plantages silencieux. Toujours ajouter default-features = false dans Cargo.toml.
Boucles actives : ne jamais bloquer indéfiniment en attendant un paquet. Utilisez sched_yield() ou des timeouts pour éviter de monopoliser le CPU.
Mauvaise gestion des capacités : chaque socket ou interface est représenté par un token. N’échangez jamais ces tokens entre processus sans vérifier les droits. En cas d’erreur (ex : socket fermé), renvoyez un code d’erreur clair plutôt que de paniquer.
Taille des buffers : smoltcp nécessite un buffer pour chaque socket. Si le buffer est trop petit, la connexion risque d’être lente ou instable. Adaptez la taille en fonction de la bande passante et de la mémoire disponible.
Prise en charge IPv6 : si vous activez IPv6, assurez‑vous que tous les serveurs (DHCPv6, DNS) sont disponibles. Sinon, désactivez la feature pour éviter des comportements indéterminés.
Conclusion

Cette seconde feuille de route fournit une approche détaillée pour porter des bibliothèques réseau modernes sur Exo‑OS. En respectant les exemples de code et en évitant les pièges courants, vous pourrez construire une pile réseau performante, sécurisée et conforme au modèle de micro‑noyau. Les prochains travaux consisteront à intégrer progressivement ces modules dans network_server et à proposer des API POSIX‑compatibles via exo-libc.

Cryptographie (exo‑crypto) :
Feuille de route d'adaptation cryptographique (Exo‑Crypto v2)

L'architecture d'Exo‑OS repose sur un serveur crypto_server qui effectue les opérations cryptographiques sensibles et gère un keystore accessible via des capabilities. Pour offrir une cryptographie moderne comparable à celle des systèmes classiques, il est nécessaire d'adapter les bibliothèques Rust spécialisées (crates RustCrypto, ring, libsodium, rustls) aux contraintes d'Exo‑OS.

Bibliothèques et usages
Groupe	Bibliothèques	Usage principal
AEAD et blocs	aes-gcm, chacha20-poly1305, aes, des, etc. (dans rustcrypto-aeads et rustcrypto-block-ciphers)	Implémentent des chiffrements symétriques modernes (AES‑GCM, ChaCha20‑Poly1305) et des modes de chiffrement classiques.
Cryptographie asymétrique	rustcrypto-rsa, rustcrypto-elliptic-curves, ring	Fournissent des signatures RSA, ECDSA, ECDH et des primitives de courbes elliptiques ; ring offre des implémentations en assembleur pour plus de performance.
Hachage et dérivation	rustcrypto-hashes, rustcrypto-kdfs, rustcrypto-password-hashes	Fournissent SHA‑2, SHA‑3, Blake2, PBKDF2, HKDF, Argon2id et d’autres fonctions de dérivation sécurisées.
Bibliothèques complètes	libsodium	Equivalent Rust de NaCl ; propose un API high‑level (crypto_box, crypto_secretbox) pour chiffrer, signer et dériver des clés.
TLS	rustls	Implémentation TLS 1.2/1.3 en Rust ; dépend de ring pour les primitives bas niveau.
Stratégie d'intégration avec le crypto_server
1. Créer un crate exo-crypto
Objectifs : factoriser l’usage des crates RustCrypto et des autres bibliothèques en exposant une API uniforme orientée capabilities. Le crate exo-crypto servira de pont entre les applications userland et le crypto_server.
Structure : subdiviser le crate en modules (par exemple aead, kdf, sign, verify, tls) et réexporter les traits définis par les crates RustCrypto. Chaque fonction prendra en paramètre un identifiant de clé (une capability) et des données sérialisées, et renverra soit un résultat brut, soit une structure. Exemple :
// exo_crypto/aead.rs
pub fn encrypt_aead(cap: SymKeyCap, nonce: &[u8], aad: &[u8], plaintext: &[u8])
    -> Result<Vec<u8>, CryptoError>
{
    // Envoyer une requête au crypto_server.  Les clés ne sortent jamais du serveur.
    let request = CryptoRequest::Encrypt { key: cap, nonce: nonce.to_vec(), aad: aad.to_vec(), data: plaintext.to_vec() };
    let resp: CryptoResponse = ipc_crypto(request)?;
    match resp {
        CryptoResponse::Data(ciphertext) => Ok(ciphertext),
        CryptoResponse::Error(e) => Err(e),
        _ => Err(CryptoError::Protocol),
    }
}

pub fn decrypt_aead(cap: SymKeyCap, nonce: &[u8], aad: &[u8], ciphertext: &[u8])
    -> Result<Vec<u8>, CryptoError> {
    let request = CryptoRequest::Decrypt { key: cap, nonce: nonce.to_vec(), aad: aad.to_vec(), data: ciphertext.to_vec() };
    let resp: CryptoResponse = ipc_crypto(request)?;
    match resp {
        CryptoResponse::Data(plaintext) => Ok(plaintext),
        CryptoResponse::Error(e) => Err(e),
        _ => Err(CryptoError::Protocol),
    }
}

Ce code illustre une approche stateless : les clés ne sortent jamais du serveur, et l'API se contente de transférer les données et de récupérer un résultat.

Adaptation no_std : toutes les dépendances doivent être compilées avec default-features = false. Activez la feature alloc pour allouer des vecteurs via exo-alloc. Évitez d’utiliser std::time ; préférez core::time::Duration.
2. Intégrer ring et libsodium

ring fournit des implémentations optimisées de primitives (AES‑GCM, RSA, ECDH, SHA‑2) en assembleur. Pour l’utiliser dans Exo‑OS :

Compilez ring en désactivant la feature std et en activant default-features = false. Une fois compilé, vous pouvez l’utiliser pour implémenter les opérations qui ne sont pas encore prises en charge par les crates RustCrypto (par exemple, ECDSA P‑256).
Créez des wrappers dans exo-crypto qui appellent ring localement ou via le crypto_server. L’exemple suivant montre comment générer une clé RSA et signer un message :
// Génération d'une clé RSA via ring (exécuté dans crypto_server)
let rng = ring::rand::SystemRandom::new();
let private_key = ring::signature::RsaKeyPair::generate_pkcs8(&rng, 2048).unwrap();
// Stocker la clé dans le keystore et retourner une capability.
let cap = keystore_store_private_key(private_key.as_ref())?;

// Signature
pub fn sign_rsa(cap: AsymKeyCap, msg: &[u8]) -> Result<Vec<u8>, CryptoError> {
    // Le serveur récupère la clé via cap et utilise ring pour signer.
    let key = keystore_get_key(cap)?;
    let keypair = ring::signature::RsaKeyPair::from_pkcs8(&key).unwrap();
    let mut signature = vec![0u8; keypair.public_modulus_len()];
    keypair.sign(&ring::signature::RSA_PKCS1_SHA256, &rng, msg, &mut signature)
        .map_err(|_| CryptoError::Internal)?;
    Ok(signature)
}

libsodium peut être une alternative simple pour des fonctions comme crypto_secretbox (chiffrement symétrique authentifié) ou crypto_sign. Pour l’utiliser :

Compilez libsodium sans le support dynamique et liez‑le statiquement.
Désactivez la génération de clés dans l’espace utilisateur ; déléguez‑la au crypto_server.
Évitez d’utiliser les API crypto_*_easy qui masquent les erreurs de vérification ; préférez les API crypto_*_detached afin de vérifier explicitement les tags d’authentification.
3. Gestion du TLS avec rustls

rustls dépend de ring pour les primitives et fournit une implémentation TLS 1.2/1.3 sûre. Pour l’intégrer :

Compilez rustls en no_std (feature std désactivée). Activez la feature dangerous_configuration uniquement si vous devez charger des certificats non contrôlés.
Implémentez un type ExoServerConnection qui dérive de rustls::ServerConnection mais récupère les clés privées via des capabilities et délègue les opérations de signature au crypto_server.
Assurez‑vous que la vérification des certificats utilise crypto_server pour réaliser les hash et signatures en constant‑time.
4. Pièges et erreurs à éviter
Dévoiler des clés : ne récupérez jamais la clé privée ou secrète dans l’espace utilisateur. Utilisez toujours une capability pour référencer la clé et exécutez les opérations critiques dans le serveur.
Non‑constante‑time : certaines opérations (comparaison de tags d’authentification, dérivation de clés) doivent être exécutées en temps constant pour éviter les fuites d’information. Utilisez les primitives de subtle ou des méthodes fournies par les crates (ex : crypto_mac::MacResult::ct_eq) et bannissez les comparaisons naïves.
Mélange de bibliothèques : n’utilisez pas simultanément des primitives de ring et de RustCrypto pour le même algorithme sans homogénéiser les formats (endianness, padding). Définissez un format unique dans exo-crypto.
Gestion mémoire : libérez toujours les buffers contenant des secrets en utilisant des fonctions de nettoyage (par ex. zeroize::Zeroize) pour éviter de laisser des traces en mémoire.
Erreurs silencieuses : ne masquez pas les erreurs renvoyées par le crypto_server. Par exemple, si le tag d’authentification est incorrect, renvoyez explicitement CryptoError::AuthenticationFailed plutôt que de renvoyer un vecteur vide, afin d’éviter des attaques par intégrité.
5. Étapes de mise en œuvre
Définir le protocole IPC entre exo-crypto et crypto_server : types de requête (Encrypt, Decrypt, Sign, Verify, GenerateKey, DeriveKey) et réponses correspondantes. Documenter le format binaire et les codes d’erreur.
Refactoriser crypto_server pour utiliser les crates RustCrypto et ring. Chaque requête doit vérifier les droits du capability et exécuter l’opération dans un environnement constant‑time. Implémenter des quotas par utilisateur et des logs d’audit.
Développer exo-crypto comme bibliothèque userland fournissant des fonctions de haut niveau (encrypt, decrypt, derive_key, sign, verify). Pour les algorithmes non disponibles dans crypto_server, exécuter l’opération localement mais en utilisant une implémentation RustCrypto en no_std.
Mettre à jour les applications (HTTP/TLS, stockage sécurisé) pour remplacer toute dépendance à ring ou libsodium par exo-crypto.
Conclusion

Cette feuille de route fournit une méthodologie claire pour intégrer des bibliothèques cryptographiques modernes dans Exo‑OS. Les exemples de code démontrent l’utilisation des capabilities, l’implémentation d’une API uniforme et le respect des contraintes de sécurité. En suivant ces recommandations, vous éviterez les erreurs silencieuses et assurerez la confidentialité des clés tout en offrant un large éventail de primitives cryptographiques aux applications.

Systèmes de fichiers (exo‑fs) :
Feuille de route d'adaptation des systèmes de fichiers (Exo‑FS v2)

Le système de fichiers d’Exo‑OS repose actuellement sur ExoFS, un format orienté blobs optimisé pour la persistance des données userland. Pour atteindre une compatibilité fonctionnelle proche des systèmes Linux/Windows, il est nécessaire de supporter d’autres formats populaires (ext2/3/4, FAT, RedoxFS) et d’améliorer le VFS. Cette feuille de route détaille l'intégration de bibliothèques de fichiers Rust et fournit des exemples de code pour éviter les écueils courants.

Bibliothèques cibles et rôles
Groupe	Bibliothèques	Description
Ext4	ext4-rs / ext4_lwext4	Wrappe le système de fichiers lwext4 en Rust ; permet de créer, monter et manipuler des volumes ext2/3/4.
FAT	fatfs	Bibliothèque Rust prenant en charge les volumes FAT12/16/32 ; nécessaire pour monter des clés USB et partitions EFI.
RedoxFS	redoxfs	Système de fichiers journalisé utilisé par Redox OS ; propose un API Rust complet.
FUSE	libfuse (C)	Permet d’implémenter des systèmes de fichiers en espace utilisateur. Utile pour monter des formats exotiques via un serveur FUSE.
Architecture d’intégration
1. Serveurs dédiés par format

Chaque format doit être encapsulé dans un serveur ring 1 distinct (par ex. ext_server, fat_server, redoxfs_server). Ces serveurs implémentent un protocole IPC standardisé avec le vfs_server :

open_inode : ouvre un fichier ou un répertoire et renvoie un handle.
read_at / write_at : lit/écrit un certain nombre d’octets à un offset.
create, unlink, mkdir, rmdir, etc. Suivent la sémantique POSIX.
sync : force la persistance sur le support.

L’ext_server utilisera ext4-rs pour monter le volume et accéder aux inodes, tandis que fat_server s’appuiera sur fatfs. Chaque serveur gère le cache propre à son système de fichiers et fournit une interface cohérente au vfs_server.

2. Implémenter le trait BlockDevice via IPC

La plupart des bibliothèques de fichiers nécessitent un objet implémentant un trait BlockDevice pour accéder au support de stockage. Dans Exo‑OS, le stockage est accessible via un serveur block_server. Il faut donc adapter ce trait pour utiliser des messages IPC. Exemple :

use ext4_rs::BlockDevice;

pub struct ExoBlockDevice {
    dev_cap: CapBlockDevice,
    block_size: usize,
}

impl BlockDevice for ExoBlockDevice {
    type Error = FsError;
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        // Demander un bloc via IPC.  Le serveur block_server renvoie un vecteur de bytes.
        let data = ipc_block_read(self.dev_cap, lba, buf.len())?;
        if data.len() != buf.len() { return Err(FsError::UnexpectedEof); }
        buf.copy_from_slice(&data);
        Ok(())
    }
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        // Envoyer un bloc via IPC.  S’assurer que la taille est un multiple de block_size.
        if buf.len() % self.block_size != 0 {
            return Err(FsError::InvalidInput);
        }
        ipc_block_write(self.dev_cap, lba, buf)
            .map_err(|_| FsError::IoError)
    }
}

Attention : toujours valider la taille et l’alignement avant d’écrire. Un offset ou une taille incorrects peuvent corrompre le système de fichiers silencieusement.

3. Intégration dans le VFS

Le vfs_server doit router les appels vers le serveur approprié en fonction du type de volume :

Lorsqu’un volume est monté, le vfs_server interroge la partition pour déterminer le type (par ex. via la table de partitions ou la signature du superblock) et crée le serveur adéquat si nécessaire.
Pour chaque inode, le vfs_server maintient une structure VNode qui contient une référence au serveur backend et un identifiant interne. Les appels POSIX (read, write, open, stat) sont traduits en appels IPC vers ce backend.
Les attributs Unix (uid, gid, mode, atime, mtime) peuvent être ignorés ou mappés vers des labels de sécurité. Par exemple, les bits rwx peuvent être convertis en droits de capabilities (lecture, écriture, exécution) au moment du montage.
4. Support FUSE

Pour les formats exotiques qui ne disposent pas de crate Rust, on peut utiliser libfuse en C et fournir un serveur FUSE. Ce serveur traduit les callbacks FUSE (lookup, open, read, write) en appels IPC vers le VFS ou vers le serveur de blocs. Les étapes principales :

Compiler libfuse en mode statique pour Exo‑OS et exposer une API Rust minimale via des FFI.
Créer un serveur fuse_server qui s’enregistre auprès de init_server et reçoit une capability pour gérer les volumes FUSE.
Pour éviter des erreurs silencieuses, vérifier systématiquement les retours de fuse_reply_*(...). Une erreur non vérifiée peut entraîner un blocage du montage.
5. Pièges et bonnes pratiques
Validation des offsets : toujours vérifier que la lecture/écriture reste dans les limites du volume. Une erreur d’offset peut entraîner une corruption silencieuse.
Gestion des caches : chaque serveur FS gère un cache de blocs. Synchronisez ce cache lors des appels sync et avant le démontage. Ne jamais vider le cache sans flush ; cela conduit à une perte de données.
Permissions : Exo‑OS ne gère pas les utilisateurs ; les bibliothèques ext2/3/4 traitent les champs uid/gid. Décidez d’une stratégie (ignorer, mapper vers des labels) et documentez‑la clairement pour éviter des incohérences.
Journalisation : si vous utilisez un FS journalisé (ext4, RedoxFS), assurez‑vous que les métadonnées sont flushées correctement. Des transactions partielles peuvent provoquer des états incohérents et des erreurs silencieuses.
Éviter les unwrap() : les bibliothèques de fichiers peuvent renvoyer des erreurs (par exemple, Error::NotEnoughSpace). Capturez ces erreurs et propagez‑les via l’IPC plutôt que d’utiliser unwrap().
6. Étapes de développement
Implémenter les adaptateurs ExoBlockDevice pour chaque bibliothèque et créer les serveurs dédiés. Tester la création et le montage de volumes dans un environnement isolé.
Étendre le vfs_server pour détecter le format au montage et router les appels. Adapter les structures VNode pour stocker des informations supplémentaires (par ex. numéro d’inode, type de FS).
Augmenter exo-libc pour exposer des appels POSIX supplémentaires (statx, fstat, renameat2). Implémenter ces appels via l’IPC.
Écrire des outils utilitaires : mkfs.ext4.exo, fsck.ext4.exo, mkfs.fat.exo. Ces outils utiliseront les crates Rust directement et fonctionneront via exo‑alloc.
Réaliser des tests : effectuer des tests de stress (écriture simultanée, crash recovery) et mesurer les performances (latence, bande passante). Ajuster la taille des caches et la granularité des flushs en conséquence.
Conclusion

Cette feuille de route propose une stratégie structurée pour apporter un support multi‑format au VFS d’Exo‑OS. En implémentant des serveurs dédiés pour ext4, FAT et RedoxFS et en adaptant les bibliothèques correspondantes via IPC, vous garantirez une compatibilité et une robustesse comparables aux systèmes traditionnels. Les exemples de code et les pièges répertoriés vous aideront à éviter des corruptions silencieuses et à assurer une intégrité des données.

Allocateurs mémoire (exo‑alloc) :
Feuille de route d'adaptation des allocateurs mémoire (Exo‑Alloc v2)

Exo‑OS offre un environnement no_std où la gestion mémoire userland doit être explicitement gérée. Les allocateurs Rust standard s’appuient sur des appels du noyau (mmap, sbrk) indisponibles directement ; il est donc crucial d’adapter des allocateurs externes (snmalloc, jemalloc, dlmalloc) afin de fournir une allocation rapide, sécurisée et intégrée au modèle de capabilities.

Bibliothèques évaluées
Allocateur	Description	Forces
snmalloc	Allocateur moderne avec pools par thread, isolation des allocations et protections contre les use‑after‑free.	Haute performance, sécurité améliorée, adapté aux environnements no_std.
jemalloc	Allocateur haute performance utilisé par Rust avant 1.32, bien optimisé pour le multi‑thread.	Faible fragmentation et bonne scalabilité.
dlmalloc	Portage Rust de Doug Lea malloc.	Simplicité et portabilité ; adapté comme fallback.
Contexte du noyau

Le noyau Exo‑OS expose des appels systèmes de gestion mémoire inspirés de Linux : mmap, munmap, mremap, mprotect. Ces syscalls doivent être invoqués via exo-libc et sont accessibles depuis userland. Il n’existe pas de brk/sbrk, ni de page fault automatiques. Les allocateurs doivent :

Utiliser mmap pour réserver de nouveaux segments de mémoire, en spécifiant les flags MAP_PRIVATE | MAP_ANONYMOUS et en alignant sur la taille de page.
Libérer la mémoire via munmap lorsque les blocs sont retournés.
Gérer la concurrence via des verrous ou des structures lock‑free adaptées à un environnement sans threads POSIX ; les primitives peuvent reposer sur AtomicUsize et sched_yield().
Intégration de snmalloc comme allocateur par défaut
Compilation

Dans Cargo.toml, désactivez les fonctionnalités par défaut et activez build_allocator :

[dependencies]
snmalloc-rs = { version = "0.3", default-features = false, features = ["build_allocator"] }
Initialisation

Définissez un type implémentant GlobalAlloc et redirigeant les appels vers les syscalls d’Exo‑OS. Exemple simplifié :

use core::alloc::{GlobalAlloc, Layout};
use snmalloc_rs::SnMalloc;

/// Wrapper autour de snmalloc pour Exo‑OS.
pub struct ExoSnmalloc;

unsafe impl GlobalAlloc for ExoSnmalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(layout.align());
        match exo_mmap(size) { // exo_mmap: fonction wrapper appelant le syscall mmap
            Ok(ptr) => ptr as *mut u8,
            Err(_) => core::ptr::null_mut(),
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size().max(layout.align());
        let _ = exo_munmap(ptr as usize, size);
    }
}

#[global_allocator]
static GLOBAL: ExoSnmalloc = ExoSnmalloc;

Cette implémentation montre comment déléguer l’allocation et la libération à des fonctions spécifiques (exo_mmap, exo_munmap) qui encapsulent les syscalls Exo‑OS. Dans une intégration complète, on laissera snmalloc gérer ses propres arènes ; exo_mmap ne sera invoqué qu’en cas de demande de pages supplémentaires.

Gestion des gros blocs

Snomalloc distingue les petites allocations (dans des pools) des grandes allocations. Assurez‑vous que les appels à exo_mmap respectent l’alignement et ne demandent pas plus que nécessaire. Libérez toujours les blocs avec exo_munmap lorsque snmalloc appelle la fonction de destruction.

Fallback vers d’autres allocateurs

En cas d’incompatibilité (architectures non supportées), configurez exo-alloc pour tomber en repli sur dlmalloc :

#[cfg(not(feature = "snmalloc"))]
use dlmalloc::GlobalDlmalloc;

#[cfg(not(feature = "snmalloc"))]
#[global_allocator]
static GLOBAL: GlobalDlmalloc = GlobalDlmalloc;
Tests de performance

Mesurez la latence et la fragmentation en comparant snmalloc, jemalloc et dlmalloc à l’aide d’outils internes (boucles d’allocation/libération et allocation multithread). Sélectionnez l’allocateur par défaut en fonction des résultats et de l’architecture ciblée.

Intégration de jemalloc

Jemalloc peut être utile pour les programmes intensifs en multi‑thread. Pour l’intégrer :

Ajoutez dans Cargo.toml :

 [dependencies]
 jemallocator = { version = "0.4", default-features = false }
Écrivez des wrappers pour les fonctions non supportées (ex. mallctl) et redirigez mmap/munmap vers exo_libc.
Déclarez #[global_allocator] uniquement dans les binaires qui nécessitent jemalloc. Ne mélangez pas plusieurs allocateurs dans le même binaire.
Adaptation de dlmalloc

Dlmalloc reste un choix simple et portable pour des scénarios de fallback. Les étapes d’intégration :

Configurez l’allocation initiale en utilisant mmap plutôt que sbrk.
Désactivez les verrous globaux si vous exécutez en mono‑thread. Activez‑les si vous utilisez un runtime comme tokio.
Exposez dlmalloc dans exo-alloc via une feature dlmalloc.
Pièges et bonnes pratiques
Alignement : lorsque vous invoquez mmap, assurez‑vous que la taille est un multiple de la taille de page (souvent 4096 octets) et que l’adresse retournée est alignée. Un mauvais alignement peut entraîner des erreurs silencieuses et des violations de mémoire.
Double libération : libérer plusieurs fois le même pointeur corrompt l’état interne de l’allocateur et peut provoquer des plantages intermittents. Utilisez des types sûrs (Box, Vec) et évitez les appels manuels à free().
Fragmentation : dans des programmes de longue durée, de petites allocations peuvent fragmenter l’espace. Préférez snmalloc ou jemalloc qui ont des stratégies de compactage et de réutilisation.
Épuisement des pages : lorsque le crypto_server ou vfs_server consomme beaucoup de mémoire, l’allocateur peut renvoyer null. Capturez cette erreur et renvoyez une erreur claire aux applications plutôt que de paniquer.
Threads : si vous utilisez un runtime asynchrone (tokio ou async‑std), assurez‑vous que l’allocateur choisi est thread‑safe et qu’il libère les arènes des threads terminés. Jemalloc et snmalloc gèrent cela ; dlmalloc ne le fait pas par défaut.
Étapes finales
Compléter le crate exo-alloc en exposant une API pour sélectionner l’allocateur (features snmalloc, jemalloc, dlmalloc). Par défaut, activez snmalloc.
Mettre à jour les programmes userland pour utiliser exo-alloc en tant qu’allocateur global. Cela peut se faire en ajoutant #[global_allocator] dans la crate racine ou via une dépendance commune.
Documenter les choix : indiquez quel allocateur est actif dans chaque composant et comment changer cette option via Cargo features.
Réaliser des tests de stress et des benchmarks pour valider la fiabilité et la performance de chaque allocateur sur des cas d’usage réels.
Conclusion

En adaptant des allocateurs modernes et en fournissant un fallback robuste, Exo‑OS assure une gestion mémoire performante et sécurisée, essentielle pour la stabilité des serveurs et des applications userland. Les exemples de code et les avertissements ci‑dessus vous aideront à éviter les erreurs silencieuses et à intégrer ces allocateurs dans le modèle de capabilities.

Bibliothèques système (exo‑system) :
Feuille de route d'adaptation des bibliothèques système (Exo‑System v2)

Les systèmes traditionnels s’appuient sur un ensemble large de bibliothèques système : libc pour la compatibilité POSIX, pam pour l’authentification, udev pour la gestion des périphériques, journald/syslog pour la journalisation, init/systemd pour le démarrage des services, etc. Dans Exo‑OS, ces concepts n’existent pas ou sont partiellement implémentés. Le but de cette feuille de route est de guider l’adaptation de ces bibliothèques afin de fournir un environnement familier aux développeurs sans sacrifier le modèle de capabilities.

Bibliothèques concernées
Bibliothèque	Usage attendu	Adaptation requise
musl, relibc	Implémentent la libc (fonctions POSIX, stdio, process, network, math).	Porter en no_std, remplacer tous les appels système par des messages IPC ou des syscalls Exo‑OS, compléter les fonctions manquantes (ex: fork, execve, signaux).
linux‑pam	Gestion modulaire de l’authentification (PAM).	Exo‑OS n’ayant pas de notion d’utilisateur, PAM doit être reconçu pour fonctionner avec des capabilities et un keystore ; probablement inutile à court terme.
libudev‑rs	Détection et gestion des périphériques via udev/netlink.	Adapter pour interroger device_server et recevoir des notifications de hot‑plug via IPC plutôt que via Netlink.
log, tracing	Infrastructure de journalisation.	Reconfigurer pour envoyer les logs vers log_server ou monitor_server via IPC, utiliser des formats structurés et un tampon circulaire.
pkgcraft, cargo-chef	Gestion de paquets et orchestration de build.	Créer un système de paquets propre à Exo‑OS, aligné sur le modèle capability.
launchd, systemd	Gestionnaires de services.	Étendre init_server pour démarrer, arrêter et superviser des services ; une interface compatible systemd n’est pas nécessaire mais certains concepts (unités, dépendances) peuvent être repris.
Adaptation de musl/relibc : créer exo-libc
1. Mapping des appels systèmes

Table des syscalls : Exo‑OS implémente 300+ appels systèmes (numéros 0–299 compatibles Linux, 300–399 pour Exo extensions). Analysez chaque fonction de musl et remplacez le syscall(NR_*, ...) par l’appel approprié (via libexo_syscall ou en envoyant un message IPC à un serveur). Exemple pour open :

int open(const char *pathname, int flags, mode_t mode) {
// Traduire en appel IPC vers vfs_server
return exo_vfs_open(pathname, flags, mode);
}

Fonctions non supportées : certaines fonctions POSIX comme fork(), execve(), kill() n’existent pas dans Exo‑OS. Il faut soit les stuber (renvoyer ENOSYS), soit les re‑définir via des primitives Exo‑OS (par ex. exo_spawn pour créer un nouveau processus). Documentez clairement ces différences pour éviter les comportements inattendus.
2. STDIO et buffers

Les fonctions d’entrée/sortie (printf, fgets, scanf) reposent sur les descripteurs de fichiers (stdin, stdout, stderr). Dans Exo‑OS, un capability identifie un flux. Modifiez les fonctions de musl pour qu’elles stockent un CapFile au lieu d’un int. Exemple :

typedef struct {
    CapFile cap;
    size_t pos;
    size_t end;
    unsigned char buffer[BUFSIZ];
} FILE;

Les appels read() et write() utilisent ensuite ipc_file_read(cap, ...) ou ipc_file_write(cap, ...).

3. Processus et signaux

Exo‑OS n’a pas de fork ni de signaux classiques. La création de processus se fait via un appel create_process() et l’attente via sys_wait(). Pour simuler fork(), exposez un wrapper qui renvoie -ENOSYS et incitez les développeurs à utiliser spawn() avec exec() directement. Pour les signaux, remplacez signal(SIGINT, handler) par un mécanisme d’abonnement à un événement via ipc_subscribe_event().

4. TTY, pseudoterminals et udev

La gestion des terminaux (pty/tty) passe par le device_server. Libudev doit être remplacé par une API Exo‑OS :

dev_list() renvoie la liste des périphériques présents (tty0, sda, etc.)
dev_subscribe() permet de s’abonner aux événements (ajout/suppression).
Les applications doivent utiliser ces primitives plutôt que de lire /dev ou /sys.
5. Journalisation et traçage

log et tracing sont des crates Rust fournissant des macros pour journaliser (info!, warn!) et des outils de traçage asynchrone. Pour Exo‑OS :

Créez un crate exo-log qui implémente le trait log::Log et envoie les entrées au log_server via IPC. Le log_server stocke les messages dans un tampon circulaire et les écrit sur la console ou dans un fichier.
Ajustez tracing pour utiliser exo-log comme collecteur. Évitez d’utiliser des fonctionnalités qui nécessitent std (comme la coloration terminal) ; préférez un format JSON pour l’analyse automatisée.
6. Gestion des paquets et services

Il n’existe pas de gestionnaire de paquets sous Exo‑OS. Les bibliothèques pkgcraft et cargo-chef peuvent servir de base pour développer exo‑pkg, un outil permettant d’empaqueter, de distribuer et d’installer des applications Exo‑OS. Ce gestionnaire devra :

Installer les fichiers dans le VFS (sous /sys/apps par exemple) via le vfs_server.
Enregistrer les services auprès de init_server et configurer les dépendances.
Gérer les mises à jour et la vérification des signatures (via crypto_server).

Pour la gestion des services, inspirez‑vous de launchd et systemd : créer un format exo-service (fichiers .exo) décrivant le binaire, les arguments, les dépendances et les droits de capabilities. init_server lira ces fichiers et lancera les services dans l’ordre approprié.

Pièges et erreurs à éviter
Incohérence des droits : ne supposez pas que les utilisateurs existent. Toutes les opérations doivent vérifier les capabilities fournies. Par exemple, open() doit vérifier que la capability possède le droit READ ou WRITE avant de continuer.
Absence de stub explicite : si une fonction POSIX n’est pas implémentée, renvoyez -ENOSYS et documentez-la. Ne renvoyez pas une valeur par défaut silencieuse, ce qui induirait l’utilisateur en erreur.
Non‑libération des ressources : certains wrappers peuvent allouer des tampons internes sans les libérer (ex. getcwd()). Assurez‑vous de libérer la mémoire via exo-alloc.
Confusion entre int et capability : l’API POSIX manipule des descripteurs entiers. Dans Exo‑OS, ce type doit encapsuler un capability et un index de table des fichiers ; ne pas les mélanger conduit à des accès illégaux.
Suppression du modèle PAM : si vous souhaitez intégrer PAM, vous devrez l’adapter pour vérifier des tokens cryptographiques plutôt que des mots de passe. Une intégration partielle peut être dangereuse si elle laisse des failles d’authentification.
Étapes de réalisation
Créer exo-libc en forkant musl ou relibc et en remplaçant les appels système par des wrappers Exo‑OS. Ajouter les fonctions manquantes en les implémentant dans les serveurs correspondants.
Mettre en place exo-log et adapter les macros log/tracing pour envoyer les messages au serveur. Établir un format d’échange (JSON ou binaire) et définir des niveaux de journalisation.
Écrire exo-udev pour la détection des périphériques. Ce crate fournira des functions watch_devices() renvoyant un flux d’événements (ajout, retrait, modification).
Développer un gestionnaire de services au sein d’init_server capable de lire des descriptors de service, de démarrer les programmes userland avec les bonnes capabilities et de gérer leur redémarrage en cas de crash.
Concevoir exo-pkg pour la distribution et l’installation des logiciels. Intégrer des vérifications de signature via crypto_server et mettre à jour vfs_server pour gérer les paquets.
Conclusion

Cette feuille de route propose un chemin clair pour enrichir Exo‑OS avec des bibliothèques système comparables à celles de Linux/Windows. En adaptant soigneusement les libc existantes, en créant des couches d’abstraction pour les périphériques et les services et en adoptant une approche basée sur les capabilities, Exo‑OS pourra offrir un environnement familier tout en préservant sa sécurité intrinsèque. Les exemples de code et les avertissements cités doivent permettre d’éviter des erreurs silencieuses lors de l’intégration.

Runtimes et concurrence (exo‑runtime) :
Feuille de route d'adaptation des runtimes et de la concurrence (Exo‑Runtime v2)

Pour porter des applications modernes sur Exo‑OS, il est nécessaire de disposer d’un runtime asynchrone et d’outils de concurrence comparables à ceux de Linux/Windows. Des crates comme Tokio, async‑std, Rayon ou des gestionnaires de services comme systemd fournissent ces fonctionnalités. Cette feuille de route décrit comment les adapter à l’architecture micro‑noyau d’Exo‑OS et propose des exemples de code.

Bibliothèques et outils étudiés
Catégorie	Bibliothèques	Description
Runtime asynchrone	tokio, async‑std	Fournissent un exécuteur de futures, des sockets TCP/UDP, des timers et des tâches asynchrones ; tokio est réputé pour son scheduler multi‑thread capable de traiter des centaines de milliers de requêtes par seconde.
Concurrence	rayon	Bibliothèque Rust pour le parallélisme des tâches (fork‑join) ; exploite plusieurs cœurs avec un scheduler work‑stealing.
Gestion de services	systemd, launchd	Gèrent le démarrage et la supervision des services. Dans Exo‑OS, ils inspirent la création d’un gestionnaire intégré à init_server.
Journalisation et traçage	tracing, tokio‑tracing	Permettent de collecter des événements, de les structurer et de les exporter.
Adaptation de Tokio / async‑std
1. Désactiver std

Par défaut, tokio et async‑std requièrent std. Pour les compiler sous Exo‑OS :

Dans Cargo.toml, désactivez les fonctionnalités par défaut et activez les modules nécessaires : io-util, net, time, macros.
Ajoutez la dépendance futures pour disposer des traits Future, Stream et Sink.
2. Implémenter un exo-executor

Exo‑OS ne fournit pas d’API POSIX pour le threading ; le scheduler est géré par le micro‑noyau. Il faut donc créer un exécuteur léger basé sur les primitives du scheduler :

Spawner : maintient une file de tâches (VecDeque<Pin<Box<dyn Future<Output=()>>>) et un masque d’événements. Chaque fois que la file n’est pas vide, le spawner récupère une tâche et l’exécute jusqu’à ce qu’elle se bloque sur une attente (poll retourne Pending). Ensuite, il passe à la tâche suivante.
Yield : lorsque toutes les tâches sont en attente et qu’aucun événement n’est disponible, appeler sched_yield() pour céder le CPU.

Enregistrement d’événements : les sockets et timers doivent s’enregistrer auprès du scheduler pour être réveillés lorsque des données sont prêtes. Cela nécessite d’exposer des hooks dans ipc et vfs (voir la correction de la boucle active du scheduler). Exemple de structure :

struct ExoExecutor {
queue: VecDeque<Pin<Box<dyn Future<Output = ()>>>>,
waker: ExoWakerRegistry,
}

impl ExoExecutor {
fn run(&mut self) {
loop {
while let Some(mut task) = self.queue.pop_front() {
match task.as_mut().poll(&mut Context::from_waker(self.waker.waker())) {
Poll::Ready(_) => (),
Poll::Pending => self.queue.push_back(task),
}
}
// Si aucune tâche n'est prête, attendre un événement
if !self.waker.has_events() {
sched_yield();
}
}
}
}

3. Adapter les primitives I/O

tokio::net::TcpStream repose sur les sockets POSIX. Dans Exo‑OS, remplacez‑le par ExoTcpStream qui utilise le service exo-net (voir exo_net_adaptation_v2.md) ; implémentez les traits AsyncRead et AsyncWrite de futures. Pour la lecture bloquante :

impl AsyncRead for ExoTcpStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<usize, std::io::Error>> {
        match ipc_recv(self.cap, buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(IpcError::WouldBlock) => {
                // enregistrer le waker pour réveiller la tâche lorsque des données arrivent
                self.register_read_waker(cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(std::io::Error::from(e))),
        }
    }
}

Même approche pour AsyncWrite. L’adaptateur doit utiliser des messages IPC asynchrones afin de ne pas bloquer la boucle.

4. Adoption d’async‑std

async‑std fournit un API similaire mais repose sur un runtime différent. Vous pouvez créer un adaptateur qui exécute les futures via exo-executor et fournit des équivalents de async_std::task::spawn, sleep, TcpStream, etc. L’intégration du scheduler Exo‑OS est similaire : les tâches s’inscrivent auprès du scheduler et se réveillent sur des événements.

5. Parallélisme avec rayon

rayon permet d’exécuter en parallèle des boucles de calcul intensif. Pour l’utiliser :

Compilez rayon avec la feature no_std et fournissez un planificateur ThreadPool basé sur le scheduler Exo‑OS. Chaque « thread » rayon doit correspondre à un processus léger géré par Exo‑OS.
Limitez le nombre de threads au nombre de cœurs disponibles (exposé par sys_get_cpus()), afin de ne pas saturer le scheduler.
6. Gestion des services et timers

Pour remplacer systemd, init_server doit être étendu avec un gestionnaire de services qui :

Lit des fichiers .exo décrivant les programmes à lancer, leurs dépendances et les capabilities requises.
Lance chaque service via create_process() avec les arguments et capabilities nécessaires.
Supervise chaque service et le redémarre en cas de crash.
Fournit une API IPC pour démarrer/arrêter/recharger un service à la demande.

Les timers (sleep, interval) doivent être implémentés via le scheduler du noyau, en utilisant les appels système existants (par ex. sys_timer_create s’il existe) ou en ajoutant un module timer_server.

Pièges et pratiques à éviter
Bloquer la boucle : ne jamais appeler std::thread::sleep ou réaliser une lecture bloquante ; utilisez sched_yield() ou les primitives asynchrones du scheduler.
Waker non enregistré : si vous oubliez d’enregistrer le waker lors d’un Pending, la tâche ne sera jamais réveillée. Vérifiez toujours que les wakers sont stockés dans un registre et déclenchés par les événements correspondants.
Mélanger plusieurs runtimes : éviter d’utiliser simultanément tokio et async‑std dans le même binaire. Choisissez un runtime et fournissez des adaptateurs pour les bibliothèques écrites pour l’autre.
Fuite de tâches : si une tâche asynchrone est abandonnée sans être await, elle peut rester dans la file et consommer des ressources. Utilisez des structures comme select! ou join! pour attendre explicitement la fin des futures.
Exploitation CPU : ne créez pas plus de threads rayon que de cœurs ; sinon, le scheduler d’Exo‑OS passera son temps à commuter, dégradant les performances.
Étapes finales
Développer exo-executor et proposer un crate exo-runtime qui expose spawn, block_on, sleep, etc. Documenter son utilisation pour remplacer tokio::main ou async_std::main.
Créer des adaptateurs I/O (ExoTcpStream, ExoUdpSocket, etc.) qui implémentent les traits AsyncRead/AsyncWrite/AsyncDatagram et utilisent exo-net.
Porter les bibliothèques dépendantes de tokio (hyper, axum) en les compilant avec default-features = false et en injectant nos adaptateurs.
Écrire des exemples : un serveur HTTP simple avec hyper et exo-runtime, et un client DNS utilisant hickory-dns et exo-net.
Conclusion

Cette feuille de route décrit comment offrir un environnement asynchrone et concurrentiel performant dans Exo‑OS. En adaptant les runtimes existants et en fournissant un exécuteur intégré au scheduler du noyau, il devient possible d’exécuter des applications réseau, des traitements parallèles et des services de fond tout en respectant le modèle de micro‑noyau. Les exemples de code et les pièges identifiés aideront à éviter des comportements bloquants ou des tâches fantômes.

Interface utilisateur (exo‑ui) :
Feuille de route d'adaptation de l'interface utilisateur (Exo‑UI v2)

Bien qu’Exo‑OS soit conçu comme un système micro‑noyau minimaliste, il est possible de développer des interfaces graphiques modernes en s’appuyant sur des bibliothèques Rust telles que winit, wgpu et iced. Cette feuille de route explique comment adapter ces bibliothèques au contexte Exo‑OS et fournit des exemples de code et de pièges à éviter.

Bibliothèques et buts
Bibliothèque	Usage	Adaptation
winit	Gestion des fenêtres et des événements (keyboard, mouse, touch).	Nécessite un backend personnalisé pour récupérer les événements via gui_server (Exo‑OS) au lieu de X11/Wayland.
wgpu	Abstraction cross‑platform pour les GPU (Vulkan/Metal/DX).	Doit être compilée en no_std avec un backend personnalisé qui envoie des commandes au driver GPU via device_server.
iced	Framework GUI basé sur wgpu et winit, composable et réactif.	Peut fonctionner au-dessus de nos adaptateurs winit/wgpu ; nécessite un executor asynchrone (voir exo-runtime).
Architecture d'intégration
1. gui_server et capabilities graphiques

Exo‑OS n’expose pas directement le matériel graphique en userland. Le device_server inclut un sous-système graphique et un serveur gui_server pourrait être responsable :

Création de surfaces : les applications demandent une surface via un message IPC et reçoivent une capability CapSurface. Cette surface représente une zone d’affichage.
Gestion des événements : le gui_server délivre des événements (clavier, souris, fenêtre) via un canal IPC asynchrone. L’application doit s’inscrire et récupérer les événements à l’aide de ipc_gui_recv().
Rendu : les commandes de rendu sont envoyées via ipc_gpu_submit() au driver graphique. Les applications ne peuvent pas accéder directement à la mémoire vidéo.
2. Adaptation de winit

winit gère la création de fenêtres et le dispatch des événements pour divers backends (X11, Wayland, Win32). Pour Exo‑OS :

Créer un backend exo : implémentez les traits winit::platform::EventLoopExtExo et WindowExtExo qui allouent une fenêtre via gui_server et envoient/receivent des événements via IPC.

Boucle d’événements : modifiez EventLoop::run pour ne pas bloquer sur std::thread::sleep mais utiliser le scheduler et sched_yield() lorsque la file d’événements est vide. Exemple :

loop {
while let Some(event) = ipc_gui_recv(self.event_cap) {
callback(event);
}
// aucune fenêtre ouverte ? sortir
if self.windows.is_empty() { break; }
sched_yield();
}

Gestion de la DPI et redimensionnement : récupérer les informations de DPI via le gui_server et ajuster le contenu en conséquence.
3. Adaptation de wgpu

wgpu abstrait les API graphiques (Vulkan, Metal, DX12) et communique avec le driver GPU via la couche HAL de wgpu. Sous Exo‑OS :

Backend personnalisé : implémentez un backend minimal exo-hal qui convertit les appels de wgpu (création de buffers, textures, pipelines) en commandes IPC vers le device_server. Ce backend doit gérer la synchronisation et les retours d’erreur.
Mémoire partagée : pour de meilleures performances, utilisez des zones partagées allouées via ipc_map_shared_memory() où l’application écrit directement ses vertices et textures, puis notifiez le device_server.
Gestion des queues de rendu : wgpu utilise des CommandEncoder et RenderPass. Dans l’adaptateur, sérialisez ces commandes en un format binaire compact et envoyez‑les à la couche kernel.
4. Utilisation d’iced

iced offre un framework déclaratif (style Elm) pour créer des interfaces réactives. Pour l’utiliser :

Exécuteur : iced nécessite un runtime asynchrone ; utilisez exo-runtime pour exécuter l’application.
Backend graphique : compilez iced avec le backend wgpu et désactivez le backend glow (OpenGL). Fournissez vos adaptateurs winit/wgpu via les traits Application::executor et Application::new.

Exemple de code :

struct CounterApp;

impl iced::Application for CounterApp {
type Executor = ExoExecutor; // défini dans exo-runtime
type Message = Msg;
type Theme = iced::Theme;

 fn new() -> (Self, Command<Self::Message>) {
     (Self, Command::none())
 }
 fn title(&self) -> String { String::from("Compteur Exo") }
 fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
     // logique de mise à jour
     Command::none()
 }
 fn view(&self) -> Element<Self::Message> {
     // construction de l'UI
 }

}

fn main() {
CounterApp::run(Settings::default().with_id(1 /* CapSurface id */)).unwrap();
}

Remplacez Settings::default() par une structure contenant la capability de la surface.

5. Pièges et erreurs à éviter
Blocage sur les événements : n’utilisez jamais une attente bloquante sur les événements (comme std::thread::sleep). Utilisez sched_yield() pour céder le CPU lorsque la file est vide.
Mémoire vidéo : n’allouez pas directement de tampons via mmap pour la vidéo ; utilisez la mémoire partagée fournie par device_server et respectez les limites de la carte graphique.
Fuites d’événements : si vous ignorez certains événements (par exemple Resized), la file peut se remplir et provoquer une saturation. Détruisez ou actualisez les surfaces correspondantes.
Compatibilité : toutes les bibliothèques doivent être compilées avec default-features = false pour désactiver les dépendances à std (par exemple winit supporte un backend no_std limité). Évitez d’utiliser des modules qui nécessitent parking_lot ou std::sync.
Étapes de développement
Implémenter le serveur graphique (gui_server) pour gérer les surfaces, les événements et la composition. Définir les messages IPC nécessaires (CreateSurface, Present, GetEvents).
Créer les backends exo pour winit et wgpu. Cela implique de définir des traits spécifiques et d’enregistrer les adaptateurs dans winit. Tester la création d’une fenêtre, la gestion des événements et le rendu d’un triangle via wgpu.
Porter iced en utilisant vos backends. Corriger les appels bloquants et adapter l’executor. Tester un exemple simple (bouton + compteur) et vérifier la fluidité.
Intégrer le son et l’entrée : si nécessaire, adapter des bibliothèques telles que cpal (son) ou gilrs (manettes) en créant des serveurs dédiés et des adaptateurs.
Optimisations : mesurer la latence d’affichage, réduire le nombre de copies mémoire et optimiser la sérialisation des commandes GPU.
Conclusion

Bien que l’intégration de bibliothèques graphiques dans un micro‑noyau soit complexe, cette feuille de route montre qu’il est possible de porter des frameworks modernes (winit, wgpu, iced) sur Exo‑OS. En créant des backends spécifiques et en respectant les contraintes du scheduler, vous pourrez développer des interfaces réactives et sécurisées sans compromettre la stabilité du système. Les exemples fournis et les pièges identifiés faciliteront votre mise en œuvre et éviteront des erreurs silencieuses.

Bibliothèques diverses (exo‑misc) :
Feuille de route d'adaptation des bibliothèques diverses (Exo‑Misc v2)

Outre les grandes catégories (réseau, cryptographie, systèmes de fichiers, allocateurs, bibliothèques système, runtime, UI), Exo‑OS devra intégrer plusieurs bibliothèques complémentaires pour être pleinement utilisable. Cette feuille de route décrit l’adaptation de quelques crates importantes telles que axum, cargo-chef, zbus, pkgcraft, rtnetlink et d’autres utilitaires.

1. Frameworks web et HTTP
1.1 axum

axum est un framework web basé sur hyper et tokio. Pour l’utiliser sous Exo‑OS :

Compilez axum avec default-features = false et activez uniquement les fonctionnalités nécessaires (headers, form, json).
Utilisez exo-runtime comme exécuteur (voir exo_runtime_adaptation_v2.md). axum::Server peut être instancié avec hyper::server::Server::builder(exo_listener) où exo_listener fournit des connexions ExoTcpStream via exo-net.
Évitez les middlewares qui utilisent tokio::fs ou tokio::signal; remplacez-les par des implémentations basées sur exo-fs et exo-event.
1.2 hyper

hyper peut être utilisé directement pour construire des clients HTTP/1.1 et HTTP/2. L’adaptation consiste à :

Remplacer tokio::net::TcpStream par ExoTcpStream.
Désactiver la feature http2 si vous ne prenez pas en charge HTTP/2 (complexe sans ALPN/TLS complet).
Utiliser rustls via exo-crypto pour le TLS.
2. Outils de build et de paquets
2.1 cargo-chef

cargo-chef est un outil pour générer des couches de dépendances Docker. Dans Exo‑OS, il pourrait servir à optimiser la construction de paquets. Adaptation :

Exécuter cargo chef prepare et cargo chef cook dans un conteneur cross‑compilé ; ajuster les chemins d’output pour cibler exo-libc.
Développer un outil exo-chef qui automatise la préparation des builds et l’empaquetage via exo-pkg.
2.2 pkgcraft

pkgcraft est une suite d’outils pour la gestion de paquets (parsing ebuilds). On peut s’en inspirer pour exo‑pkg. L’intégration consiste à :

Écrire un parseur de manifeste exo.toml décrivant les métadonnées du paquet (nom, version, dépendances, capabilities requises).
Utiliser pkgcraft pour valider les dépendances et les contraintes de version.
Intégrer exo‑pkg au init_server et au vfs_server pour installer et enregistrer les paquets.
3. Bus de messages et RPC
3.1 zbus

zbus est une bibliothèque Rust implémentant le protocole D‑Bus. Sous Exo‑OS, D‑Bus n’existe pas, mais zbus peut être utilisé pour construire un bus de messages léger entre applications userland :

Remplacez la couche de transport de zbus par un backend exo-ipc utilisant les appels IPC Exo‑OS. L’API haute niveau (proxy, signal) reste similaire.
Créez un bus_server ring 1 chargé d’acheminer les messages, d’enregistrer les noms de services et de gérer les permissions via des capabilities. Les clients s’y connectent en appelant connect_bus() et reçoivent une capability représentant leur session.

Exemple d’usage :

let conn = zbus::ConnectionBuilder::unix_stream(exo_bus_stream).serve().await?;
conn.object_server().at("/org/exo/MyApp", object).await?;

Implémentez un superviseur qui redémarre le bus_server en cas de crash pour éviter que des applications se retrouvent bloquées.
3.2 Autres RPC

Certaines bibliothèques (par ex. gRPC, MQTT) peuvent être portées de manière similaire en remplaçant leur transport par des messages IPC Exo‑OS. Documentez les limitations (taille des messages, absence de protocole stream) et proposez des solutions (découpage en fragments, multiplexer).

4. Configuration réseau avancée

Le crate rtnetlink permet d’envoyer des messages netlink sous Linux pour configurer les interfaces réseau. Dans Exo‑OS, cette fonctionnalité est assurée par network_server. Pour l’utiliser :

Adapter rtnetlink pour envoyer des requêtes AddRoute, DelAddr, etc., via IPC. Par exemple, ip route add devient un appel net_server_add_route(dest, gateway, mask).
Fournir un wrapper exo-netlink qui expose un API compatible avec la CLI Linux (ip addr, ip link). Les appels doivent être traduits en messages au serveur réseau et renvoyer des codes d’erreur clairs.
5. Pièges et conseils divers
Crochet de build : ne supposiez pas que std::process::Command est disponible ; utilisez exo-libc::exec pour lancer des processus lors du build.
Chemins codés en dur : évitez d’utiliser /etc ou /usr; définissez des chemins virtuels dans le VFS (/sys/config, /usr/apps) et créez des symlinks si nécessaire.
Non respect des capabilities : lors de la construction d’un framework RPC, vérifiez toujours les droits du caller. Une procédure publique ne doit pas accéder à un service interne sans un token approprié.
Dépendance à std : compilez chaque crate avec default-features = false. Si la bibliothèque n’est pas no_std, envisagez d’écrire un remplacement minimal plutôt que de la porter entièrement.
Conclusion

L’intégration de ces bibliothèques diverses enrichira l’écosystème Exo‑OS et rapprochera ses fonctionnalités de celles d’un système complet. En adaptant les frameworks web, les outils de build, les bus de messages et la configuration réseau aux primitives Exo‑OS, vous fournirez un environnement cohérent et sécurisé. Les exemples et les mises en garde présentés ici aideront à prévenir les erreurs silencieuses et à garantir une intégration fluide.