use super::*;
use std::cell::Cell;

fn timestamp_only() -> Maylily {
    Maylily::new(Options {
        radix: 10,
        time_base: 100,
        machine_id: 0,
        machine_bits: 0,
        generator_id: 0,
        generator_bits: 0,
        sequence_bits: 0,
    })
    .unwrap()
}

#[test]
fn blocking_generation_waits_until_the_clock_advances() {
    let mut generator = timestamp_only();
    assert_eq!(generator.generate_at(100).unwrap(), "0");
    let mut times = [100, 100, 101].into_iter();
    let waits = Cell::new(0);
    let result = generator.generate_with(
        || Ok(times.next().expect("retry loop read too many timestamps")),
        || waits.set(waits.get() + 1),
    );
    assert_eq!(result.unwrap(), "1");
    assert_eq!(waits.get(), 2);
    assert_eq!(times.next(), None);
}

#[test]
fn rollback_during_exhaustion_is_returned_without_discarding_state() {
    let mut generator = timestamp_only();
    generator.generate_at(100).unwrap();
    let mut times = [100, 99].into_iter();
    let waits = Cell::new(0);
    assert_eq!(
        generator.generate_with(
            || Ok(times.next().expect("rollback must not be retried")),
            || waits.set(waits.get() + 1),
        ),
        Err(Error::ClockRollback {
            now_ms: 99,
            last_ms: 100
        }),
    );
    assert_eq!(waits.get(), 1);
    assert_eq!(generator.generate_at(101).unwrap(), "1");
}

#[test]
fn system_clock_errors_are_returned_without_waiting_or_consuming_an_id() {
    let mut generator = timestamp_only();
    assert_eq!(
        generator.generate_with(|| Err(Error::SystemClock), || panic!("must not wait")),
        Err(Error::SystemClock),
    );
    assert_eq!(generator.generate_at(100).unwrap(), "0");
}

#[test]
fn full_width_sequence_does_not_wrap_at_u64_max() {
    let mut generator = timestamp_only();
    generator.options.sequence_bits = 64;
    generator.previous = Some((100, u64::MAX));
    assert_eq!(generator.generate_at(100), Err(Error::SequenceOverflow));
    assert_eq!(generator.previous, Some((100, u64::MAX)));
    assert_eq!(generator.generate_at(101).unwrap(), "18446744073709551616");
}

#[cfg(feature = "async")]
#[tokio::test(start_paused = true)]
async fn async_generation_awaits_each_retry_until_the_clock_advances() {
    let mut generator = timestamp_only();
    generator.generate_at(100).unwrap();
    let mut times = [100, 100, 101].into_iter();
    let start = tokio::time::Instant::now();
    assert_eq!(
        generator
            .generate_async_with(|| Ok(times.next().expect("too many retries")))
            .await
            .unwrap(),
        "1"
    );
    assert_eq!(start.elapsed(), Duration::from_millis(2));
    assert_eq!(times.next(), None);
}

#[cfg(feature = "async")]
#[tokio::test(start_paused = true)]
async fn async_generation_returns_clock_errors_and_keeps_previous_state() {
    let mut generator = timestamp_only();
    generator.generate_at(100).unwrap();
    let mut times = [100, 99].into_iter();
    assert_eq!(
        generator
            .generate_async_with(|| Ok(times.next().expect("must stop on rollback")))
            .await,
        Err(Error::ClockRollback {
            now_ms: 99,
            last_ms: 100
        })
    );
    assert_eq!(
        generator
            .generate_async_with(|| Err(Error::SystemClock))
            .await,
        Err(Error::SystemClock)
    );
    assert_eq!(generator.generate_at(101).unwrap(), "1");
}

#[cfg(feature = "async")]
#[tokio::test(start_paused = true)]
async fn cancelling_async_wait_does_not_consume_an_id() {
    let mut generator = timestamp_only();
    generator.generate_at(100).unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(2),
            generator.generate_async_with(|| Ok(100))
        )
        .await
        .is_err()
    );
    assert_eq!(generator.generate_at(100), Err(Error::SequenceOverflow));
    assert_eq!(generator.generate_at(101).unwrap(), "1");
}

#[cfg(feature = "async")]
#[tokio::test]
async fn sync_and_async_generation_share_the_same_sequence_state() {
    let mut generator = Maylily::new(Options {
        generator_id: 0,
        ..Options::default()
    })
    .unwrap();
    let first = generator
        .generate_async()
        .await
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let second = generator.generate().unwrap().parse::<u128>().unwrap();
    let third = generator
        .generate_async()
        .await
        .unwrap()
        .parse::<u128>()
        .unwrap();
    assert!(first < second && second < third);
}
