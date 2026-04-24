//! # Process Profiler — Profilage comportemental des processus
//!
//! Suit le comportement de chaque processus par PID (max 32 profils) :
//! - Fréquence des appels système (256 entrées)
//! - Patrons d'accès mémoire
//! - Activité réseau
//! - Graphe d'appels IPC
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de profils de processus.
pub const MAX_PROFILES: usize = 32;

/// Nombre d'entrées dans le tableau de fréquence des syscalls.
pub const SYSCALL_FREQ_SIZE: usize = 256;

/// Nombre maximum de régions mémoire trackées par profil.
pub const MAX_MEMORY_REGIONS: usize = 8;

/// Nombre maximum de connexions réseau trackées par profil.
pub const MAX_NETWORK_ENTRIES: usize = 8;

/// Nombre maximum de nœuds dans le graphe IPC.
pub const MAX_IPC_NODES: usize = 16;

/// Nombre maximum d'arêtes dans le graphe IPC.
pub const MAX_IPC_EDGES: usize = 32;

// ── Types de syscall ─────────────────────────────────────────────────────────

/// Catégorie d'appel système.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum SyscallCategory {
    File = 0,
    Memory = 1,
    Network = 2,
    Process = 3,
    Signal = 4,
    Ipc = 5,
    Time = 6,
    Other = 7,
}

impl SyscallCategory {
    /// Classifie un numéro de syscall dans sa catégorie.
    /// Classification Linux/x86_64 simplifiée.
    pub fn from_syscall_nr(nr: u64) -> SyscallCategory {
        match nr {
            0..=19 => SyscallCategory::File,  // read, write, open, close, etc.
            20..=39 => SyscallCategory::File, // more file ops
            40..=59 => SyscallCategory::Process, // pid, execve, etc.
            60..=79 => SyscallCategory::File, // more file ops
            80..=99 => SyscallCategory::File, // more file ops
            100..=119 => SyscallCategory::File, // more file ops
            120..=139 => SyscallCategory::Process, // clone, wait, etc.
            140..=159 => SyscallCategory::File, // more file ops
            160..=179 => SyscallCategory::Ipc, // ipc, shm, etc.
            200..=219 => SyscallCategory::File, // more file ops
            220..=239 => SyscallCategory::File, // more file ops
            240..=259 => SyscallCategory::File, // more file ops
            260..=279 => SyscallCategory::Network, // socket, bind, etc.
            280..=299 => SyscallCategory::Ipc, // ipc extended
            300..=319 => SyscallCategory::Ipc, // ipc router
            _ => SyscallCategory::Other,
        }
    }
}

// ── Région mémoire ───────────────────────────────────────────────────────────

/// Région mémoire accédée par un processus.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemoryRegion {
    /// Adresse de début.
    pub start_addr: u64,
    /// Taille en octets.
    pub size: u64,
    /// Nombre d'accès en lecture.
    pub read_count: u64,
    /// Nombre d'accès en écriture.
    pub write_count: u64,
    /// Nombre d'accès en exécution.
    pub exec_count: u64,
    /// Flags de protection (rwx).
    pub protection: u8,
    /// La région est-elle valide ?
    pub valid: bool,
    /// Réservé.
    _reserved: [u8; 2],
}

impl MemoryRegion {
    pub const fn empty() -> Self {
        Self {
            start_addr: 0,
            size: 0,
            read_count: 0,
            write_count: 0,
            exec_count: 0,
            protection: 0,
            valid: false,
            _reserved: [0; 2],
        }
    }

    /// Calcule le score de suspicion de cette région.
    /// Une région avec beaucoup d'écritures+exécutions est suspecte.
    pub fn suspicion_score(&self) -> u32 {
        if !self.valid {
            return 0;
        }
        let mut score = 0u32;

        // Écriture + exécution = très suspect (shellcode possible)
        if self.write_count > 0 && self.exec_count > 0 {
            score += 100;
        }

        // Beaucoup d'exécutions depuis une région RW
        if self.protection & 0x7 == 0x7 && self.exec_count > 10 {
            score += 50;
        }

        // Taille inhabituelle (< 256 octets avec beaucoup d'accès)
        if self.size < 256 && (self.read_count + self.write_count) > 1000 {
            score += 30;
        }

        score.min(200)
    }
}

// ── Entrée réseau ────────────────────────────────────────────────────────────

