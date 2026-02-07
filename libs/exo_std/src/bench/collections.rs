//! Collection benchmarks
//!
//! Benchmarks for HashMap, BTreeMap, Vec, and other collections.

extern crate alloc;

use alloc::vec::Vec;
use crate::bench::Benchmark;
use crate::collections::{HashMap, BTreeMap};
use crate::println;

/// Benchmark HashMap insert
pub fn bench_hashmap_insert() {
    let _result = Benchmark::new("HashMap insert")
        .iterations(1000)
        .run(|| {
            let mut map = HashMap::new();
            for i in 0..100 {
                map.insert(i, i * 2);
            }
        });

    #[cfg(feature = "test_mode")]
    {
        println!("HashMap insert: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark HashMap get
pub fn bench_hashmap_get() {
    let mut map = HashMap::new();
    for i in 0..100 {
        map.insert(i, i * 2);
    }

    let _result = Benchmark::new("HashMap get")
        .iterations(10000)
        .run(|| {
            for i in 0..100 {
                let _ = map.get(&i);
            }
        });

    #[cfg(feature = "test_mode")]
    {
        println!("HashMap get: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark HashMap remove
pub fn bench_hashmap_remove() {
    let _result = Benchmark::new("HashMap remove")
        .iterations(1000)
        .run(|| {
            let mut map = HashMap::new();
            for i in 0..100 {
                map.insert(i, i * 2);
            }
            for i in 0..100 {
                map.remove(&i);
            }
        });

    #[cfg(feature = "test_mode")]
    {
        println!("HashMap remove: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark BTreeMap insert
pub fn bench_btreemap_insert() {
    let _result = Benchmark::new("BTreeMap insert")
        .iterations(1000)
        .run(|| {
            let mut map = BTreeMap::new();
            for i in 0..100 {
                map.insert(i, i * 2);
            }
        });

    #[cfg(feature = "test_mode")]
    {
        println!("BTreeMap insert: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark BTreeMap get
pub fn bench_btreemap_get() {
    let mut map = BTreeMap::new();
    for i in 0..100 {
        map.insert(i, i * 2);
    }

    let _result = Benchmark::new("BTreeMap get")
        .iterations(10000)
        .run(|| {
            for i in 0..100 {
                let _ = map.get(&i);
            }
        });

    #[cfg(feature = "test_mode")]
    {
        println!("BTreeMap get: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark Vec push
pub fn bench_vec_push() {
    let _result = Benchmark::new("Vec push")
        .iterations(10000)
        .run(|| {
            let mut v = Vec::new();
            for i in 0..100 {
                v.push(i);
            }
        });

    #[cfg(feature = "test_mode")]
    {
        println!("Vec push: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Run all collection benchmarks
pub fn run_all() {
    bench_hashmap_insert();
    bench_hashmap_get();
    bench_hashmap_remove();
    bench_btreemap_insert();
    bench_btreemap_get();
    bench_vec_push();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bench_hashmap() {
        bench_hashmap_insert();
        bench_hashmap_get();
    }

    #[test]
    fn test_bench_btreemap() {
        bench_btreemap_insert();
        bench_btreemap_get();
    }
}
