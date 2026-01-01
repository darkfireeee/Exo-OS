# IPC-SMP Integration Plan

**Date**: 2025-01-08  
**Version**: v0.6.0 → v0.7.0  
**Phase**: 2b → 2c

---

## 🎯 Objectif

Intégrer les systèmes **IPC** (Inter-Process Communication) et **SMP** (Symmetric MultiProcessing) pour permettre:

1. ✅ Communication cross-CPU efficace
2. ✅ Per-CPU IPC channels pour réduire contention
3. ✅ NUMA-aware message routing (si hardware multi-socket)
4. ✅ Wait queues intégrées avec scheduler SMP

---

## 📊 État actuel

### IPC (v0.6.0)
- ✅ MPMC ring buffers (lock-free)
- ✅ Fusion ring (high-performance)
- ✅ Typed channels (type-safe)
- ✅ Capabilities (security)
- ✅ Named endpoints (discovery)
- ⚠️ **Pas d'awareness SMP** - channels globaux

### SMP (v0.6.0)
- ✅ 4 CPUs online (1 BSP + 3 APs)
- ✅ Per-CPU scheduler queues
- ✅ Load balancing (work stealing)
- ✅ `current_cpu_id()` fast path (2-3 cycles)
- ⚠️ **Pas d'IPC per-CPU**

---

## 🚀 Architecture proposée

### 1. Per-CPU IPC Channels

```rust
// kernel/src/ipc/smp/percpu_channels.rs

/// Per-CPU IPC channel manager
pub struct PerCpuIpcManager {
    /// Channels locaux à chaque CPU
    local_channels: [Mutex<ChannelRegistry>; MAX_CPUS],
    
    /// Global channel registry (pour cross-CPU)
    global_channels: RwLock<HashMap<ChannelId, Arc<Channel>>>,
    
    /// Statistics per-CPU
    stats: [AtomicU64; MAX_CPUS],
}

impl PerCpuIpcManager {
    /// Create channel on current CPU (fast path)
    pub fn create_local_channel(&self) -> Result<ChannelId, IpcError> {
        let cpu_id = current_cpu_id();
        let mut registry = self.local_channels[cpu_id].lock();
        registry.create_channel()
    }
    
    /// Send message to local channel (no IPI needed)
    pub fn send_local(&self, id: ChannelId, msg: &[u8]) -> Result<(), IpcError> {
        let cpu_id = current_cpu_id();
        let registry = self.local_channels[cpu_id].lock();
        registry.send(id, msg)
    }
    
    /// Send message cross-CPU (may trigger IPI)
    pub fn send_cross_cpu(&self, id: ChannelId, msg: &[u8]) -> Result<(), IpcError> {
        // 1. Lookup channel in global registry
        let channels = self.global_channels.read();
        let channel = channels.get(&id).ok_or(IpcError::NotFound)?;
        
        // 2. Send to target CPU's queue
        let target_cpu = channel.owner_cpu();
        if target_cpu == current_cpu_id() {
            // Local - fast path
            self.send_local(id, msg)
        } else {
            // Cross-CPU - send IPI to wake receiver
            channel.send(msg)?;
            send_ipi(target_cpu, IPI_IPC_MESSAGE);
            Ok(())
        }
    }
}
```

### 2. NUMA-Aware Routing

```rust
// kernel/src/ipc/smp/numa.rs

/// NUMA node awareness for IPC
pub struct NumaIpcRouter {
    /// CPU to NUMA node mapping
    cpu_to_node: [u8; MAX_CPUS],
    
    /// Per-node channel pools
    node_pools: [Mutex<ChannelPool>; MAX_NUMA_NODES],
}

impl NumaIpcRouter {
    /// Choose best CPU for channel based on NUMA
    pub fn choose_cpu_for_channel(&self, hint: Option<usize>) -> usize {
        let current_cpu = current_cpu_id();
        let current_node = self.cpu_to_node[current_cpu];
        
        if let Some(cpu) = hint {
            // Check if hint is on same NUMA node
            if self.cpu_to_node[cpu] == current_node {
                return cpu; // Stay local
            }
        }
        
        // Find least loaded CPU on same node
        self.find_least_loaded_on_node(current_node)
    }
    
    /// Get NUMA node for CPU
    pub fn numa_node(cpu_id: usize) -> u8 {
        // TODO: Read from ACPI SRAT table
        // For now, assume 2 nodes: CPUs 0-1 on node 0, CPUs 2-3 on node 1
        if cpu_id < 2 { 0 } else { 1 }
    }
}
```

