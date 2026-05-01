use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("servers/syscall_abi must live two levels below repo root")
        .to_path_buf()
}

fn parse_sys_u64_consts(path: &Path) -> BTreeMap<String, u64> {
    let src = fs::read_to_string(path).expect("source file must be readable");
    let mut values = BTreeMap::new();
    let mut pending = Vec::new();

    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, rest)) = rest.split_once(": u64") else {
            continue;
        };
        if !name.starts_with("SYS_") {
            continue;
        }
        let Some((_, expr)) = rest.split_once('=') else {
            continue;
        };
        let expr = expr
            .split("//")
            .next()
            .unwrap_or(expr)
            .trim()
            .trim_end_matches(';')
            .trim();
        if let Ok(value) = expr.parse::<u64>() {
            values.insert(name.to_string(), value);
        } else {
            pending.push((name.to_string(), expr.to_string()));
        }
    }

    let mut progressed = true;
    while progressed && !pending.is_empty() {
        progressed = false;
        let mut next = Vec::new();
        for (name, expr) in pending {
            if let Some(value) = values.get(&expr).copied() {
                values.insert(name, value);
                progressed = true;
            } else {
                next.push((name, expr));
            }
        }
        pending = next;
    }

    values
}

#[test]
fn syscall_contract_stress_kernel_numbers_are_exported_by_abi() {
    let root = repo_root();
    let kernel = parse_sys_u64_consts(&root.join("kernel/src/syscall/numbers.rs"));
    let abi = parse_sys_u64_consts(&root.join("servers/syscall_abi/src/lib.rs"));

    let mut mismatches = Vec::new();
    for (name, kernel_value) in &kernel {
        match abi.get(name) {
            Some(abi_value) if abi_value == kernel_value => {}
            Some(abi_value) => {
                mismatches.push(format!("{name}: kernel={kernel_value}, abi={abi_value}"))
            }
            None => mismatches.push(format!("{name}: missing from ABI")),
        }
    }

    assert!(
        mismatches.is_empty(),
        "kernel/syscall ABI drift:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn syscall_contract_stress_exofs_range_is_complete_and_contiguous() {
    let root = repo_root();
    let abi = parse_sys_u64_consts(&root.join("servers/syscall_abi/src/lib.rs"));

    let expected = [
        ("SYS_EXOFS_PATH_RESOLVE", 500),
        ("SYS_EXOFS_OBJECT_OPEN", 501),
        ("SYS_EXOFS_OBJECT_READ", 502),
        ("SYS_EXOFS_OBJECT_WRITE", 503),
        ("SYS_EXOFS_OBJECT_CREATE", 504),
        ("SYS_EXOFS_OBJECT_DELETE", 505),
        ("SYS_EXOFS_OBJECT_STAT", 506),
        ("SYS_EXOFS_OBJECT_SET_META", 507),
        ("SYS_EXOFS_GET_CONTENT_HASH", 508),
        ("SYS_EXOFS_SNAPSHOT_CREATE", 509),
        ("SYS_EXOFS_SNAPSHOT_LIST", 510),
        ("SYS_EXOFS_SNAPSHOT_MOUNT", 511),
        ("SYS_EXOFS_RELATION_CREATE", 512),
        ("SYS_EXOFS_RELATION_QUERY", 513),
        ("SYS_EXOFS_GC_TRIGGER", 514),
        ("SYS_EXOFS_QUOTA_QUERY", 515),
        ("SYS_EXOFS_EXPORT_OBJECT", 516),
        ("SYS_EXOFS_IMPORT_OBJECT", 517),
        ("SYS_EXOFS_EPOCH_COMMIT", 518),
        ("SYS_EXOFS_OPEN_BY_PATH", 519),
        ("SYS_EXOFS_READDIR", 520),
    ];

    for (name, value) in expected {
        assert_eq!(abi.get(name).copied(), Some(value), "{name}");
    }

    let mut seen = [false; 21];
    for (name, value) in &abi {
        if !name.starts_with("SYS_EXOFS_")
            || matches!(
                name.as_str(),
                "SYS_EXOFS_FIRST" | "SYS_EXOFS_LAST" | "SYS_EXOFS_COUNT"
            )
        {
            continue;
        }
        assert!(
            (500..=520).contains(value),
            "{name} escaped ExoFS range with {value}"
        );
        let idx = (*value - 500) as usize;
        assert!(!seen[idx], "duplicate ExoFS syscall number {value}");
        seen[idx] = true;
    }

    assert!(seen.into_iter().all(|present| present));
}
