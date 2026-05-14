Panorama des serveurs et bibliothèques : point de maturité et axes d’évolution
Introduction

Cette synthèse dresse un état des lieux des serveurs et bibliothèques d’Exo‑OS en
comparaison avec les fonctionnalités des systèmes d’exploitation matures comme
Linux ou Windows. Elle met en avant les atouts spécifiques d’Exo‑OS (modèle
capability, ExoShield, ExoPhoenix, API asynchrone) tout en identifiant les
fonctionnalités manquantes qu’il serait souhaitable d’implémenter pour
approcher la richesse fonctionnelle des plates‑formes traditionnelles. Les
constats s’appuient sur les audits précédents et sur la
documentation du projet.

Atouts fondamentaux d’Exo‑OS

Exo‑OS se distingue par une architecture modulaire et sécurisée :

Modèle de sécurité par capabilities : l’accès aux objets (fichiers,
mémoire partagée, sockets, IPC) est contrôlé par des captokens. Il n’y a
ni notion d’utilisateur, ni bits rwx ou ACL ; l’autorisation est
déterminée par le porteur du token. Ce choix supprime une grande partie des
attaques de type time‑of‑check vs time‑of‑use et élimine la gestion de
super‑utilisateurs, mais impose une translation pour émuler des
permissions POSIX classiques. Les rapports précédents ont mis en avant
l’importance de vérifications en temps constant des capacités afin d’éviter
des fuites par canaux auxiliaires.
Isolation forte par ExoShield/ExoVeil : le noyau et les services ring 1
bénéficient d’un ensemble de mesures (PKS, CET, journal immuable,
keystore centralisé). Le drapeau SECURITY_READY garantit que les cœurs
d’application ne démarrent qu’après l’initialisation de toutes les
protections. ExoShield fournit en outre un
serveur de confinement qui surveille les processus et applique des
politiques de sécurité.
Résilience via ExoPhoenix : la capacité à suspendre le noyau A, à le
remplacer par un noyau B et à restaurer l’état depuis un shared sentinel
region (SSR) confère à Exo‑OS un mécanisme de reprise après erreur
comparable à l’hibernation et au hot‑swapping de micro‑kernels modernes.
Le protocole SSR est décrit et mis en œuvre dans la lib partagée
exo-phoenix-ssr avec support de 256 cœurs et un plan de restitution
sécurisé【179340715623637†L17-L33】.
IPC asynchrone : les corrections récentes ont supprimé les boucles
actives et installé des hooks scheduler/VMM dans l’IPC. La file d’attente
IpcWaitQueue bloque maintenant réellement les threads après quelques
itérations et utilise un AtomicU64 pour l’identifiant de thread, ce qui
réduit fortement la latence de communication.
Crypto centralisé : toutes les primitives cryptographiques sont
offertes par un serveur dédié (crypto_server), qui expose une API
uniforme pour la signature, le hachage et la dérivation de clés. Le noyau
et les serveurs n’implémentent donc pas leur propre cryptographie,
réduisant la surface d’attaque.

Ces éléments constituent une base solide pour construire un système puissant et
sécurisé. Toutefois, pour atteindre une fonctionnalité globale comparable à
Linux ou Windows, plusieurs extensions sont nécessaires.

État des serveurs Ring 1
memory_server (gestion de mémoire)

Couverture actuelle : alloue et libère des pages, fournit du partage de
mémoire anonyme et du copy‑on‑write, et gère des quotas simples. Son API
est orientée capacités : un processus doit présenter un captoken de type
CapType::Memory pour obtenir de nouvelles pages. Des optimisations ont été
apportées pour éviter les allocations dans les interruptions et pour
protéger le verrou global CoW, mais le OOM killer reste minimaliste et
quelques appels utilisent encore des unwrap().

Fonctionnalités manquantes pour atteindre la parité :

