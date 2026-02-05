# exo_text

Text processing and parsing utilities for Exo-OS.

## Features

- **JSON**: Streaming JSON parser with SIMD optimizations
- **TOML**: Configuration file parser (serde-based)
- **Markdown**: Parser and renderer (HTML + terminal output)
- **Diff**: Unified diff generator

## Architecture

```
exo_text/
├── src/
│   ├── json/           # JSON parser/serializer
│   ├── toml/           # TOML deserializer
│   ├── markdown/       # Markdown parser/renderer
│   └── diff/           # Diff generator
└── benches/            # Performance benchmarks
```

## Usage

### JSON Parsing

```rust
use exo_text::json;

let data = r#"{"name": "Exo-OS", "version": 1}"#;
let parsed = json::parse(data)?;
```

### TOML Configuration

```rust
use exo_text::toml;

#[derive(Deserialize)]
struct Config {
    name: String,
    port: u16,
}

let config: Config = toml::from_str(toml_str)?;
```

### Markdown Rendering

```rust
use exo_text::markdown;

let html = markdown::to_html("# Title\n\nParagraph");
let terminal = markdown::to_terminal("**bold** text");
```

### Unified Diff

```rust
use exo_text::diff;

let diff = diff::unified("old content", "new content");
println!("{}", diff);
```

## Benchmarks

```bash
cargo bench --package exo_text
```

Typical performance:
- **JSON**: 500-800 MB/s parsing (SIMD enabled)
- **TOML**: 50-100 MB/s parsing
- **Markdown**: 100-200 MB/s rendering

## Features

Enable specific parsers:

```toml
exo_text = { version = "0.1", features = ["json", "markdown"] }
```

## References

- [JSON Spec (RFC 8259)](https://www.rfc-editor.org/rfc/rfc8259)
- [TOML Spec](https://toml.io/en/v1.0.0)
- [CommonMark Spec](https://commonmark.org/)