/// Activité réseau d'un processus.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetworkEntry {
    /// Adresse IP distante (format réseau, big-endian).
    pub remote_ip: [u8; 4],
    /// Port distant.
    pub remote_port: u16,
    /// Port local.
    pub local_port: u16,
    /// Protocole (6 = TCP, 17 = UDP).
    pub protocol: u8,
    /// Octets envoyés.
    pub bytes_sent: u64,
    /// Octets reçus.
    pub bytes_received: u64,
    /// Nombre de connexions.
    pub connection_count: u32,
    /// La connexion est-elle active ?
    pub active: bool,
    /// Réservé.
    _reserved: [u8; 3],
}

impl NetworkEntry {
    pub const fn empty() -> Self {
        Self {
            remote_ip: [0u8; 4],
            remote_port: 0,
            local_port: 0,
            protocol: 0,
            bytes_sent: 0,
            bytes_received: 0,
            connection_count: 0,
            active: false,
            _reserved: [0; 3],
        }
    }

    /// Calcule le score de suspicion de cette activité réseau.
    pub fn suspicion_score(&self) -> u32 {
        if !self.active {
            return 0;
        }
        let mut score = 0u32;

        // Beaucoup de données envoyées (possible exfiltration)
        if self.bytes_sent > 10_000_000 {
            score += 50;
        } else if self.bytes_sent > 1_000_000 {
            score += 20;
        }

        // Ratio sent/received suspect
        if self.bytes_received > 0 {
            let ratio = self.bytes_sent / self.bytes_received.max(1);
            if ratio > 100 {
                score += 40;
            }
        }

        // Connexions multiples vers le même hôte
        if self.connection_count > 100 {
            score += 30;
        }

        // Port inhabituel (< 1024 ou > 49152 avec beaucoup d'activité)
        if self.remote_port > 49152 && self.bytes_sent > 1_000_000 {
            score += 20;
        }

        score.min(200)
    }
}

// ── Nœud du graphe IPC ───────────────────────────────────────────────────────

/// Nœud du graphe d'appels IPC.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IpcNode {
    /// PID du processus cible.
    pub target_pid: u32,
    /// Nombre de messages envoyés.
    pub msg_sent: u32,
    /// Nombre de messages reçus.
    pub msg_received: u32,
    /// Octets totaux transférés.
    pub bytes_transferred: u64,
    /// Le nœud est-il valide ?
    pub valid: bool,
    /// Réservé.
    _reserved: [u8; 3],
}

impl IpcNode {
    pub const fn empty() -> Self {
        Self {
            target_pid: 0,
            msg_sent: 0,
            msg_received: 0,
            bytes_transferred: 0,
            valid: false,
            _reserved: [0; 3],
        }
    }
}

/// Arête du graphe IPC (connexion entre deux PIDs).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IpcEdge {
    /// PID source.
    pub from_pid: u32,
    /// PID destination.
    pub to_pid: u32,
    /// Fréquence d'appel (par seconde, moyennée).
    pub frequency: u32,
    /// Type de message IPC.
    pub msg_type: u8,
    /// L'arête est-elle valide ?
    pub valid: bool,
    /// Réservé.
    _reserved: [u8; 2],
}

impl IpcEdge {
    pub const fn empty() -> Self {
        Self {
            from_pid: 0,
            to_pid: 0,
            frequency: 0,
            msg_type: 0,
            valid: false,
            _reserved: [0; 2],
        }
    }
}

// ── Profil de processus ──────────────────────────────────────────────────────

/// Profil complet d'un processus.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProcessProfile {
    /// PID du processus.
    pub pid: u32,
    /// Fréquence des appels système (indexé par numéro de syscall).
    pub syscall_frequency: [u64; SYSCALL_FREQ_SIZE],
    /// Régions mémoire accédées.
    pub memory_access_pattern: [MemoryRegion; MAX_MEMORY_REGIONS],
    /// Activité réseau.
    pub network_activity: [NetworkEntry; MAX_NETWORK_ENTRIES],
    /// Nœuds du graphe IPC.
    pub ipc_nodes: [IpcNode; MAX_IPC_NODES],
    /// Arêtes du graphe IPC.
    pub ipc_edges: [IpcEdge; MAX_IPC_EDGES],
    /// Nombre total de syscalls.
    pub total_syscalls: u64,
    /// Nombre d'entrées mémoire valides.
    pub memory_region_count: u8,
    /// Nombre d'entrées réseau valides.
    pub network_entry_count: u8,
    /// Nombre de nœuds IPC valides.
    pub ipc_node_count: u8,
    /// Nombre d'arêtes IPC valides.
    pub ipc_edge_count: u8,
    /// Horodatage de création du profil.
    pub created_tsc: u64,
    /// Horodatage de dernière activité.
    pub last_activity_tsc: u64,
    /// Le profil est-il valide ?
    pub valid: bool,
    /// Réservé.
    _reserved: [u8; 3],
}