Overcommit et mémoire virtuelle : les systèmes Unix/Windows utilisent
une mémoire virtuelle illimitée avec sur‑allocation et swap. Exo‑OS ne
dispose pas de mécanisme de swap ou d’échange vers disque ; l’allocation
échoue lorsque la mémoire physique est épuisée.
Gestion fine des privilèges mémoire : pas de protection read-only
sur les pages partagées, absence de guard pages ou de mmap dynamique à
la manière de mprotect/munmap. Un portage d’une API mmap sur le
modèle POSIX et la prise en charge de la pagination en espace utilisateur
seraient nécessaires.
Contrôle de ressources par cgroups : pas de support pour isoler les
groupes de processus et limiter leur consommation (CPU/ressources mémoire)
comme les cgroups Linux.
network_server (réseau)

Couverture actuelle : support de base pour la pile IP (sockets
abstraits) et quelques notions d’endpoints. L’API permet de créer un
socket, envoyer et recevoir des paquets via IPC. Les corrections
récentes imposent un délai pour les appels IPC afin d’éviter les blocages
indéfinis.

Fonctionnalités manquantes :

Pile TCP/IP complète : la prise en charge de protocoles comme TCP,
UDP, ICMP, DHCP, DNS, NAT et IPv6 est minimale ou inexistante.
TLS/SSL : l’établissement de connexions sécurisées n’est pas câblé dans
le serveur réseau ; cela devrait être délégué au crypto_server mais
l’interface reste à définir.
Paramétrage dynamique : absence d’API pour configurer les interfaces,
l’adressage, les routes et les pare‑feux. Un équivalent de ifconfig ou
netsh est nécessaire.
scheduler_server (ordonnanceur)

Couverture actuelle : offre des services d’ordonnancement temps réel.
Après correction, il valide les paramètres de période et de durée pour
éviter des divisions par zéro et interagit avec le noyau pour la mise en
file de nouvelles tâches【699009319511083†L1-L20】. Les threads sont
classés par priorité et l’ordonnancement adopte une politique inspirée du
Completely Fair Scheduler.

Fonctionnalités manquantes :

Groupes de processus et priorité fixe : les systèmes classiques
disposent de cgroups et de priorités niceness. Exo‑OS n’implémente
qu’une priorité par thread sans notion de groupe ou de hiérarchie.
Événements d’ordonnancement POSIX : signaux SIGSTOP, SIGCONT,
SIGKILL n’existent pas ; il n’y a pas de gestion de signaux
inter‑processus.
Lissage des temps de tranches : les constantes CFS (latence cible et
granularité) sont figées et ne s’adaptent pas au nombre de cœurs
disponibles. Un ajustement dynamique serait
nécessaire pour offrir un comportement semblable à Linux.
device_server (périphériques)

Couverture actuelle : centralise les accès aux périphériques PCI et
IOMMU. Le serveur fournit une API pour enregistrer un handler d’interruption
et allouer un périphérique. Les audits ont mis en évidence des bogues sur la
purge de handlers et la réinitialisation de compteurs qui ont depuis été
corrigés.

Fonctionnalités manquantes :

Découverte et hot‑plug : pas de détection dynamique des périphériques
connectés, absence de gestion des bus modernes (USB, Thunderbolt). Une
couche équivalente à udev ou au service de gestion des périphériques
Windows est indispensable pour une expérience utilisateur complète.
Interface pilote générique : pas de module de pilotes chargeables à
chaud. Les pilotes sont compilés en dur et doivent être intégrés au
serveur. La prise en charge de pilotes binaires ou écrits par des tiers
est à concevoir.
crypto_server

Couverture actuelle : propose des primitives de hachage, de signature et
de génération de clés. Les rapports précédents soulignent que cette centralisation
est l’un des points forts d’Exo‑OS car elle évite la duplication des
implémentations et permet de certifier la conformité FIPS.

Fonctionnalités manquantes :

