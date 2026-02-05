# exo_logger

Structured logging system for Exo-OS with span-based contexts and async buffering.

## Features

- **Structured logging**: JSON Lines format
- **Span contexts**: Distributed tracing support
- **File rotation**: Automatic rotation at 10MB
- **Compression**: zstd compression for old logs
- **Async buffering**: Lock-free MPSC channels
- **Dynamic filtering**: Runtime log level changes
- **Correlation IDs**: Request tracking across services

## Architecture

```
exo_logger/
├── src/
│   ├── collector.rs    # Log collector (multi-source)
│   ├── formatter/      # JSON/Pretty formatters
│   ├── sink/           # File/IPC sinks
│   ├── filter.rs       # Dynamic filtering
│   └── span.rs         # Span-based contexts
```

## Usage

### Basic Logging

```rust
use exo_logger::{info, error};

info!("Server started", port = 8080);
error!("Connection failed", error = "timeout");
```

### Span Contexts

```rust
use exo_logger::span;

let _span = span!("request", method = "GET", path = "/api");
info!("Processing request");
// Logs include span context automatically
```

### Configuration

```rust
use exo_logger::LoggerBuilder;

LoggerBuilder::new()
    .level(Level::Info)
    .format(Format::Json)
    .output("/var/log/exo-os/app.log")
    .rotation(10_000_000) // 10MB
    .compression(true)
    .build()?;
```

## Log Levels

- `TRACE`: Detailed debugging
- `DEBUG`: Debug information
- `INFO`: Informational messages
- `WARN`: Warning messages
- `ERROR`: Error conditions
- `FATAL`: Fatal errors

## Performance

- **Throughput**: 1M+ logs/sec (async mode)
- **Latency**: <1μs (buffered)
- **Overhead**: <5% CPU in typical workloads

## Output Formats

### JSON Lines

```json
{"timestamp":"2026-02-05T...", "level":"INFO", "message":"...", "span_id":"..."}
```

### Pretty (Development)

```
[INFO ] 2026-02-05 12:34:56 - Server started (port=8080)
```

## References

- [Tracing Ecosystem](https://tokio.rs/tokio/topics/tracing)
- [JSON Lines](https://jsonlines.org/)
