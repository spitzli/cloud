use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    thread,
};
use webdock_ids::{Error, GenerateError, Generator, Id, Identity, Layout, PLATFORM_EPOCH_MS};

fn identity() -> Identity {
    Identity::platform(0, 2).unwrap()
}

#[test]
fn durable_checkpoint_resumes_after_maylily_packed_id() {
    let options = maylily::Options {
        radix: 10,
        time_base: PLATFORM_EPOCH_MS,
        machine_id: 0,
        machine_bits: 6,
        generator_id: 2,
        generator_bits: 4,
        sequence_bits: 12,
    };
    let checkpoint: Id = options
        .pack_at(PLATFORM_EPOCH_MS + 1, 17)
        .unwrap()
        .to_string()
        .parse()
        .unwrap();
    let mut resumed = Generator::resume(Layout::PLATFORM, identity(), Some(checkpoint)).unwrap();
    let mut committed = None;
    let next = resumed
        .next_at(PLATFORM_EPOCH_MS + 1, |id| {
            committed = Some(id);
            Ok::<_, &str>(())
        })
        .unwrap();
    assert_eq!(committed, Some(next));
    assert_eq!(next.as_i64(), checkpoint.as_i64() + 1);
    assert_eq!(options.parse(&next.to_string()).unwrap().sequence, 18);
    assert_eq!(Layout::PLATFORM.decode(next).unwrap().sequence, 18);
}

#[test]
fn platform_layout_preserves_signed_boundary_and_legacy_slots() {
    let layout = Layout::PLATFORM;
    assert_eq!(layout.epoch_ms(), 1_767_225_600_000);
    assert_eq!(layout.bits(), [41, 6, 4, 12]);
    for slot in 0..2 {
        assert_eq!(Identity::platform(0, slot), Err(Error::ReservedIdentity));
    }
    assert!(Identity::platform(64, 0).is_err());
    assert!(Identity::platform(0, 16).is_err());
    let last = layout
        .pack(
            layout.max_timestamp_ms(),
            Identity::platform(63, 15).unwrap(),
            4095,
        )
        .unwrap();
    assert_eq!(last.as_i64(), i64::MAX);
    assert_eq!(
        layout.pack(layout.max_timestamp_ms() + 1, identity(), 0),
        Err(Error::TimestampOverflow)
    );
    assert_eq!(
        layout.pack(PLATFORM_EPOCH_MS - 1, identity(), 0),
        Err(Error::BeforeEpoch)
    );
    assert_eq!(
        layout.pack(PLATFORM_EPOCH_MS, identity(), 4096),
        Err(Error::SequenceOverflow)
    );
    assert_eq!(
        layout
            .decode("4096".parse().unwrap())
            .unwrap()
            .identity
            .generator(),
        1
    );
}

#[test]
fn invalid_layouts_cannot_shift_or_overflow() {
    for bits in [[0, 6, 4, 12], [42, 6, 4, 12], [255, 255, 255, 255]] {
        assert_eq!(
            Layout::new(0, bits[0], bits[1], bits[2], bits[3]),
            Err(Error::InvalidLayout)
        );
    }
    assert_eq!(Layout::new(u64::MAX, 1, 0, 0, 0), Err(Error::InvalidLayout));
    let tiny = Layout::new(0, 1, 0, 0, 0).unwrap();
    let who = Identity::new(tiny, 0, 0).unwrap();
    assert_eq!(tiny.pack(1, who, 0).unwrap().as_i64(), 1);
    assert_eq!(
        tiny.decode("2".parse().unwrap()),
        Err(Error::IdOutsideLayout)
    );
}