Support algorithmique étendu : algorithmes de chiffrement symétrique
(AES‑GCM, ChaCha20‑Poly1305), RSA, ECDSA, PBKDF2/Argon2, aléas
cryptographiquement sûrs. Les algorithmes actuels (Ed25519, Blake3) sont
présents mais insuffisants pour certains cas d’usage.
Gestion de clés persistantes : il manque un service d’enregistrement
durable de clés (coffre HSM) et la rotation automatique de clés selon des
politiques de sécurité.
Interopérabilité : l’API n’expose pas encore de formats
standardisés (PKCS#11, JWK) ni d’interfaçage avec TLS dans le serveur
réseau.
vfs_server et ExoFS

Couverture actuelle : ExoFS est un système de fichiers append‑only avec
grands blobs et cache. Les fonctions vfs_read/vfs_write ont été
correctement alignées avec le cache BLOB ; vfs_rmdir vérifie désormais que
le dossier est vide avant suppression. Une
implémentation d’I/O asynchrones (io_uring) existe et permet de réaliser
des synchronisations plus rapides.

Fonctionnalités manquantes :

Multi‑systèmes de fichiers : pas de support pour divers formats (FAT,
ext4, NTFS, squashfs). Les systèmes Windows et Linux autorisent le
montage de nombreux types de volumes ; ExoFS reste unique.
Gestion des droits : sans notion de propriétaire ni d’ACL, il est
difficile de reproduire les commandes chmod ou icacls. Une couche
d’interface pourrait traduire les modèles POSIX en capabilities tout en
respectant la sécurité d’Exo‑OS.
Fonctions avancées : opérations atomiques (rename, fsync rapide),
instantanés (snapshots), quotas utilisateurs, compression, journaling et
recherche d’index ; autant de fonctionnalités présentes dans ext4/ZFS ou
NTFS qui manquent encore.
exo_shield (serveur de confinement)

Couverture actuelle : applique des politiques de confinement basées sur
des signatures et un moteur comportemental. Il gère un sandbox
application, effectue du forensics et surveille les IPC. Le serveur
utilise des signatures locales mais devrait déléguer toute cryptographie au
crypto_server【179340715623637†L44-L52】.

Fonctionnalités manquantes :

Convergence avec la spec ExoShield v1.0 : les invariants de boot
(IOMMU, PKS, CET, handoff) décrits dans la spec ne se retrouvent pas dans
ce serveur, qui agit surtout comme un « antivirus » ring 1. Il faudrait
soit rebaptiser le serveur pour éviter la confusion, soit intégrer les
fonctionnalités de boot dans ce module.
Gestion centralisée des politiques : l’interface d’administration
(chargement de règles, mise à jour des signatures, notification) est
encore rudimentaire. Des outils de gestion équivalents à Windows
Defender/SELinux seraient nécessaires.
État des bibliothèques
musl‑exo / exo‑libc

Couverture actuelle : Exo‑OS embarque une version adaptée de musl
(musl-exo) qui fournit l’essentiel de la norme C (ISO C99 et POSIX 2008
base). Ce socle permet de compiler des applications C/POSIX, mais de
nombreuses fonctions avancées (interfaces system V IPC, locales, dynamique
linker) ne sont pas exposées.

Fonctionnalités manquantes pour la compatibilité :

Linker et chargement dynamique : le chargement de bibliothèques
partagées (dlopen, dlsym) n’est pas disponible ; Exo‑OS ne propose
actuellement qu’une liaison statique. Les systèmes modernes utilisent un
chargeur dynamique (ld.so) pour charger les modules partagés.
Fonctions système étendues : la majeure partie des appels POSIX
manquants identifiés dans l’audit syscalls (près de 240 manquants dans
le noyau) nécessitent des wrappers libc. Parmi eux : gestion de
processus (fork, execve, posix_spawn), signaux (sigaction,
sigwait), manipulations d’utilisateurs/groupes (getuid, setgid),
temporisateurs (timer_create, timerfd), et les API de threads
(pthread_barrier, pthread_cancel).
Compatibilité Windows : Exo‑OS ne propose pas de bibliothèque Win32 ou
d’émulation WSL. Une surcouche POSIX enrichie ou un sous‑système
d’émulation (type Wine/NT) serait nécessaire pour exécuter des
applications Windows.
exo‑alloc et generic‑rt

Couverture actuelle : exo-alloc expose des allocateurs adaptés au
noyau et au userland ; generic-rt fournit une runtime Rust minimaliste.
Les primitives de multi‑threading sont présentes (mutex, RwLock, channels),
mais manquent de fonctionnalités haut niveau (sémaphores nommés, file de
messages persistante).

Fonctionnalités manquantes :

Garbage‑collector optionnel : certains langages (Go, Java) supposent
un GC natif. Il serait envisageable d’intégrer un runtime gérant un GC
concurrent pour ces langages.
Asynchronisme complet : l’écosystème Rust standard repose sur
tokio/async-std et mio pour l’I/O non bloquante. Exo‑OS devrait
exposer un équivalent ou un adaptateur de io_uring pour servir de
backend.
Primitives de synchronisation avancées : événements, condition
variables avec timeout, futexes et semaphores référençables par nom.
exo-phoenix-ssr

Couverture actuelle : fournit les structures et offsets du Shared
Sentinel Region ainsi que les appels pour freezer et restaurer le noyau.
Il supporte 256 cœurs et implémente la détection et l’acquittement de
freeze events【179340715623637†L17-L33】.

Fonctionnalités manquantes :

Réveil et reseed complet : les spécifications mentionnent un
“Phoenix reseed” après la restauration, pour recharger l’entropie du
générateur de nombres aléatoires et nettoyer le contexte. Cette étape
n’est pas visible dans le code actuel.
Support de disques multiples : la restauration de l’image ExoFS se
limite à un volume ; un système complet devrait pouvoir restaurer
plusieurs partitions ou des instantanés différenciés.
Synthèse et recommandations

Exo‑OS présente une base innovante qui se démarque des OS classiques par son
modèle de sécurité, son isolation forte et sa résilience intégrée. Néanmoins,
pour offrir une expérience comparable à Linux ou Windows tout en préservant
ses atouts, le système devra évoluer sur plusieurs axes :

API de compatibilité POSIX/Win32 : introduire une couche
d’émulation ou de traduction pour permettre aux applications existantes
(qui supposent des signaux, des groupes utilisateurs, des permissions
rwx, etc.) de fonctionner sans réécriture. Cette couche peut se
traduire par une bibliothèque userland qui convertit les notions
traditionnelles en captokens.
Réseau et pile protocolaire complète : élargir le support du
network_server à TCP, UDP et IPv6, introduire des API de configuration
dynamiques et intégrer TLS via le crypto_server.
Gestion avancée du stockage : permettre le montage de plusieurs
systèmes de fichiers et offrir des fonctionnalités comme les
snapshots, les quotas et les ACL. La translation des ACL en
capabilities devra être soigneusement conçue pour rester fidèle au
modèle Exo‑OS.
Device management dynamique : ajouter un démon de découverte de
périphériques et un modèle de chargement à chaud des pilotes (éventuellement
via des WebAssembly modules confinés). Cela améliorerait la compatibilité
matérielle et l’expérience utilisateur.
Bibliothèques et runtime : enrichir musl-exo pour couvrir les API
POSIX manquantes, fournir un chargeur dynamique, et intégrer des
bibliothèques de haut niveau (graphisme, audio, réseau) afin de
simplifier le développement applicatif.
Outils et services système : développer des services d’horloge
(NTP), de journalisation (syslog), de gestion de services (init/supervisor)
et un gestionnaire de paquets pour distribuer et mettre à jour
logiciels et modules.

En suivant ces orientations, Exo‑OS pourrait conserver son modèle unique
tout en offrant un ensemble de fonctionnalités riche et familier aux
développeurs venant des environnements Linux ou Windows.