### 3. Wait Queue Integration

```rust
// kernel/src/ipc/smp/wait_queue.rs

use crate::scheduler::core::percpu_queue::PER_CPU_QUEUES;

/// IPC wait queue integrated with SMP scheduler
pub struct IpcWaitQueue {
    /// Waiting threads per CPU
    waiters: [Mutex<VecDeque<Arc<Thread>>>; MAX_CPUS],
}

impl IpcWaitQueue {
    /// Block current thread waiting for IPC event
    pub fn wait(&self, timeout: Option<Duration>) -> Result<(), IpcError> {
        let cpu_id = current_cpu_id();
        let current_thread = PER_CPU_QUEUES.get(cpu_id)
            .and_then(|q| q.current_thread())
            .ok_or(IpcError::Internal)?;
        
        // Add to wait queue
        {
            let mut waiters = self.waiters[cpu_id].lock();
            waiters.push_back(current_thread.clone());
        }
        
        // Yield to scheduler
        crate::scheduler::core::scheduler::schedule_smp();
        
        // When we return, message is available or timeout occurred
        Ok(())
    }
    
    /// Wake one thread waiting on this queue
    pub fn wake_one(&self, cpu_id: usize) -> bool {
        let mut waiters = self.waiters[cpu_id].lock();
        if let Some(thread) = waiters.pop_front() {
            // Re-enqueue in scheduler
            if let Some(queue) = PER_CPU_QUEUES.get(cpu_id) {
                queue.enqueue(thread);
                return true;
            }
        }
        false
    }
}
```

---

## 📋 Plan d'implémentation

### Phase 1: Per-CPU Channels (1 semaine)

**Fichiers à créer**:
1. `kernel/src/ipc/smp/mod.rs` - Module SMP IPC
2. `kernel/src/ipc/smp/percpu_channels.rs` - Per-CPU channel manager
3. `kernel/src/ipc/smp/stats.rs` - Statistics per-CPU

**Tests**:
- ✅ Create channel on each CPU
- ✅ Send/receive local (same CPU)
- ✅ Verify no cross-CPU overhead

**Metrics**:
- Local send: <100 cycles
- Cross-CPU send: <5000 cycles (includes IPI)

### Phase 2: Cross-CPU Messaging (1 semaine)

**Fichiers à modifier**:
1. `kernel/src/arch/x86_64/interrupts/apic.rs` - Add IPI handler for IPC
2. `kernel/src/ipc/core/mpmc_ring.rs` - Add CPU affinity hints
3. `kernel/src/ipc/core/endpoint.rs` - Route to correct CPU

**Tests**:
- ✅ Send from CPU 0 to CPU 1
- ✅ IPI delivery verified
- ✅ Message received correctly

**Metrics**:
- IPI latency: 20-50µs
- Cross-CPU throughput: >100K msg/sec

### Phase 3: Wait Queue Integration (1 semaine)

**Fichiers à créer**:
1. `kernel/src/ipc/smp/wait_queue.rs` - IPC wait queues
2. `kernel/src/ipc/smp/blocking.rs` - Blocking operations

**Tests**:
- ✅ Block thread on empty channel
- ✅ Wake on message arrival
- ✅ Timeout handling

**Metrics**:
- Block/wake latency: <10µs
- No busy-waiting

### Phase 4: NUMA Awareness (1 semaine - optionnel)

**Fichiers à créer**:
1. `kernel/src/ipc/smp/numa.rs` - NUMA-aware routing
2. `kernel/src/arch/x86_64/acpi/srat.rs` - Parse SRAT table

**Tests**:
- ✅ Detect NUMA nodes
- ✅ Prefer local node for channels
- ✅ Measure cross-node vs local performance

**Metrics**:
- Local node: 100ns latency
- Remote node: 200-300ns latency

---

## 🔧 API Modifications

### Before (v0.6.0)
```rust
// Global channel creation
let channel = ipc::channel::create()?;
channel.send(&msg)?; // No CPU awareness
```

### After (v0.7.0)
```rust
// Per-CPU channel creation
let channel = ipc::smp::create_local_channel()?;
channel.send_local(&msg)?; // Fast path

// Or cross-CPU explicitly
let channel = ipc::smp::create_global_channel()?;
channel.send_cross_cpu(target_cpu, &msg)?; // IPI if needed
```

---

