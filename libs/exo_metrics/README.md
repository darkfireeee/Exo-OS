# exo_metrics

Telemetry and metrics collection system for Exo-OS with Prometheus-compatible export.

## Features

- **Counters**: Monotonic counters (requests served, bytes transferred)
- **Gauges**: Point-in-time values (CPU usage, memory usage)
- **Histograms**: Distribution metrics (latency P50/P95/P99)
- **Timers**: Latency measurement helpers
- **Prometheus export**: `/metrics` HTTP endpoint
- **Lock-free**: Thread-local counters with periodic aggregation
- **System metrics**: CPU, memory, I/O, syscall counts

## Architecture

```
exo_metrics/
├── src/
│   ├── registry.rs     # Metrics registration
│   ├── exporters/      # Prometheus exporter
│   ├── aggregator.rs   # Histogram aggregation
│   ├── timer.rs        # Latency timers
│   └── system.rs       # System metrics collector
```

## Usage

### Basic Metrics

```rust
use exo_metrics::{counter, gauge, histogram};

counter!("requests_total", 1);
gauge!("cpu_usage", 42.5);
histogram!("request_latency_ms", duration.as_millis());
```

### Timers

```rust
use exo_metrics::Timer;

let timer = Timer::start("api_request");
// ... do work ...
timer.stop(); // Automatically records to histogram
```

### Prometheus Export

```rust
use exo_metrics::PrometheusExporter;

let exporter = PrometheusExporter::new();
let metrics = exporter.export(); // Prometheus text format
```

### System Metrics

```rust
use exo_metrics::system;

system::record_cpu_usage();
system::record_memory_usage();
system::record_io_throughput();
```

## Metrics

| Name | Type | Description |
|------|------|-------------|
| `cpu_usage_percent` | Gauge | CPU utilization (0-100) |
| `memory_bytes` | Gauge | Memory usage in bytes |
| `syscalls_total` | Counter | Total syscall count |
| `io_read_bytes` | Counter | Bytes read |
| `io_write_bytes` | Counter | Bytes written |
| `ipc_messages_total` | Counter | IPC message count |
| `request_latency_ms` | Histogram | Request latency |

## Performance

- **Overhead**: <1% CPU for typical workloads
- **Atomic operations**: Lock-free counters
- **Thread-local**: Per-thread caching, periodic aggregation
- **Memory**: ~100 bytes per metric

## Prometheus Format

```
# HELP cpu_usage_percent CPU utilization percentage
# TYPE cpu_usage_percent gauge
cpu_usage_percent 42.5

# HELP requests_total Total requests served
# TYPE requests_total counter
requests_total 12345
```

## References

- [Prometheus Exposition Format](https://prometheus.io/docs/instrumenting/exposition_formats/)
- [HdrHistogram](http://hdrhistogram.org/)