impl ProcessProfile {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            syscall_frequency: [0u64; SYSCALL_FREQ_SIZE],
            memory_access_pattern: [MemoryRegion::empty(); MAX_MEMORY_REGIONS],
            network_activity: [NetworkEntry::empty(); MAX_NETWORK_ENTRIES],
            ipc_nodes: [IpcNode::empty(); MAX_IPC_NODES],
            ipc_edges: [IpcEdge::empty(); MAX_IPC_EDGES],
            total_syscalls: 0,
            memory_region_count: 0,
            network_entry_count: 0,
            ipc_node_count: 0,
            ipc_edge_count: 0,
            created_tsc: 0,
            last_activity_tsc: 0,
            valid: false,
            _reserved: [0; 3],
        }
    }

    /// Calcule un résumé des fréquences de syscall par catégorie.
    pub fn syscall_category_summary(&self) -> [u64; 8] {
        let mut summary = [0u64; 8];
        for i in 0..SYSCALL_FREQ_SIZE {
            let cat = SyscallCategory::from_syscall_nr(i as u64);
            summary[cat as usize] += self.syscall_frequency[i];
        }
        summary
    }

    /// Calcule le score de suspicion global du profil.
    pub fn suspicion_score(&self) -> u32 {
        let mut score = 0u32;

        // Score mémoire
        for i in 0..MAX_MEMORY_REGIONS {
            score = score.saturating_add(self.memory_access_pattern[i].suspicion_score());
        }

        // Score réseau
        for i in 0..MAX_NETWORK_ENTRIES {
            score = score.saturating_add(self.network_activity[i].suspicion_score());
        }

        // Score IPC : beaucoup de connexions vers des PIDs différents
        if self.ipc_node_count > 10 {
            score = score.saturating_add(30);
        }

        // Score syscall : distribution inhabituelle
        let cat_summary = self.syscall_category_summary();
        let total = cat_summary.iter().sum::<u64>();
        if total > 0 {
            // Beaucoup d'appels réseau par rapport au total
            let net_ratio = (cat_summary[2] * 100) / total.max(1);
            if net_ratio > 80 {
                score = score.saturating_add(40);
            }
            // Beaucoup d'appels IPC
            let ipc_ratio = (cat_summary[5] * 100) / total.max(1);
            if ipc_ratio > 60 {
                score = score.saturating_add(20);
            }
        }

        score.min(1000)
    }
}

// ── Profileur ────────────────────────────────────────────────────────────────

static PROFILER: Mutex<ProfilerInner> = Mutex::new(ProfilerInner::new());

static TOTAL_PROFILES_CREATED: AtomicU64 = AtomicU64::new(0);
static TOTAL_SYSCALLS_TRACKED: AtomicU64 = AtomicU64::new(0);
static TOTAL_NETWORK_EVENTS: AtomicU64 = AtomicU64::new(0);

struct ProfilerInner {
    profiles: [ProcessProfile; MAX_PROFILES],
    profile_count: usize,
}

impl ProfilerInner {
    const fn new() -> Self {
        Self {
            profiles: [ProcessProfile::empty(); MAX_PROFILES],
            profile_count: 0,
        }
    }
}

// ── Lecture TSC ──────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── API publique ─────────────────────────────────────────────────────────────

