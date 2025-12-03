# ⏱️ Scheduler - Ordonnanceur 3-Queue EMA

## Vue d'ensemble

L'ordonnanceur d'Exo-OS utilise un système **3-Queue avec prédiction EMA** pour optimiser les performances et minimiser la latence des context switches.

## Architecture

```
kernel/src/scheduler/
├── core/
│   ├── scheduler.rs      # Scheduler principal 3-Queue
│   ├── affinity.rs       # Affinité CPU
│   ├── statistics.rs     # Stats globales
│   └── predictive.rs     # Ordonnanceur prédictif
├── thread/               # Gestion des threads
├── switch/               # Context switching (304 cycles)
├── prediction/           # Algorithmes EMA
├── realtime/             # Temps réel
└── idle.rs               # Idle thread
```

## Performance

| Métrique | Exo-OS | Linux CFS |
|----------|--------|-----------|
| Context switch | 304 cycles | ~1500 cycles |
| Scheduling decision | ~50 cycles | ~200 cycles |
| Thread spawn | ~2000 cycles | ~10000 cycles |

## Modules

- [3-Queue System](./3_queue.md)
- [EMA Prediction](./ema_prediction.md)
- [Context Switch](./context_switch.md)
- [Real-Time](./realtime.md)
