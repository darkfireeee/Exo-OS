//! # load_balancer — Répartition de charge IPC (ipc_router PID 2)
//!
//! Système de répartition de charge pour les services multi-instances.
//! Supporte Round-Robin, LeastLoaded, WeightedRandom, et PriorityBased.
//! Intègre un Circuit Breaker pour éviter les instances défaillantes.
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin
//! - IPC-02 : pas de Vec/String/Box

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum d'instances par service.
const MAX_INSTANCES: usize = 16;

/// Nombre maximum de pools de services.
const MAX_POOLS: usize = 10;

/// Nombre maximum de circuits breakers.
const MAX_CIRCUITS: usize = 16;

/// Seuil de pannes pour ouvrir un circuit breaker.
const CIRCUIT_FAILURE_THRESHOLD: u32 = 5;

/// Durée du circuit breaker ouvert (en cycles TSC, ~30 secondes).
const CIRCUIT_OPEN_DURATION_TSC: u64 = 90_000_000_000;

/// Durée du half-open (en cycles TSC, ~5 secondes).
const CIRCUIT_HALF_OPEN_DURATION_TSC: u64 = 15_000_000_000;

/// Timeout de heartbeat (en cycles TSC, ~5 secondes).
const HEARTBEAT_TIMEOUT_TSC: u64 = 15_000_000_000;

// ── Stratégies de répartition ────────────────────────────────────────────────

/// Stratégie de sélection d'instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LoadStrategy {
    /// Round-robin séquentiel.
    RoundRobin = 0,
    /// Instance avec la charge la plus faible.
    LeastLoaded = 1,
    /// Random pondéré par le poids.
    WeightedRandom = 2,
    /// Basé sur la priorité des instances.
    PriorityBased = 3,
}

impl LoadStrategy {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::RoundRobin),
            1 => Some(Self::LeastLoaded),
            2 => Some(Self::WeightedRandom),
            3 => Some(Self::PriorityBased),
            _ => None,
        }
    }
}

// ── État du Circuit Breaker ──────────────────────────────────────────────────

/// État d'un circuit breaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CircuitState {
    /// Circuit fermé : tout passe.
    Closed = 0,
    /// Circuit ouvert : tout est rejeté.
    Open = 1,
    /// Circuit semi-ouvert : un essai est autorisé.
    HalfOpen = 2,
}

impl CircuitState {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Closed),
            1 => Some(Self::Open),
            2 => Some(Self::HalfOpen),
            _ => None,
        }
    }
}

// ── Instance de service ──────────────────────────────────────────────────────

/// Représente une instance d'un service.
#[repr(C)]
struct ServiceInstance {
    /// PID de l'instance.
    pid: u32,
    /// Score de charge (plus bas = moins chargé).
    load_score: AtomicU32,
    /// Poids pour la sélection (plus élevé = plus de trafic).
    weight: u32,
    /// Priorité (plus élevé = préféré).
    priority: u8,
    /// Instance vivante ?
    is_alive: AtomicU8,
    /// TSC du dernier heartbeat.
    last_heartbeat_tsc: AtomicU64,
    /// Messages en attente.
    pending_messages: AtomicU32,
    /// Latence moyenne (en cycles TSC).
    avg_latency: AtomicU64,
    /// Taux d'erreur (pourcentage × 100).
    error_rate: AtomicU32,
    /// État actif dans le pool.
    active: AtomicU8,
}

impl ServiceInstance {
    const fn new() -> Self {
        Self {
            pid: 0,
            load_score: AtomicU32::new(0),
            weight: 100,
            priority: 5,
            is_alive: AtomicU8::new(0),
            last_heartbeat_tsc: AtomicU64::new(0),
            pending_messages: AtomicU32::new(0),
            avg_latency: AtomicU64::new(0),
            error_rate: AtomicU32::new(0),
            active: AtomicU8::new(0),
        }
    }
}

// ── Pool de services ─────────────────────────────────────────────────────────

/// Pool d'instances pour un service.
#[repr(C)]
struct ServicePool {
    /// ID du service.
    service_id: u32,
    /// Instances du pool.
    instances: [ServiceInstance; MAX_INSTANCES],
    /// Nombre d'instances.
    instance_count: AtomicU32,
    /// Index du round-robin.
    round_robin_index: AtomicU32,
    /// Stratégie de sélection.
    strategy: AtomicU8,
    /// État actif.
    active: AtomicU8,
}