/// Crée ou récupère un profil pour un PID.
///
/// # Retour
/// - L'index du profil, ou MAX_PROFILES si plein.
pub fn get_or_create_profile(pid: u32) -> usize {
    let mut profiler = PROFILER.lock();

    // Chercher un profil existant
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].valid && profiler.profiles[i].pid == pid {
            return i;
        }
    }

    // Chercher un slot libre
    for i in 0..profiler.profile_count {
        if !profiler.profiles[i].valid {
            profiler.profiles[i] = ProcessProfile {
                pid,
                valid: true,
                created_tsc: read_tsc(),
                last_activity_tsc: read_tsc(),
                ..ProcessProfile::empty()
            };
            TOTAL_PROFILES_CREATED.fetch_add(1, Ordering::Relaxed);
            return i;
        }
    }

    // Créer un nouveau slot
    if profiler.profile_count < MAX_PROFILES {
        let idx = profiler.profile_count;
        profiler.profiles[idx] = ProcessProfile {
            pid,
            valid: true,
            created_tsc: read_tsc(),
            last_activity_tsc: read_tsc(),
            ..ProcessProfile::empty()
        };
        profiler.profile_count += 1;
        TOTAL_PROFILES_CREATED.fetch_add(1, Ordering::Relaxed);
        return idx;
    }

    // Réutiliser le profil le moins récemment actif
    let mut oldest_tsc = u64::MAX;
    let mut oldest_idx = 0;
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].last_activity_tsc < oldest_tsc {
            oldest_tsc = profiler.profiles[i].last_activity_tsc;
            oldest_idx = i;
        }
    }
    profiler.profiles[oldest_idx] = ProcessProfile {
        pid,
        valid: true,
        created_tsc: read_tsc(),
        last_activity_tsc: read_tsc(),
        ..ProcessProfile::empty()
    };
    TOTAL_PROFILES_CREATED.fetch_add(1, Ordering::Relaxed);
    oldest_idx
}

/// Enregistre un appel système pour un processus.
pub fn record_syscall(pid: u32, syscall_nr: u64) {
    let idx = get_or_create_profile(pid);
    if idx >= MAX_PROFILES {
        return;
    }

    let mut profiler = PROFILER.lock();
    let profile = &mut profiler.profiles[idx];

    let nr = syscall_nr as usize;
    if nr < SYSCALL_FREQ_SIZE {
        profile.syscall_frequency[nr] += 1;
    }
    profile.total_syscalls += 1;
    profile.last_activity_tsc = read_tsc();

    TOTAL_SYSCALLS_TRACKED.fetch_add(1, Ordering::Relaxed);
}

/// Enregistre un accès mémoire pour un processus.
pub fn record_memory_access(pid: u32, addr: u64, size: u64, access_type: u8, protection: u8) {
    let idx = get_or_create_profile(pid);
    if idx >= MAX_PROFILES {
        return;
    }

    let mut profiler = PROFILER.lock();
    let profile = &mut profiler.profiles[idx];
    profile.last_activity_tsc = read_tsc();

    // Chercher une région existante qui chevauche
    let region_start = addr & !0xFFF; // Alignement page
    let region_end = (addr + size + 0xFFF) & !0xFFF;

    for i in 0..MAX_MEMORY_REGIONS {
        let region = &mut profile.memory_access_pattern[i];
        if !region.valid {
            // Nouvelle région
            region.start_addr = region_start;
            region.size = region_end - region_start;
            region.protection = protection;
            region.valid = true;
            match access_type {
                0 => region.read_count += 1,
                1 => region.write_count += 1,
                2 => region.exec_count += 1,
                _ => region.read_count += 1,
            }
            profile.memory_region_count = profile
                .memory_region_count
                .saturating_add(1)
                .min(MAX_MEMORY_REGIONS as u8);
            return;
        }

        // Vérifier le chevauchement
        let existing_end = region.start_addr + region.size;
        if region_start < existing_end && region_end > region.start_addr {
            // Chevauchement : mettre à jour les compteurs
            match access_type {
                0 => region.read_count += 1,
                1 => region.write_count += 1,
                2 => region.exec_count += 1,
                _ => region.read_count += 1,
            }
            // Étendre la région si nécessaire
            if region_start < region.start_addr {
                region.size += region.start_addr - region_start;
                region.start_addr = region_start;
            }
            if region_end > existing_end {
                region.size += region_end - existing_end;
            }
            return;
        }
    }

    // Pas de slot libre : remplacer la région la moins accédée
    let mut min_access = u64::MAX;
    let mut min_idx = 0;
    for i in 0..MAX_MEMORY_REGIONS {
        let r = &profile.memory_access_pattern[i];
        let total = r.read_count + r.write_count + r.exec_count;
        if total < min_access {
            min_access = total;
            min_idx = i;
        }
    }
    profile.memory_access_pattern[min_idx] = MemoryRegion {
        start_addr: region_start,
        size: region_end - region_start,
        protection,
        valid: true,
        ..MemoryRegion::empty()
    };
    match access_type {
        0 => profile.memory_access_pattern[min_idx].read_count = 1,
        1 => profile.memory_access_pattern[min_idx].write_count = 1,
        2 => profile.memory_access_pattern[min_idx].exec_count = 1,
        _ => profile.memory_access_pattern[min_idx].read_count = 1,
    }
}

