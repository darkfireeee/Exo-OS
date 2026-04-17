# TLA+ Setup and Testing Guide for Exo-OS

## Installation Complete ✓

### Installed Components
- **Java**: OpenJDK 11.0.30 (installed via alpine apk)
- **TLA+ Toolbox**: Version 2026.04.09.014118 (installed to `/opt/tlaplus`)
- **TLC Model Checker**: Ready for use

### Environment Setup

Add these to your shell profile (`.bashrc`, `.zshrc`, etc.):

```bash
export PATH="/opt/tlaplus:$PATH"
export JAVA_HOME="/usr/lib/jvm/java-11-openjdk"
export TLATOOLS="/opt/tlaplus/tla2tools.jar"
```

Or set them for the current session:
```bash
export PATH="/opt/tlaplus:$PATH"
export TLATOOLS="/opt/tlaplus/tla2tools.jar"
```

## Quick Start: Running TLC Model Checker

### Basic Syntax
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC -config ExoOS_Stress.cfg ExoOS_Full
```

### TLA+ Models Available in `/workspaces/Exo-OS/docs/Exo-OS-TLA+/`

- `ExoOS_Full.tla` - Full OS specification
- `ContextSwitch.tla` - Context switching specification
- `Memory.tla` - Memory management
- `IrqRouting.tla` - IRQ routing logic
- `ExoFS.tla` - File system (ExoFS)
- `ExoShield.tla` - Security shield implementation
- `SmpBoot.tla` - SMP boot sequence
- `PciDoExit.tla` - PCI device exit handling
- `CapTokens.tla` - Capability tokens
- `IommuQueue.tla` - IOMMU queue management
- `ExoPhoenixHandoff.tla` - Phoenix handoff protocol
- `Adversarial.tla` - Adversarial model
- `IrqRoutingStress.tla` - Stress test for IRQ routing

### Configuration File
- `ExoOS_Stress.cfg` - Stress test configuration with parameters

## Testing Examples

### 1. Run with Default Configuration
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC -config ExoOS_Stress.cfg ExoOS_Full
```

### 2. Run with Custom Depth Limit (Simulation)
```bash
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \
  -simulate \
  -depth 100 \
  -config ExoOS_Stress.cfg \
  ExoOS_Full
```

### 3. View Specification Help
```bash
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC -help
```

### 4. Test Individual Module
```bash
java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC -config ContextSwitch.cfg ContextSwitch
```

## Common TLC Options

| Option | Purpose |
|--------|---------|
| `-config FILE` | Specify configuration file (default: SPEC.cfg) |
| `-simulate` | Run simulation instead of model checking |
| `-depth NUM` | Maximum depth for simulation |
| `-workers NUM` | Number of worker threads |
| `-deadlock` | Check for deadlock (default behavior) |
| `-cleanup` | Clean up previous state directory |
| `-checkpoint MINUTES` | Checkpoint interval |
| `-continue` | Continue after finding first violation |
| `-difftrace` | Show diff trace for error |

## Troubleshooting

### Java not found
Ensure Java is in PATH:
```bash
java -version
```

### TLC not found
Verify TLA+ tools installation:
```bash
ls -la /opt/tlaplus/tla2tools.jar
```

### Specification file not found
Ensure you're in the correct directory:
```bash
cd /workspaces/Exo-OS/docs/Exo-OS-TLA+
pwd
ls -la *.tla
```

### Out of memory
Increase Java heap size:
```bash
java -Xmx4g -cp /opt/tlaplus/tla2tools.jar tlc2.TLC ...
```

## Files and Locations

| Item | Location |
|------|----------|
| Java Installation | `/usr/lib/jvm/java-11-openjdk/` |
| TLA+ Toolbox | `/opt/tlaplus/` |
| TLA+ Jar File | `/opt/tlaplus/tla2tools.jar` |
| Project TLA+ Files | `/workspaces/Exo-OS/docs/Exo-OS-TLA+/` |

## Additional Resources

- TLA+ Tools Documentation: https://lamport.azurewebsites.net/tla/current-tools.pdf
- TLA+ Learning: https://lamport.azurewebsites.net/tla/learning.html
- TLA+ Hyperbook: https://lamport.azurewebsites.net/tla/hyperbook.html

---

Setup completed on: 2026-04-17