impl ServicePool {
    const fn new() -> Self {
        Self {
            service_id: 0,
            instances: [
                ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(),
                ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(),
                ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(),
                ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(), ServiceInstance::new(),
            ],
            instance_count: AtomicU32::new(0),
            round_robin_index: AtomicU32::new(0),
            strategy: AtomicU8::new(LoadStrategy::RoundRobin as u8),
            active: AtomicU8::new(0),
        }
    }
}

// ── Circuit Breaker ──────────────────────────────────────────────────────────

/// Circuit breaker pour une instance.
#[repr(C)]
struct CircuitBreaker {
    /// PID de l'instance.
    pid: u32,
    /// Nombre de pannes consécutives.
    failure_count: AtomicU32,
    /// TSC de la dernière panne.
    last_failure_tsc: AtomicU64,
    /// État du circuit.
    state: AtomicU8,
    /// Nombre de succès en half-open.
    half_open_successes: AtomicU32,
    /// État actif.
    active: AtomicU8,
}

impl CircuitBreaker {
    const fn new() -> Self {
        Self {
            pid: 0,
            failure_count: AtomicU32::new(0),
            last_failure_tsc: AtomicU64::new(0),
            state: AtomicU8::new(CircuitState::Closed as u8),
            half_open_successes: AtomicU32::new(0),
            active: AtomicU8::new(0),
        }
    }
}

// ── Stockage statique ────────────────────────────────────────────────────────

static POOLS: spin::Mutex<[ServicePool; MAX_POOLS]> = spin::Mutex::new({
    const P: ServicePool = ServicePool::new();
    [P; MAX_POOLS]
});

static POOL_COUNT: AtomicU32 = AtomicU32::new(0);

static CIRCUITS: spin::Mutex<[CircuitBreaker; MAX_CIRCUITS]> = spin::Mutex::new({
    const C: CircuitBreaker = CircuitBreaker::new();
    [C; MAX_CIRCUITS]
});

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

// ── Sélection d'instance ─────────────────────────────────────────────────────

/// Sélectionne la meilleure instance selon la stratégie configurée.
pub fn select_instance(service_id: u32) -> Option<u32> {
    let pools = POOLS.lock();
    let pool_count = POOL_COUNT.load(Ordering::Acquire) as usize;

    // Trouver le pool pour ce service
    let pool_idx = {
        let mut found = None;
        for i in 0..pool_count.min(MAX_POOLS) {
            if pools[i].active.load(Ordering::Acquire) != 0 && pools[i].service_id == service_id {
                found = Some(i);
                break;
            }
        }
        found
    };

    let pool_idx = match pool_idx {
        Some(idx) => idx,
        None => return None,
    };

    let pool = &pools[pool_idx];
    let count = pool.instance_count.load(Ordering::Acquire) as usize;
    if count == 0 {
        return None;
    }

    let strategy = LoadStrategy::from_u8(pool.strategy.load(Ordering::Acquire))
        .unwrap_or(LoadStrategy::RoundRobin);

    match strategy {
        LoadStrategy::RoundRobin => select_round_robin(pool, count),
        LoadStrategy::LeastLoaded => select_least_loaded(pool, count),
        LoadStrategy::WeightedRandom => select_weighted_random(pool, count),
        LoadStrategy::PriorityBased => select_priority_based(pool, count),
    }
}

/// Sélection Round-Robin : alterne cycliquement entre les instances.
fn select_round_robin(pool: &ServicePool, count: usize) -> Option<u32> {
    let start = pool.round_robin_index.fetch_add(1, Ordering::AcqRel) as usize;

    for offset in 0..count {
        let idx = (start + offset) % count;
        let inst = &pool.instances[idx];
        if inst.active.load(Ordering::Acquire) != 0 &&
           inst.is_alive.load(Ordering::Acquire) != 0 &&
           check_circuit(inst.pid) == CircuitState::Closed {
            return Some(inst.pid);
        }
    }

    // Fallback : première instance vivante même si circuit est half-open
    for idx in 0..count {
        let inst = &pool.instances[idx];
        if inst.active.load(Ordering::Acquire) != 0 &&
           inst.is_alive.load(Ordering::Acquire) != 0 {
            return Some(inst.pid);
        }
    }

    None
}

/// Sélection Least-Loaded : l'instance avec le moins de messages en attente.
fn select_least_loaded(pool: &ServicePool, count: usize) -> Option<u32> {
    let mut best_idx: Option<usize> = None;
    let mut best_load = u32::MAX;

    for idx in 0..count {
        let inst = &pool.instances[idx];
        if inst.active.load(Ordering::Acquire) == 0 { continue; }
        if inst.is_alive.load(Ordering::Acquire) == 0 { continue; }
        if check_circuit(inst.pid) != CircuitState::Closed { continue; }

        let load = inst.pending_messages.load(Ordering::Acquire);
        if load < best_load {
            best_load = load;
            best_idx = Some(idx);
        }
    }

    best_idx.map(|idx| pool.instances[idx].pid)
}

