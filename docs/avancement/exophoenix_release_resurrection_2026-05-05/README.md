# ExoPhoenix Release Resurrection Proof - 2026-05-05

This folder contains the release-mode QEMU proof bundle for ExoPhoenix.

## Result

- `exo-os-phoenix-release.iso` booted under QEMU.
- Kernel A triggered a controlled Ring 0 divide-error (`#DE`).
- Kernel B detected the stopped heartbeat and entered ExoPhoenix handoff.
- The IOMMU handoff window was locked.
- Forge verified the clean Kernel A image contract.
- ExoFS clean image reload completed.
- Kernel A relaunched and emitted `RESURRECTION_OK`.
- QEMU exited through `isa-debug-exit` with `QEMU_STATUS:33`.

## Key Files

| File | Purpose |
|---|---|
| `resurrection-release-e9.log` | Human-readable QEMU debug-port proof log |
| `resurrection-release-status.txt` | QEMU exit status for the resurrection run |
| `resurrection-release-idt-excerpt.txt` | QEMU interrupt excerpt proving real CPL0 `#DE` with valid IDT |
| `resurrection-release-int.log.gz` | Compressed raw QEMU `-d int,cpu_reset` log for the resurrection run |
| `normal-release-e9.log` | Normal release boot log, without autodestruction |
| `normal-release-status.txt` | Normal release QEMU timeout status (`124`, expected idle timeout) |
| `normal-release-int.log.gz` | Compressed raw QEMU interrupt/reset log for normal release boot |
| `make-build-after-idt.log` | `make build` validation after the IDT fix |
| `make-test-after-idt.log` | Unit test validation after the IDT fix |
| `run-tests-after-idt.log` | Standard integration test script output |
| `tla-sany-after-idt.log` | TLA+ SANY validation output |
| `tla-tlc-sim-after-idt.log` | TLA+ TLC simulation output |

## Timing Calibration Note

The E9 logs contain:

```text
[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]
```

This is expected on the current QEMU TCG path. When PIT driver calibration is unavailable or too slow, ExoOS intentionally falls back to the 3 GHz constant-TSC calibration path. This is a timing fallback, not an ExoPhoenix degradation.
