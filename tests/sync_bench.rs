//! Breaks a watcher sync into its phases so we can see where time goes after
//! the single-parse optimization.
//!
//! ```text
//! cargo test --release --test sync_bench -- --ignored --nocapture
//! ```

#![allow(clippy::print_stdout, clippy::unwrap_used)]

use std::path::Path;
use std::time::{Duration, Instant};

use axon::domain::analyze::scan_actual_model;
use axon::store::Store;
use axon::store::canonicalize_path;

#[test]
#[ignore = "benchmark; run with --ignored --nocapture"]
fn bench_sync_breakdown() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ws = canonicalize_path(&root.to_string_lossy());
    let store = Store::open(root).unwrap();

    // Warm up (parse caches, store schema).
    let warm = scan_actual_model(root, None).unwrap();
    store.save_actual(&ws, &warm).unwrap();
    store.compute_drift(&ws).unwrap();

    let iters = 10;
    let (mut scan, mut save, mut drift) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    for _ in 0..iters {
        let t = Instant::now();
        let model = scan_actual_model(root, None).unwrap();
        scan += t.elapsed();

        let t = Instant::now();
        store.save_actual(&ws, &model).unwrap();
        save += t.elapsed();

        let t = Instant::now();
        store.compute_drift(&ws).unwrap();
        drift += t.elapsed();
    }

    let ms = |d: Duration| d.as_secs_f64() * 1000.0 / iters as f64;
    let total = ms(scan) + ms(save) + ms(drift);
    println!("\nper full sync (excludes process/store startup):");
    println!(
        "  scan_actual_model : {:>7.1} ms  ({:>4.1}%)",
        ms(scan),
        100.0 * ms(scan) / total
    );
    println!(
        "  save_actual       : {:>7.1} ms  ({:>4.1}%)",
        ms(save),
        100.0 * ms(save) / total
    );
    println!(
        "  compute_drift     : {:>7.1} ms  ({:>4.1}%)",
        ms(drift),
        100.0 * ms(drift) / total
    );
    println!("  total             : {total:>7.1} ms\n");
}
