//! Micro-benchmark isolating the single-parse optimization.
//!
//! Run explicitly (it is `#[ignore]`d so it never runs in CI):
//!
//! ```text
//! cargo test --release --test scanner_bench -- --ignored --nocapture
//! ```
//!
//! It scans axon's own `src/` two ways on the same in-memory sources:
//! - OLD: `extract_live_dependencies` + `scan_file` + `extract_calls` → 3 parses/file
//! - NEW: `scan_source` → 1 parse/file

#![allow(clippy::print_stdout, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use axon::domain::rust_syn::RustSynScanner;
use axon::domain::scanner::AstScanner;

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

#[test]
#[ignore = "benchmark; run with --ignored --nocapture"]
fn bench_single_vs_triple_parse() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&root, &mut files);
    let sources: Vec<(PathBuf, String)> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|s| (p.clone(), s)))
        .collect();
    assert!(!sources.is_empty(), "no source files found under {root:?}");

    let scanner = RustSynScanner;
    let iters = 20;

    // Warm caches/branch predictors.
    for (p, s) in &sources {
        let _ = scanner.scan_source(p, s);
    }

    let t_old = Instant::now();
    for _ in 0..iters {
        for (p, s) in &sources {
            let _ = scanner.extract_live_dependencies(p, s).unwrap();
            let _ = scanner.scan_file(p, s).unwrap();
            let _ = scanner.extract_calls(p, s).unwrap();
        }
    }
    let old = t_old.elapsed();

    let t_new = Instant::now();
    for _ in 0..iters {
        for (p, s) in &sources {
            let _ = scanner.scan_source(p, s).unwrap();
        }
    }
    let new = t_new.elapsed();

    let per = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / iters as f64;
    println!("\nfiles={} iters={}", sources.len(), iters);
    println!("OLD  (3 parses/file): {:>8.2} ms/scan", per(old));
    println!("NEW  (1 parse/file):  {:>8.2} ms/scan", per(new));
    println!("speedup: {:.2}x\n", old.as_secs_f64() / new.as_secs_f64());
}