/// Sélection Weighted Random : probabiliste selon les poids.
fn select_weighted_random(pool: &ServicePool, count: usize) -> Option<u32> {
    // Somme des poids des instances vivantes
    let mut total_weight: u64 = 0;
    let mut weights = [0u64; MAX_INSTANCES];

    for idx in 0..count {
        let inst = &pool.instances[idx];
        if inst.active.load(Ordering::Acquire) != 0 &&
           inst.is_alive.load(Ordering::Acquire) != 0 &&
           check_circuit(inst.pid) == CircuitState::Closed {
            weights[idx] = inst.weight as u64;
            total_weight += weights[idx];
        }
    }

    if total_weight == 0 {
        return None;
    }

    // Pseudo-aléatoire via TSC
    let seed = read_tsc();
    let mut rng = seed;
    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let threshold = (rng >> 32) as u64 % total_weight;

    let mut cumulative: u64 = 0;
    for idx in 0..count {
        cumulative += weights[idx];
        if cumulative > threshold {
            return Some(pool.instances[idx].pid);
        }
    }

    // Fallback : dernière instance
    for idx in (0..count).rev() {
        if pool.instances[idx].active.load(Ordering::Acquire) != 0 {
            return Some(pool.instances[idx].pid);
        }
    }
    None
}

/// Sélection Priority-Based : l'instance avec la plus haute priorité.
fn select_priority_based(pool: &ServicePool, count: usize) -> Option<u32> {
    let mut best_idx: Option<usize> = None;
    let mut best_priority: u8 = 0;

    for idx in 0..count {
        let inst = &pool.instances[idx];
        if inst.active.load(Ordering::Acquire) == 0 { continue; }
        if inst.is_alive.load(Ordering::Acquire) == 0 { continue; }
        if check_circuit(inst.pid) != CircuitState::Closed { continue; }

        if inst.priority > best_priority {
            best_priority = inst.priority;
            best_idx = Some(idx);
        }
    }

    best_idx.map(|idx| pool.instances[idx].pid)
}

// ── Circuit Breaker ──────────────────────────────────────────────────────────

/// Vérifie l'état du circuit breaker pour un PID.
pub fn check_circuit(pid: u32) -> CircuitState {
    let now = read_tsc();
    let circuits = CIRCUITS.lock();

    for circuit in circuits.iter() {
        if circuit.active.load(Ordering::Acquire) != 0 && circuit.pid == pid {
            let state = CircuitState::from_u8(circuit.state.load(Ordering::Acquire))
                .unwrap_or(CircuitState::Closed);

            match state {
                CircuitState::Open => {
                    let last = circuit.last_failure_tsc.load(Ordering::Acquire);
                    if now.wrapping_sub(last) > CIRCUIT_OPEN_DURATION_TSC {
                        // Transition vers half-open
                        drop(circuits);
                        set_circuit_state(pid, CircuitState::HalfOpen);
                        return CircuitState::HalfOpen;
                    }
                    return CircuitState::Open;
                }
                CircuitState::HalfOpen => {
                    let last = circuit.last_failure_tsc.load(Ordering::Acquire);
                    if now.wrapping_sub(last) > CIRCUIT_HALF_OPEN_DURATION_TSC {
                        // Timeout en half-open → retour à open
                        return CircuitState::HalfOpen;
                    }
                    return CircuitState::HalfOpen;
                }
                CircuitState::Closed => {
                    return CircuitState::Closed;
                }
            }
        }
    }

    // Pas de circuit = fermé (tout passe)
    CircuitState::Closed
}

/// Change l'état du circuit breaker.
fn set_circuit_state(pid: u32, new_state: CircuitState) {
    let mut circuits = CIRCUITS.lock();

    for circuit in circuits.iter() {
        if circuit.active.load(Ordering::Acquire) != 0 && circuit.pid == pid {
            circuit.state.store(new_state as u8, Ordering::Release);
            if new_state == CircuitState::HalfOpen {
                circuit.half_open_successes.store(0, Ordering::Release);
            }
            return;
        }
    }

    // Créer un nouveau circuit si nécessaire
    if new_state == CircuitState::Open {
        for circuit in circuits.iter() {
            if circuit.active.load(Ordering::Acquire) == 0 {
                circuit.pid = pid;
                circuit.failure_count.store(CIRCUIT_FAILURE_THRESHOLD, Ordering::Release);
                circuit.last_failure_tsc.store(read_tsc(), Ordering::Release);
                circuit.state.store(CircuitState::Open as u8, Ordering::Release);
                circuit.half_open_successes.store(0, Ordering::Release);
                circuit.active.store(1, Ordering::Release);
                return;
            }
        }
    }
}

