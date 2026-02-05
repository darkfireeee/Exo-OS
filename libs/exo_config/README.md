# exo_config

Configuration management system for Exo-OS with hierarchical merging and hot-reload support.

## Features

- **Multi-source loading**: TOML files, environment variables, defaults
- **Hierarchical merging**: user > system > default
- **Hot-reload**: File watching with automatic reload
- **Validation**: Schema validation
- **Migration**: Auto-migration of old config formats

## Architecture

```
exo_config/
├── src/
│   ├── loader.rs       # Config loading from multiple sources
│   ├── validator.rs    # Schema validation
│   ├── watcher.rs      # Hot-reload file watching
│   ├── merger.rs       # Hierarchical merge logic
│   └── migrate.rs      # Config migration
```

## Usage

```rust
use exo_config::ConfigLoader;

let config = ConfigLoader::new()
    .add_file("/etc/exo-os/config.toml")
    .add_file("~/.config/exo-os/config.toml")
    .load()?;
```

### Hot-reload

```rust
config.watch(|new_config| {
    println!("Config reloaded!");
});
```

### Validation

```rust
use exo_config::Schema;

let schema = Schema::from_file("schema.json")?;
config.validate(&schema)?;
```

## Configuration Hierarchy

1. **Default** (`/etc/exo-os/default.toml`)
2. **System** (`/etc/exo-os/config.toml`)
3. **User** (`~/.config/exo-os/config.toml`)
4. **Environment** (EXO_* variables)

## Performance

- Lazy loading of unused sections
- Memoized parsing (cached results)
- Minimal memory footprint

## References

- [TOML Spec](https://toml.io/en/v1.0.0)