#[test]
fn decimal_ids_reject_lossy_json_and_noncanonical_values() {
    for value in ["0", "8192", "9007199254740993", "9223372036854775807"] {
        let id: Id = value.parse().unwrap();
        assert_eq!(id.to_string(), value);
        let json = format!("\"{value}\"");
        assert_eq!(serde_json::to_string(&id).unwrap(), json);
        assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
    }
    for value in [
        "",
        "-1",
        "+1",
        "01",
        " 1",
        "1 ",
        "1.0",
        "1e3",
        "9223372036854775808",
        "١",
    ] {
        assert!(value.parse::<Id>().is_err(), "{value}");
    }
    assert!(Id::try_from(-1).is_err());
    assert!(serde_json::from_str::<Id>("9007199254740993").is_err());
}

#[test]
fn generation_handles_rollback_exhaustion_and_recovery_without_wrapping() {
    let l = Layout::new(100, 3, 1, 1, 1).unwrap();
    let who = Identity::new(l, 1, 0).unwrap();
    let first = l.next_after(who, None, 100).unwrap();
    let second = l.next_after(who, Some(first), 100).unwrap();
    assert_eq!(second.as_i64(), first.as_i64() + 1);
    assert_eq!(
        l.next_after(who, Some(second), 100),
        Err(Error::SequenceOverflow)
    );
    let third = l.next_after(who, Some(second), 101).unwrap();
    assert_eq!(l.decode(third).unwrap().sequence, 0);
    assert_eq!(
        l.next_after(who, Some(third), 100),
        Err(Error::ClockRollback {
            now_ms: 100,
            last_ms: 101
        })
    );
    assert!(l.next_after(who, Some(third), 102).unwrap() > third);
    let other = Identity::new(l, 0, 0).unwrap();
    assert_eq!(
        l.next_after(other, Some(third), 102),
        Err(Error::CheckpointIdentityMismatch)
    );
    let end = l.pack(l.max_timestamp_ms(), who, 1).unwrap();
    assert_eq!(
        l.next_after(who, Some(end), 108),
        Err(Error::TimestampOverflow)
    );
}

#[test]
fn persistence_precedes_release_and_restart_skips_committed_or_uncertain_values() {
    let mut generator = Generator::resume(Layout::PLATFORM, identity(), None).unwrap();
    let mut saved = None;
    let first = generator
        .next_at(PLATFORM_EPOCH_MS, |id| {
            saved = Some(id);
            Ok::<_, &str>(())
        })
        .unwrap();
    assert_eq!(saved, Some(first));
    let result = generator.next_at(PLATFORM_EPOCH_MS, |id| {
        saved = Some(id);
        Err("ack lost")
    });
    assert!(matches!(
        result,
        Err(GenerateError::Persistence("ack lost"))
    ));
    let uncertain = saved.unwrap();
    assert_eq!(uncertain.as_i64(), first.as_i64() + 1);
    let after = generator
        .next_at(PLATFORM_EPOCH_MS, |id| {
            saved = Some(id);
            Ok::<_, &str>(())
        })
        .unwrap();
    assert_eq!(after.as_i64(), uncertain.as_i64() + 1);
    let mut resumed = Generator::resume(Layout::PLATFORM, identity(), saved).unwrap();
    let restarted = resumed
        .next_at(PLATFORM_EPOCH_MS, |_| Ok::<_, &str>(()))
        .unwrap();
    assert_eq!(restarted.as_i64(), after.as_i64() + 1);
}

