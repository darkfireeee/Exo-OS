# TLA+ Testing Guide for Exo-OS

## Installation Summary

✅ **Successfully Installed:**
- Java OpenJDK 11.0.30
- TLA+ Toolbox v2026.04.09.014118
- TLC Model Checker

## Quick Start

### 1. Setup Environment (one-time)
```bash
cd /workspaces/Exo-OS
source tla_env.sh testing
```

### 2. Run Model Checker

**Basic command format:**
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC -config ExoOS_Stress.cfg ExoOS_Full
```

## Available Models

| Model | File | Purpose |
|-------|------|---------|
| Full Exo-OS | `ExoOS_Full.tla` | Complete system specification |
| Context Switching | `ContextSwitch.tla` | CPU context switching logic |
| Memory Management | `Memory.tla` | Memory subsystem |
| IRQ Routing | `IrqRouting.tla` | IRQ distribution |
| IRQ Stress Test | `IrqRoutingStress.tla` | Stress testing IRQs |
| File System | `ExoFS.tla` | ExoFS specification |
| Security Shield | `ExoShield.tla` | Security mechanisms |
| SMP Boot | `SmpBoot.tla` | Multi-processor bootup |
| PCI Device Exit | `PciDoExit.tla` | PCI device handling |
| Capability Tokens | `CapTokens.tla` | Capability system |
| IOMMU Queue | `IommuQueue.tla` | IOMMU queue management |
| Phoenix Handoff | `ExoPhoenixHandoff.tla` | Phoenix protocol |
| Adversarial Model | `Adversarial.tla` | Adversarial scenario testing |

## Testing Examples

### Example 1: Quick Simulation (limited depth)
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -simulate -depth 50 \
  -config ExoOS_Stress.cfg \
  ExoOS_Full
```

### Example 2: Full Model Checking with Config
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -config ExoOS_Stress.cfg \
  ExoOS_Full
```

### Example 3: Test Individual Module (ContextSwitch)
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC ContextSwitch
```

### Example 4: Multiple Worker Threads
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -workers 4 \
  -config ExoOS_Stress.cfg \
  ExoOS_Full
```

### Example 5: Increase Heap Memory
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -Xmx8g -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -config ExoOS_Stress.cfg \
  ExoOS_Full
```

### Example 6: Check for Deadlocks
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -deadlock \
  -config ExoOS_Stress.cfg \
  ExoOS_Full
```

### Example 7: Simulation with Specific Seed
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -simulate -depth 100 -seed 12345 \
  -config ExoOS_Stress.cfg \
  ExoOS_Full
```

## Configuration File: ExoOS_Stress.cfg

The configuration file defines test parameters:

```tla
SPECIFICATION Spec

CONSTANTS
    CORES = {"c1", "c2", "c3", "c4"}
    IRQS = {"irq1", "irq2"}
    PIDS = {1, 2}
    MAX_PENDING_ACKS = 3
    MAX_OVERFLOWS = 5
    MAX_GEN = 3
    MAX_TIMER = 10
    TIMEOUT_TICKS = 5
    MAX_TICKS = 15
    N_CYCLES = 5

INVARIANTS
    S_GLOBAL_1
    S_GLOBAL_2
    S_GLOBAL_5

PROPERTIES
    S_GLOBAL_3
```

## Interpreting Output

When running TLC, you'll see output like:

```
TLC2 Version 2026.04.09.014118 (rev: 389440f)
Running breadth-first search Model-Checking with fp 102 and seed 6015706654147479304...
Parsing file /workspaces/Exo-OS/docs/Exo-OS-TLA+/ExoOS_Full.tla
Semantic processing of module ExoOS_Full
Starting... (2026-04-17 16:40:49)
Computing initial states...
Computed 16 distinct states generated
Progress(5): 96,645 states generated, 37,413 distinct states found
```

**Key metrics:**
- **states generated**: Total states explored
- **distinct states found**: Unique states in the state space
- **states left on queue**: States still to explore
- If it completes successfully: "Model checking completed successfully"
- If it fails: Shows invariant violation or error details

## Common Issues & Solutions

### ❌ Java OutOfMemory
**Solution:** Increase heap size
```bash
java -Xmx4g -cp /opt/tlaplus/tla2tools.jar tlc2.TLC ...
```

### ❌ Module not found
**Solution:** Ensure you're in the correct directory
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
pwd
```

### ❌ Config file not found
**Solution:** Use full path or ensure file exists
```bash
ls -la ExoOS_Stress.cfg
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -config $(pwd)/ExoOS_Stress.cfg ExoOS_Full
```

### ❌ Performance is slow
**Solutions:**
1. Use simulation mode instead of model checking: `-simulate -depth 50`
2. Increase workers: `-workers 4`
3. Reduce state space via config parameters
4. Use checkpoint: `-checkpoint 10`

## Advanced Options

| Option | Usage |
|--------|-------|
| `-cleanup` | Removes previous state directory |
| `-continue` | Continues after finding first invariant violation |
| `-checkpoint MIN` | Save checkpoint every N minutes |
| `-coverage MIN` | Collect coverage information |
| `-difftrace` | Show diff trace for errors |
| `-gzip` | Compress checkpoints |
| `-nowarning` | Suppress warnings |
| `-debug` | Enable debug logging |
| `-seed SEED` | Set random seed for simulation |
| `-aril NUM` | Adjust random initial seed |

## Manual Testing Workflow

1. **Set up environment:**
   ```bash
   cd /workspaces/Exo-OS
   source tla_env.sh testing
   ```

2. **Check specifications:**
   ```bash
   cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
   ls -la *.tla
   ```

3. **Start with simulation:**
   ```bash
   java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
     -simulate -depth 50 \
     -config ExoOS_Stress.cfg \
     ExoOS_Full
   ```

4. **If simulation passes, run full check:**
   ```bash
   java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
     -config ExoOS_Stress.cfg \
     ExoOS_Full
   ```

5. **Review output for violations/errors**

## File Structure

```
/opt/tlaplus/                          # TLA+ Installation
├── tla2tools.jar                      # TLC model checker
├── CommunityModules-deps.jar          # Community modules
├── plugins/                           # Eclipse plugins
└── configuration/

/workspaces/Exo-OS/                    # Project root
├── tla_env.sh                         # Environment setup script
├── TLA_SETUP.md                       # This file
└── docs/Exo-OS-TLA+/                  # TLA+ specifications
    ├── *.tla                          # Module files
    └── ExoOS_Stress.cfg               # Configuration
```

## References

- **TLA+ Website**: https://lamport.azurewebsites.net/tla/
- **TLA+ Tools**: https://lamport.azurewebsites.net/tla/toolbox.html
- **TLC Guide**: https://lamport.azurewebsites.net/tla/current-tools.pdf
- **TLA+ Hyperbook**: https://lamport.azurewebsites.net/tla/hyperbook.html

---

**Setup completed:** 2026-04-17  
**Tested:** ✅ Java working, ✅ TLA+ working, ✅ TLC model checking functional
