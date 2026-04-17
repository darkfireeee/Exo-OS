#!/bin/bash
# TLA+ Quick Reference Card for Exo-OS
# Run with: bash tla_quick_ref.sh

cat << 'EOF'

╔════════════════════════════════════════════════════════════════════════╗
║                   TLA+ QUICK REFERENCE - Exo-OS                       ║
╚════════════════════════════════════════════════════════════════════════╝

┌─ SETUP ─────────────────────────────────────────────────────────────────┐
│ cd /workspaces/Exo-OS                                                  │
│ source tla_env.sh testing                                              │
└─────────────────────────────────────────────────────────────────────────┘

┌─ RUN TLC MODEL CHECKER ─────────────────────────────────────────────────┐
│ cd /workspaces/Exo-OS/docs/Exo-OS-TLA+                                 │
│                                                                          │
│ Full model check:                                                       │
│   java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \                       │
│     -config ExoOS_Stress.cfg ExoOS_Full                                 │
│                                                                          │
│ Quick simulation (depth=50):                                            │
│   java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \                       │
│     -simulate -depth 50 -config ExoOS_Stress.cfg ExoOS_Full            │
│                                                                          │
│ With multiple workers:                                                  │
│   java -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \                       │
│     -workers 4 -config ExoOS_Stress.cfg ExoOS_Full                     │
│                                                                          │
│ With more heap memory:                                                  │
│   java -Xmx4g -cp /opt/tlaplus/tla2tools.jar tlc2.TLC \                │
│     -config ExoOS_Stress.cfg ExoOS_Full                                 │
└─────────────────────────────────────────────────────────────────────────┘

┌─ AVAILABLE MODELS ──────────────────────────────────────────────────────┐
│ ExoOS_Full.tla              Full OS specification                       │
│ ContextSwitch.tla           CPU context switching                       │
│ Memory.tla                  Memory management                           │
│ IrqRouting.tla              IRQ distribution                            │
│ ExoFS.tla                   File system                                 │
│ ExoShield.tla               Security mechanisms                         │
│ SmpBoot.tla                 Multi-processor boot                        │
│ (and 6 more in /docs/Exo-OS-TLA+)                                      │
└─────────────────────────────────────────────────────────────────────────┘

┌─ COMMON TLC OPTIONS ────────────────────────────────────────────────────┐
│ -config FILE              Specify configuration file                    │
│ -simulate                 Run simulation instead of model checking      │
│ -depth NUM                Max depth for simulation                      │
│ -workers NUM              Number of worker threads                      │
│ -cleanup                  Clean up previous state directory             │
│ -continue                 Continue after first invariant violation      │
│ -seed NUM                 Set random seed                               │
│ -checkpoint MIN           Checkpoint interval in minutes                │
│ -deadlock                 Test for deadlock (default)                   │
│ -nowarning                Suppress warnings                             │
└─────────────────────────────────────────────────────────────────────────┘

┌─ KEY LOCATIONS ─────────────────────────────────────────────────────────┐
│ Java:               /usr/lib/jvm/java-11-openjdk/                      │
│ TLA+ Toolbox:       /opt/tlaplus/                                      │
│ TLA+ Jar:           /opt/tlaplus/tla2tools.jar                         │
│ Project Models:     /workspaces/Exo-OS/docs/Exo-OS-TLA+/              │
│ Config File:        /workspaces/Exo-OS/docs/Exo-OS-TLA+/ExoOS_Stress.cfg
└─────────────────────────────────────────────────────────────────────────┘

┌─ TROUBLESHOOTING ───────────────────────────────────────────────────────┐
│ OutOfMemory:        java -Xmx8g -cp ...                                │
│ Slow performance:   Use -simulate -depth 50                             │
│ Module not found:   cd /workspaces/Exo-OS/docs/Exo-OS-TLA+            │
│ Java not found:     apt-get install openjdk-11-jdk                     │
└─────────────────────────────────────────────────────────────────────────┘

┌─ RESOURCES ─────────────────────────────────────────────────────────────┐
│ Detailed Guide:     /workspaces/Exo-OS/TLA_TESTING_GUIDE.md            │
│ Setup Guide:        /workspaces/Exo-OS/TLA_SETUP.md                    │
│ Current Script:     /workspaces/Exo-OS/tla_env.sh                      │
│ TLA+ Website:       https://lamport.azurewebsites.net/tla/             │
└─────────────────────────────────────────────────────────────────────────┘

═══════════════════════════════════════════════════════════════════════════

Version: Java 11.0.30, TLA+ 2026.04.09.014118
Tested: ✅ Java, ✅ TLA+, ✅ TLC Model Checking
Setup Date: 2026-04-17

═══════════════════════════════════════════════════════════════════════════

EOF