/// Enregistre un échec pour une instance (circuit breaker).
pub fn record_failure(pid: u32) {
    let mut circuits = CIRCUITS.lock();

    for circuit in circuits.iter() {
        if circuit.active.load(Ordering::Acquire) != 0 && circuit.pid == pid {
            let state = CircuitState::from_u8(circuit.state.load(Ordering::Acquire))
                .unwrap_or(CircuitState::Closed);

            match state {
                CircuitState::Closed => {
                    let failures = circuit.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
                    if failures >= CIRCUIT_FAILURE_THRESHOLD {
                        circuit.state.store(CircuitState::Open as u8, Ordering::Release);
                        circuit.last_failure_tsc.store(read_tsc(), Ordering::Release);
                    }
                }
                CircuitState::HalfOpen => {
                    // Échec en half-open → retour à open
                    circuit.state.store(CircuitState::Open as u8, Ordering::Release);
                    circuit.last_failure_tsc.store(read_tsc(), Ordering::Release);
                    circuit.half_open_successes.store(0, Ordering::Release);
                }
                CircuitState::Open => {
                    // Déjà ouvert, juste mettre à jour le TSC
                    circuit.last_failure_tsc.store(read_tsc(), Ordering::Release);
                }
            }
            return;
        }
    }

    // Créer un circuit pour ce PID
    for circuit in circuits.iter() {
        if circuit.active.load(Ordering::Acquire) == 0 {
            circuit.pid = pid;
            circuit.failure_count.store(1, Ordering::Release);
            circuit.last_failure_tsc.store(read_tsc(), Ordering::Release);
            circuit.state.store(CircuitState::Closed as u8, Ordering::Release);
            circuit.active.store(1, Ordering::Release);
            return;
        }
    }
}

/// Enregistre un succès pour une instance (circuit breaker).
pub fn record_success(pid: u32) {
    let mut circuits = CIRCUITS.lock();

    for circuit in circuits.iter() {
        if circuit.active.load(Ordering::Acquire) != 0 && circuit.pid == pid {
            let state = CircuitState::from_u8(circuit.state.load(Ordering::Acquire))
                .unwrap_or(CircuitState::Closed);

            match state {
                CircuitState::HalfOpen => {
                    let successes = circuit.half_open_successes.fetch_add(1, Ordering::AcqRel) + 1;
                    if successes >= 3 {
                        // Assez de succès → fermer le circuit
                        circuit.state.store(CircuitState::Closed as u8, Ordering::Release);
                        circuit.failure_count.store(0, Ordering::Release);
                    }
                }
                CircuitState::Closed => {
                    // Réduire le compteur de pannes
                    let current = circuit.failure_count.load(Ordering::Acquire);
                    if current > 0 {
                        circuit.failure_count.store(current - 1, Ordering::Release);
                    }
                }
                CircuitState::Open => {
                    // Ignorer les succès quand le circuit est ouvert
                }
            }
            return;
        }
    }
}

// ── Gestion des pools ────────────────────────────────────────────────────────