## 📊 Performance Impact

### Expected improvements

| Metric | v0.6.0 (global) | v0.7.0 (per-CPU) | Improvement |
|--------|-----------------|------------------|-------------|
| Local send | 500 cycles | 100 cycles | **5x faster** ✅ |
| Lock contention | High | Minimal | **10x less** ✅ |
| Throughput | 50K msg/s | 500K msg/s | **10x higher** ✅ |
| Scalability | 1 CPU → 2 CPU: 1.2x | 1 CPU → 4 CPU: 3.5x | **Linear** ✅ |

### Benchmarks to run

```rust
#[test]
fn bench_local_ipc() {
    // Send 1M messages on same CPU
    for _ in 0..1_000_000 {
        channel.send_local(&msg).unwrap();
    }
    // Target: <100 cycles per send
}

#[test]
fn bench_cross_cpu_ipc() {
    // Send 100K messages CPU 0 → CPU 1
    for _ in 0..100_000 {
        channel.send_cross_cpu(1, &msg).unwrap();
    }
    // Target: <5000 cycles per send (includes IPI)
}
```

---

## ⚠️ Risques et mitigation

### Risque 1: IPI Storm
**Problème**: Trop d'IPIs dégradent performance  
**Solution**: Batch IPIs, coalescing, rate limiting

### Risque 2: NUMA Mismatch
**Problème**: Channel sur mauvais node → latency x2  
**Solution**: Heuristics pour placement initial, migration possible

### Risque 3: Deadlock
**Problème**: CPU A attend IPC de CPU B, CPU B attend CPU A  
**Solution**: Timeouts obligatoires, deadlock detection

---

## ✅ Checklist Phase 2c

- [ ] **Week 1**: Per-CPU channels
  - [ ] Create `ipc/smp/` module
  - [ ] Implement `PerCpuIpcManager`
  - [ ] Tests local send/receive
  - [ ] Benchmark: <100 cycles local

- [ ] **Week 2**: Cross-CPU messaging
  - [ ] IPI handler for IPC
  - [ ] Global channel registry
  - [ ] Tests cross-CPU
  - [ ] Benchmark: <50µs IPI

- [ ] **Week 3**: Wait queue integration
  - [ ] `IpcWaitQueue` implementation
  - [ ] Integrate with `schedule_smp()`
  - [ ] Tests blocking/waking
  - [ ] Benchmark: <10µs wake

- [ ] **Week 4**: NUMA awareness (optionnel)
  - [ ] ACPI SRAT parsing
  - [ ] NUMA-aware routing
  - [ ] Tests multi-node
  - [ ] Benchmark: local vs remote

- [ ] **Week 5**: Documentation & cleanup
  - [ ] API documentation
  - [ ] Integration guide
  - [ ] Performance report
  - [ ] Update CHANGELOG

---

## 📚 Documentation à créer

1. **IPC_SMP_ARCHITECTURE.md** - Architecture détaillée
2. **IPC_SMP_API.md** - Guide API développeur
3. **IPC_SMP_PERFORMANCE.md** - Benchmarks et optimisations
4. **IPC_SMP_MIGRATION.md** - Guide migration v0.6 → v0.7

---

## 🎯 Success Criteria v0.7.0

| Metric | Target | Validation |
|--------|--------|------------|
| Local IPC latency | <100 cycles | Benchmark |
| Cross-CPU latency | <50µs (with IPI) | Benchmark |
| Throughput | >500K msg/s | Stress test |
| Scalability | 3.5x on 4 CPUs | Parallel test |
| No deadlocks | 0 in 24h stress | Fuzzing |
| NUMA benefit | 2x vs remote | NUMA system |

---

## 🔗 Dependencies

### Required for Phase 2c
- ✅ SMP scheduler (v0.6.0 - DONE)
- ✅ Per-CPU queues (v0.6.0 - DONE)
- ✅ `current_cpu_id()` (v0.6.0 - DONE)
- ⏳ Timer integration (Phase 2c Week 3)
- ⏳ Wait queues (Phase 2c Week 3)

### Optional for v0.7.0
- ⏳ ACPI SRAT parsing (NUMA)
- ⏳ Hardware topology detection
- ⏳ Advanced IPI coalescing

---

**Status**: 📋 **PLANNED** - Ready to start Phase 2c  
**ETA**: 4-5 weeks (mid-February 2025)  
**Next**: Create `kernel/src/ipc/smp/mod.rs` stub