/// Enregistre une activité réseau pour un processus.
pub fn record_network_activity(
    pid: u32,
    remote_ip: [u8; 4],
    remote_port: u16,
    local_port: u16,
    protocol: u8,
    bytes_sent: u64,
    bytes_received: u64,
) {
    let idx = get_or_create_profile(pid);
    if idx >= MAX_PROFILES {
        return;
    }

    let mut profiler = PROFILER.lock();
    let profile = &mut profiler.profiles[idx];
    profile.last_activity_tsc = read_tsc();

    // Chercher une entrée existante pour cette connexion
    for i in 0..MAX_NETWORK_ENTRIES {
        let entry = &mut profile.network_activity[i];
        if !entry.active {
            // Nouvelle entrée
            entry.remote_ip = remote_ip;
            entry.remote_port = remote_port;
            entry.local_port = local_port;
            entry.protocol = protocol;
            entry.bytes_sent = bytes_sent;
            entry.bytes_received = bytes_received;
            entry.connection_count = 1;
            entry.active = true;
            profile.network_entry_count = profile
                .network_entry_count
                .saturating_add(1)
                .min(MAX_NETWORK_ENTRIES as u8);
            TOTAL_NETWORK_EVENTS.fetch_add(1, Ordering::Relaxed);
            return;
        }

        // Vérifier si c'est la même connexion
        if entry.remote_port == remote_port
            && entry.local_port == local_port
            && entry.protocol == protocol
            && entry.remote_ip == remote_ip
        {
            entry.bytes_sent += bytes_sent;
            entry.bytes_received += bytes_received;
            entry.connection_count += 1;
            TOTAL_NETWORK_EVENTS.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    // Pas de slot libre : remplacer l'entrée la moins active
    let mut min_bytes = u64::MAX;
    let mut min_idx = 0;
    for i in 0..MAX_NETWORK_ENTRIES {
        let e = &profile.network_activity[i];
        let total = e.bytes_sent + e.bytes_received;
        if total < min_bytes {
            min_bytes = total;
            min_idx = i;
        }
    }
    profile.network_activity[min_idx] = NetworkEntry {
        remote_ip,
        remote_port,
        local_port,
        protocol,
        bytes_sent,
        bytes_received,
        connection_count: 1,
        active: true,
        _reserved: [0; 3],
    };
    TOTAL_NETWORK_EVENTS.fetch_add(1, Ordering::Relaxed);
}

/// Enregistre un appel IPC pour un processus.
pub fn record_ipc_call(pid: u32, target_pid: u32, msg_type: u8, bytes: u64) {
    let idx = get_or_create_profile(pid);
    if idx >= MAX_PROFILES {
        return;
    }

    let mut profiler = PROFILER.lock();
    let profile = &mut profiler.profiles[idx];
    profile.last_activity_tsc = read_tsc();

    // Mettre à jour les nœuds IPC
    let mut node_found = false;
    for i in 0..MAX_IPC_NODES {
        let node = &mut profile.ipc_nodes[i];
        if node.valid && node.target_pid == target_pid {
            node.msg_sent += 1;
            node.bytes_transferred += bytes;
            node_found = true;
            break;
        }
        if !node.valid {
            node.target_pid = target_pid;
            node.msg_sent = 1;
            node.bytes_transferred = bytes;
            node.valid = true;
            profile.ipc_node_count = profile
                .ipc_node_count
                .saturating_add(1)
                .min(MAX_IPC_NODES as u8);
            node_found = true;
            break;
        }
    }

    if !node_found {
        // Remplacer le nœud le moins actif
        let mut min_sent = u32::MAX;
        let mut min_idx = 0;
        for i in 0..MAX_IPC_NODES {
            if profile.ipc_nodes[i].msg_sent < min_sent {
                min_sent = profile.ipc_nodes[i].msg_sent;
                min_idx = i;
            }
        }
        profile.ipc_nodes[min_idx] = IpcNode {
            target_pid,
            msg_sent: 1,
            msg_received: 0,
            bytes_transferred: bytes,
            valid: true,
            _reserved: [0; 3],
        };
    }

    // Mettre à jour les arêtes IPC
    let mut edge_found = false;
    for i in 0..MAX_IPC_EDGES {
        let edge = &mut profile.ipc_edges[i];
        if edge.valid && edge.from_pid == pid && edge.to_pid == target_pid {
            edge.frequency += 1;
            edge_found = true;
            break;
        }
        if !edge.valid {
            edge.from_pid = pid;
            edge.to_pid = target_pid;
            edge.frequency = 1;
            edge.msg_type = msg_type;
            edge.valid = true;
            profile.ipc_edge_count = profile
                .ipc_edge_count
                .saturating_add(1)
                .min(MAX_IPC_EDGES as u8);
            edge_found = true;
            break;
        }
    }

    if !edge_found {
        let mut min_freq = u32::MAX;
        let mut min_idx = 0;
        for i in 0..MAX_IPC_EDGES {
            if profile.ipc_edges[i].frequency < min_freq {
                min_freq = profile.ipc_edges[i].frequency;
                min_idx = i;
            }
        }
        profile.ipc_edges[min_idx] = IpcEdge {
            from_pid: pid,
            to_pid: target_pid,
            frequency: 1,
            msg_type,
            valid: true,
            _reserved: [0; 2],
        };
    }
}

/// Supprime le profil d'un processus.
pub fn remove_profile(pid: u32) -> bool {
    let mut profiler = PROFILER.lock();
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].valid && profiler.profiles[i].pid == pid {
            profiler.profiles[i] = ProcessProfile::empty();
            return true;
        }
    }
    false
}