/// Enregistre une nouvelle instance dans un pool de service.
pub fn register_instance(service_id: u32, pid: u32, weight: u32, priority: u8) -> bool {
    let mut pools = POOLS.lock();
    let pool_count = POOL_COUNT.load(Ordering::Acquire) as usize;

    // Trouver le pool existant ou en créer un
    let pool_idx = {
        let mut found = None;
        for i in 0..pool_count.min(MAX_POOLS) {
            if pools[i].active.load(Ordering::Acquire) != 0 && pools[i].service_id == service_id {
                found = Some(i);
                break;
            }
        }
        found
    };

    let pool_idx = match pool_idx {
        Some(idx) => idx,
        None => {
            // Créer un nouveau pool
            let mut new_idx = None;
            for i in 0..MAX_POOLS {
                if pools[i].active.load(Ordering::Acquire) == 0 {
                    pools[i].service_id = service_id;
                    pools[i].active.store(1, Ordering::Release);
                    pools[i].instance_count.store(0, Ordering::Release);
                    pools[i].round_robin_index.store(0, Ordering::Release);
                    pools[i].strategy.store(LoadStrategy::RoundRobin as u8, Ordering::Release);
                    POOL_COUNT.fetch_add(1, Ordering::Release);
                    new_idx = Some(i);
                    break;
                }
            }
            match new_idx {
                Some(idx) => idx,
                None => return false,
            }
        }
    };

    let pool = &mut pools[pool_idx];
    let count = pool.instance_count.load(Ordering::Acquire) as usize;

    // Vérifier si l'instance existe déjà
    for i in 0..count.min(MAX_INSTANCES) {
        if pool.instances[i].active.load(Ordering::Acquire) != 0 && pool.instances[i].pid == pid {
            // Mettre à jour le poids et la priorité
            pool.instances[i].weight = weight;
            pool.instances[i].priority = priority;
            return true;
        }
    }

    // Ajouter une nouvelle instance
    if count >= MAX_INSTANCES {
        return false;
    }

    pool.instances[count].pid = pid;
    pool.instances[count].weight = weight;
    pool.instances[count].priority = priority;
    pool.instances[count].load_score.store(0, Ordering::Release);
    pool.instances[count].is_alive.store(1, Ordering::Release);
    pool.instances[count].last_heartbeat_tsc.store(read_tsc(), Ordering::Release);
    pool.instances[count].pending_messages.store(0, Ordering::Release);
    pool.instances[count].avg_latency.store(0, Ordering::Release);
    pool.instances[count].error_rate.store(0, Ordering::Release);
    pool.instances[count].active.store(1, Ordering::Release);
    pool.instance_count.fetch_add(1, Ordering::Release);

    true
}

/// Retire une instance d'un pool.
pub fn unregister_instance(service_id: u32, pid: u32) -> bool {
    let mut pools = POOLS.lock();

    for i in 0..MAX_POOLS {
        if pools[i].active.load(Ordering::Acquire) != 0 && pools[i].service_id == service_id {
            let count = pools[i].instance_count.load(Ordering::Acquire) as usize;
            for j in 0..count.min(MAX_INSTANCES) {
                if pools[i].instances[j].active.load(Ordering::Acquire) != 0 && pools[i].instances[j].pid == pid {
                    pools[i].instances[j].active.store(0, Ordering::Release);
                    pools[i].instances[j].is_alive.store(0, Ordering::Release);
                    pools[i].instance_count.fetch_sub(1, Ordering::Release);
                    return true;
                }
            }
        }
    }
    false
}

/// Met à jour la charge d'une instance.
pub fn update_load(pid: u32, pending: u32, latency_tsc: u64) {
    let pools = POOLS.lock();

    for pool in pools.iter() {
        if pool.active.load(Ordering::Acquire) == 0 { continue; }
        let count = pool.instance_count.load(Ordering::Acquire) as usize;
        for j in 0..count.min(MAX_INSTANCES) {
            if pool.instances[j].active.load(Ordering::Acquire) != 0 && pool.instances[j].pid == pid {
                pool.instances[j].pending_messages.store(pending, Ordering::Release);
                pool.instances[j].avg_latency.store(latency_tsc, Ordering::Release);
                pool.instances[j].load_score.store(pending, Ordering::Release);
                return;
            }
        }
    }
}

/// Vérifie la santé d'une instance via son heartbeat.
pub fn check_health(pid: u32) -> bool {
    let now = read_tsc();
    let pools = POOLS.lock();

    for pool in pools.iter() {
        if pool.active.load(Ordering::Acquire) == 0 { continue; }
        let count = pool.instance_count.load(Ordering::Acquire) as usize;
        for j in 0..count.min(MAX_INSTANCES) {
            if pool.instances[j].active.load(Ordering::Acquire) != 0 && pool.instances[j].pid == pid {
                let last = pool.instances[j].last_heartbeat_tsc.load(Ordering::Acquire);
                return now.wrapping_sub(last) < HEARTBEAT_TIMEOUT_TSC;
            }
        }
    }
    false
}

/// Rééquilibre les instances quand une instance rejoint ou quitte.
pub fn rebalance(service_id: u32) {
    // Réinitialiser le round-robin pour une distribution équitable
    let pools = POOLS.lock();

    for pool in pools.iter() {
        if pool.active.load(Ordering::Acquire) != 0 && pool.service_id == service_id {
            pool.round_robin_index.store(0, Ordering::Release);
            return;
        }
    }
}

/// Initialise le load balancer.
pub fn load_balancer_init() {
    // Les pools seront créés dynamiquement au fur et à mesure
    // que les services s'enregistrent via IPC
}
