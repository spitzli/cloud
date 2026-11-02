# maylily-rs

A reusable Rust crate implementing the ID format described by
[Maylily](https://github.com/shimataro/maylily) and its
[Deno port](https://github.com/itohatweb/deno-maylily). Independently implemented
in Rust; no JavaScript runtime or external ID server is required.
This crate is licensed under MIT (see the `LICENSE` file) and has not yet been published to crates.io.

IDs contain elapsed milliseconds, machine ID, generator ID, and sequence, in that
order. Packing uses `num-bigint`, so the result is not bounded to 64 or 128 bits.
Output is a string in any radix from 2 through 36, using lowercase letters.

```rust
use maylily::{Maylily, Options};

let mut generator = Maylily::new(Options {
    machine_id: 1,
    generator_id: 2, // assign this pair exclusively to this generator
    ..Options::default()
})?;
let id: String = generator.generate()?;
# Ok::<(), maylily::Error>(())
```

Defaults match the references: radix 10, Unix time base `946684800000`
(2000-01-01 UTC), machine ID 0 with 3 bits, generator ID `PID % 1024` with 10
bits, and 8 sequence bits. Rust field names use snake_case (`time_base`,
`machine_bits`, etc.). Each instance retains its own immutable options; there is
no mutable global generator. To use default configuration, pass
`Options::default()` to `Maylily::new`.

### Custom epoch, for example January 2017

`time_base` is Unix milliseconds, equivalent to JavaScript's `timeBase`.
`Date.parse("2017-01-01T00:00:00Z")` is `1_483_228_800_000`:

```rust
use maylily::{Maylily, Options};

let options = Options {
    time_base: 1_483_228_800_000, // 2017-01-01 00:00:00 UTC
    machine_id: 1,
    generator_id: 2,
    ..Options::default()
};
let mut generator = Maylily::new(options)?;
let id = generator.generate_at(1_483_228_800_001)?;
assert_eq!(id, "2359808");
# Ok::<(), maylily::Error>(())
```

Earlier timestamps return `BeforeEpoch`. Keep an established namespace's epoch
and bit layout fixed: changing the epoch changes ID interpretation and can cause
collisions with existing IDs. No date-parsing dependency is needed for a fixed
epoch constant.

Machine, generator, and sequence widths each accept 0–64 bits. A zero-width field
can only contain zero. Timestamps, time base and field values use `u64`; the
combined ID can occupy up to 256 bits. This is a deliberate field-size bound,
while both references use arbitrary-precision integers for the packed result.
Invalid widths, identities, radix and before-epoch timestamps return errors.

`generate_at(now_ms)` accepts an explicit Unix timestamp, increments the sequence
within that millisecond and resets it to zero when time advances. It rejects clock
rollback and returns `SequenceOverflow` when that millisecond is full; a rejected
attempt leaves the last successful state intact. `generate()` reads the system
clock and blocks, sleeping 1 ms between retries on exhaustion. Async callers
can enable the `async` feature for `generate_async()`, or call `generate_at` and
wait with their runtime's timer. By default this crate has no runtime dependency.
Options cannot change midway
through a sequence; create separately coordinated namespaces for other layouts.

`Options::pack_at(now_ms, sequence)` exposes pure packing as a `BigUint`, for
callers that manage their own allocation and persistence. It does not reserve an
ID. The format is:

```text
(((elapsed_ms * 2^machine_bits + machine_id)
              * 2^generator_bits + generator_id)
              * 2^sequence_bits + sequence)
```

Unique IDs require unique machine/generator pairs under a single fixed layout.
PIDs can repeat or collide, especially across containers. Multiple instances with
the same identity and time/sequence collide. This crate does not assign machine
IDs, persist state, fence previous owners, or guarantee safety after restarts or
clock rollback. It does not provide random or secret IDs. Keep IDs as strings or
BigInt in JavaScript; never convert them to Number.

This is a standalone Cargo project with its own manifest, lockfile and lint
configuration. It has no platform-specific dependencies.

```toml
[dependencies]
maylily = { git = "https://github.com/itohatweb/maylily-rs", branch = "main" }
```

The Git dependency requires a revision containing this Rust implementation.
Use a local path dependency while working on an unmerged contribution.

## Optional features

All features are off by default. Enable only the integrations you need:

```toml
[dependencies]
maylily = { path = "../maylily-rs", features = ["serde", "async", "parse"] }
```

- `serde` adds `Serialize` and `Deserialize` to `Options` and, with `parse`, to
  `ParsedId`. This supports configuration files and decoded data in Serde formats.
  All fields are required; unknown fields and wrong types
  are rejected. `Maylily::new` and `pack_at` still validate field ranges before
  generation. Generated IDs are already strings and need no custom serializer.
  Generator sequence state is not serializable. This feature does not enable
  `num-bigint`'s separate Serde representation.
- `async` adds `Maylily::generate_async()`, using a
  [Tokio timer](https://docs.rs/tokio/latest/tokio/time/fn.sleep.html) while the
  current millisecond's sequence is full. It requires a Tokio runtime with
  timers enabled; without one it may panic when waiting becomes necessary.
  Tokio's runtime, macros and test utilities are not enabled by this library's
  production dependency. Consumers configure their runtime themselves.
- `parse` adds `Options::parse(&str)` and `ParsedId`, without new dependencies.
  It decodes absolute Unix milliseconds, elapsed milliseconds, machine ID,
  generator ID and sequence. It uses the configured radix, epoch and bit widths;
  the configured producer IDs are ignored. Input must use canonical lowercase
  digits, without signs, whitespace or leading zeros (except `"0"`). Parsing
  rejects malformed IDs and timestamp overflow; it does not change generator state.

### Decode an ID

```rust
# #[cfg(feature = "parse")]
# {
use maylily::{Maylily, Options};

let options = Options {
    time_base: 1_483_228_800_000,
    machine_id: 1,
    generator_id: 2,
    ..Options::default()
};
let mut generator = Maylily::new(options)?;
let id = generator.generate_at(1_483_228_800_001)?;
let parts = options.parse(&id)?;
assert_eq!(parts.timestamp_ms, 1_483_228_800_001);
assert_eq!(parts.elapsed_ms, 1);
assert_eq!(parts.machine_id, 1);
assert_eq!(parts.generator_id, 2);
assert_eq!(parts.sequence, 0);
# }
# Ok::<(), maylily::Error>(())
```

The epoch, radix and bit widths are not embedded in the ID. A different valid
configuration can silently produce different decoded fields: always retain the
original layout. Decoding is not authentication or a durable restart checkpoint.
Decoded fields use `u64`; JSON consumers must preserve integer precision for
values beyond JavaScript's safe integer range.

### Async generation

```rust
# #[cfg(feature = "async")]
async fn next_id(generator: &mut maylily::Maylily) -> Result<String, maylily::Error> {
    generator.generate_async().await
}
```

Sync and async calls share one generator's sequence. Cancelling an async call
while it waits does not consume an ID. Tokio's paused timers do not change the
system wall clock; tests of retry behavior use explicit timestamps.
Serde support follows the [official derive feature](https://serde.rs/feature-flags.html).

## Verification

```sh
cargo test --locked
cargo test --locked --features serde
cargo test --locked --features async
cargo test --locked --features parse
cargo test --locked --all-features
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Tests include the exact sequence, clock advancement, exhaustion and radix vectors
from the [Deno reference tests](https://github.com/itohatweb/deno-maylily/blob/main/test/mod.test.ts),
plus invalid input, custom epochs, zero-width fields and 256-bit output. The
retry loop is tested with a controlled clock and waiter, including repeated
exhaustion, rollback while waiting and clock errors. Additional tests exercise
all 35 valid radices, collision-free small layouts and full-width sequence overflow.
Feature tests cover Serde roundtrips and malformed configuration, async timer
retries, clock failures, cancellation, and mixed sync/async allocation.
`./scripts/check.sh` tests all eight feature combinations, including parsing with
Serde, malformed input, 256-bit boundaries, custom epochs and every radix.

## Small throughput benchmark

```sh
cargo bench --bench throughput --locked
cargo bench --bench throughput --locked --features serde
cargo bench --bench throughput --locked --features async
cargo bench --bench throughput --locked --all-features
```

Each case warms up once and measures nine samples of 10,000 operations in an
optimized build. JSON output includes all sample times, median, minimum, maximum
and throughput. Generator/runtime construction and JSON fixture creation are
excluded from the timer; ID allocation, formatting, dropping returned strings,
and any exhaustion waits are included in generation measurements.

`direct_8bit` uses explicit timestamps advancing once per 256 IDs, measuring
generation without wall-clock reads or waits. `sync_8bit` and `async_8bit` use the
default sequence capacity of 256 IDs per millisecond per identity. The corresponding
`16bit` cases allow 65,536 per millisecond, so this batch does not exhaust a
millisecond. This changes the ID layout and is a separate benchmark configuration,
not a recommendation to change an existing namespace.

The async benchmark awaits calls sequentially on a Tokio current-thread runtime.
It measures per-generator throughput, not concurrent request handling. Timer
granularity and scheduling affect the default-sequence results. Enabling `serde`
alone does not serialize IDs: `id_json_encode` separately measures encoding
pre-generated ID strings, while `options_json_roundtrip` measures 10,000
configuration encode/decode roundtrips. Small differences between feature builds
do not by themselves establish feature overhead.

Local sample results and machine details are in
[`benchmarks/throughput-2026-10-27.json`](benchmarks/throughput-2026-10-27.json).
These synthetic generators reuse test identities across samples; no IDs are
persisted or used for real records.

## Contributing and releases

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md),
[CHANGELOG.md](CHANGELOG.md) and [release instructions](docs/RELEASING.md).
CI and release jobs use GitHub-hosted Ubuntu runners. Publication is started
manually through the release workflow.