#[test]
fn independent_instances_need_shared_checkpoint_coordination() {
    let l = Layout::PLATFORM;
    // Equal identities and stale state demonstrably collide: deployment coordination is mandatory.
    assert_eq!(
        l.next_after(identity(), None, PLATFORM_EPOCH_MS),
        l.next_after(identity(), None, PLATFORM_EPOCH_MS)
    );
    let state = Arc::new(Mutex::new(None));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let state = Arc::clone(&state);
            thread::spawn(move || {
                (0..256)
                    .map(|_| {
                        let mut checkpoint = state.lock().unwrap();
                        let next = l
                            .next_after(identity(), *checkpoint, PLATFORM_EPOCH_MS)
                            .unwrap();
                        *checkpoint = Some(next);
                        next
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let all: Vec<_> = threads
        .into_iter()
        .flat_map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(all.len(), 2048);
    assert_eq!(all.into_iter().collect::<HashSet<_>>().len(), 2048);
}

#[test]
fn packing_roundtrips_and_distinct_fields_do_not_overlap() {
    let l = Layout::new(77, 5, 3, 2, 2).unwrap();
    let mut values = HashSet::new();
    for timestamp in 77..109 {
        for machine in 0..8 {
            for generator in 0..4 {
                for sequence in 0..4 {
                    let who = Identity::new(l, machine, generator).unwrap();
                    let id = l.pack(timestamp, who, sequence).unwrap();
                    assert!(values.insert(id));
                    let decoded = l.decode(id).unwrap();
                    assert_eq!(
                        (decoded.timestamp_ms, decoded.identity, decoded.sequence),
                        (timestamp, who, sequence)
                    );
                }
            }
        }
    }
    assert_eq!(values.len(), 4096);
}

#[test]
fn matches_independent_javascript_bigint_fixtures() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/packing.json")).unwrap();
    for case in fixtures.as_array().unwrap() {
        let l = Layout::PLATFORM;
        let who = Identity::platform(
            case["machine"].as_u64().unwrap(),
            case["generator"].as_u64().unwrap(),
        )
        .unwrap();
        let id = l
            .pack(
                case["timestamp_ms"].as_u64().unwrap(),
                who,
                case["sequence"].as_u64().unwrap(),
            )
            .unwrap();
        assert_eq!(id.to_string(), case["id"].as_str().unwrap());
    }
}

#[test]
fn invalid_checkpoints_and_rejected_attempts_do_not_reach_persistence() {
    let l = Layout::PLATFORM;
    let other = Identity::platform(1, 0).unwrap();
    let foreign = l.pack(PLATFORM_EPOCH_MS, other, 0).unwrap();
    assert!(matches!(
        Generator::resume(l, identity(), Some(foreign)),
        Err(Error::CheckpointIdentityMismatch)
    ));
    let legacy = l.decode("4096".parse().unwrap()).unwrap().identity;
    assert!(matches!(
        Generator::resume(l, legacy, None),
        Err(Error::ReservedIdentity)
    ));
    assert_eq!(
        l.pack(PLATFORM_EPOCH_MS, legacy, 0),
        Err(Error::ReservedIdentity)
    );
    let checkpoint = l.pack(PLATFORM_EPOCH_MS + 1, identity(), 4095).unwrap();
    let mut generator = Generator::resume(l, identity(), Some(checkpoint)).unwrap();
    for now in [
        PLATFORM_EPOCH_MS,
        PLATFORM_EPOCH_MS + 1,
        l.max_timestamp_ms() + 1,
    ] {
        assert!(
            generator
                .next_at(now, |_| -> Result<(), &str> {
                    panic!("invalid attempt persisted")
                })
                .is_err()
        );
    }
    let next = generator
        .next_at(PLATFORM_EPOCH_MS + 2, |_| Ok::<_, &str>(()))
        .unwrap();
    assert_eq!(l.decode(next).unwrap().sequence, 0);
}

#[test]
fn timestamp_only_layout_checks_the_full_signed_limit() {
    let layout = Layout::new(0, 63, 0, 0, 0).unwrap();
    let who = Identity::new(layout, 0, 0).unwrap();
    let last = layout.pack(i64::MAX as u64, who, 0).unwrap();
    assert_eq!(last.as_i64(), i64::MAX);
    assert_eq!(layout.decode(last).unwrap().timestamp_ms, i64::MAX as u64);
    assert_eq!(
        layout.next_after(who, Some(last), i64::MAX as u64),
        Err(Error::SequenceOverflow)
    );
    assert_eq!(
        layout.next_after(who, Some(last), i64::MAX as u64 + 1),
        Err(Error::TimestampOverflow)
    );
}