/// Récupère une copie du profil d'un processus.
pub fn get_profile(pid: u32) -> Option<ProcessProfile> {
    let profiler = PROFILER.lock();
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].valid && profiler.profiles[i].pid == pid {
            return Some(profiler.profiles[i]);
        }
    }
    None
}

/// Récupère les fréquences de syscall pour un processus.
pub fn get_syscall_frequency(pid: u32) -> Option<[u64; SYSCALL_FREQ_SIZE]> {
    let profiler = PROFILER.lock();
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].valid && profiler.profiles[i].pid == pid {
            return Some(profiler.profiles[i].syscall_frequency);
        }
    }
    None
}

/// Récupère le résumé par catégorie de syscall pour un PID.
pub fn get_syscall_category_summary(pid: u32) -> Option<[u64; 8]> {
    let profiler = PROFILER.lock();
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].valid && profiler.profiles[i].pid == pid {
            return Some(profiler.profiles[i].syscall_category_summary());
        }
    }
    None
}

/// Calcule le score de suspicion d'un processus.
pub fn get_suspicion_score(pid: u32) -> u32 {
    let profiler = PROFILER.lock();
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].valid && profiler.profiles[i].pid == pid {
            return profiler.profiles[i].suspicion_score();
        }
    }
    0
}

/// Statistiques du profileur.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ProfilerStats {
    pub active_profiles: u32,
    pub total_profiles_created: u64,
    pub total_syscalls_tracked: u64,
    pub total_network_events: u64,
}

/// Retourne les statistiques du profileur.
pub fn get_profiler_stats() -> ProfilerStats {
    let profiler = PROFILER.lock();
    let mut active = 0u32;
    for i in 0..profiler.profile_count {
        if profiler.profiles[i].valid {
            active += 1;
        }
    }
    ProfilerStats {
        active_profiles: active,
        total_profiles_created: TOTAL_PROFILES_CREATED.load(Ordering::Relaxed),
        total_syscalls_tracked: TOTAL_SYSCALLS_TRACKED.load(Ordering::Relaxed),
        total_network_events: TOTAL_NETWORK_EVENTS.load(Ordering::Relaxed),
    }
}

/// Initialise le profileur.
pub fn profiler_init() {
    let mut profiler = PROFILER.lock();
    for i in 0..MAX_PROFILES {
        profiler.profiles[i] = ProcessProfile::empty();
    }
    profiler.profile_count = 0;

    TOTAL_PROFILES_CREATED.store(0, Ordering::Release);
    TOTAL_SYSCALLS_TRACKED.store(0, Ordering::Release);
    TOTAL_NETWORK_EVENTS.store(0, Ordering::Release);
}
