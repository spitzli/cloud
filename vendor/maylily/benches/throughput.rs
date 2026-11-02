use maylily::{Maylily, Options};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

const IDS: u64 = 10_000;
const SAMPLES: usize = 9;
const FIXED_NOW_MS: u64 = 1_800_000_000_000;

fn options(sequence_bits: u8) -> Options {
    Options {
        machine_id: 1,
        generator_id: 2,
        sequence_bits,
        ..Options::default()
    }
}

fn measure(name: &str, mut run: impl FnMut() -> Duration) -> Value {
    black_box(run()); // Warm up before collecting samples.
    let mut samples = (0..SAMPLES)
        .map(|_| run().as_secs_f64())
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    let median = samples[SAMPLES / 2];
    json!({
        "case": name,
        "operations_per_sample": IDS,
        "samples": SAMPLES,
        "median_ms": median * 1000.0,
        "min_ms": samples[0] * 1000.0,
        "max_ms": samples[SAMPLES - 1] * 1000.0,
        "median_ns_per_operation": median * 1e9 / IDS as f64,
        "operations_per_second": IDS as f64 / median,
        "sample_ms": samples.iter().map(|seconds| seconds * 1000.0).collect::<Vec<_>>(),
    })
}

fn direct() -> Duration {
    let mut generator = Maylily::new(options(8)).unwrap();
    let start = Instant::now();
    for i in 0..IDS {
        black_box(
            generator
                .generate_at(black_box(FIXED_NOW_MS + i / 256))
                .unwrap(),
        );
    }
    start.elapsed()
}

fn sync(sequence_bits: u8) -> Duration {
    let mut generator = Maylily::new(options(sequence_bits)).unwrap();
    let start = Instant::now();
    for _ in 0..IDS {
        black_box(generator.generate().unwrap());
    }
    start.elapsed()
}

#[cfg(feature = "async")]
async fn asynchronous(sequence_bits: u8) -> Duration {
    let mut generator = Maylily::new(options(sequence_bits)).unwrap();
    let start = Instant::now();
    for _ in 0..IDS {
        black_box(generator.generate_async().await.unwrap());
    }
    start.elapsed()
}

fn main() {
    // Independent known values catch accidental layout or fixture changes.
    let config = options(8);
    let mut check = Maylily::new(config).unwrap();
    assert_eq!(check.generate_at(config.time_base).unwrap(), "262656");
    assert_eq!(check.generate_at(config.time_base).unwrap(), "262657");

    let mut results = vec![
        measure("direct_8bit", direct),
        measure("sync_8bit", || sync(8)),
        measure("sync_16bit", || sync(16)),
    ];

    #[cfg(feature = "async")]
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        results.push(measure("async_8bit", || runtime.block_on(asynchronous(8))));
        results.push(measure("async_16bit", || {
            runtime.block_on(asynchronous(16))
        }));
    }

    #[cfg(feature = "serde")]
    {
        let mut generator = Maylily::new(config).unwrap();
        let ids = (0..IDS)
            .map(|i| generator.generate_at(FIXED_NOW_MS + i / 256).unwrap())
            .collect::<Vec<_>>();
        let encoded = serde_json::to_string(&ids[0]).unwrap();
        assert_eq!(serde_json::from_str::<String>(&encoded).unwrap(), ids[0]);
        results.push(measure("id_json_encode", || {
            let start = Instant::now();
            for id in &ids {
                black_box(serde_json::to_string(black_box(id)).unwrap());
            }
            start.elapsed()
        }));
        results.push(measure("options_json_roundtrip", || {
            let start = Instant::now();
            for _ in 0..IDS {
                let encoded = serde_json::to_string(black_box(&config)).unwrap();
                black_box(serde_json::from_str::<Options>(black_box(&encoded)).unwrap());
            }
            start.elapsed()
        }));
    }

    results.sort_by(|a, b| a["case"].as_str().cmp(&b["case"].as_str()));
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "benchmark": "maylily-throughput",
            "features": {"serde": cfg!(feature = "serde"), "async": cfg!(feature = "async")},
            "warmup_samples": 1,
            "samples_per_case": SAMPLES,
            "operations_per_sample": IDS,
            "setup_timed": false,
            "async_runtime": cfg!(feature = "async").then_some("Tokio current-thread; runtime construction excluded"),
            "results": results,
        }))
        .unwrap()
    );
}
