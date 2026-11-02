use maylily::{Error, Maylily, Options};

const JANUARY_2019: u64 = 1_546_300_800_000;

fn options() -> Options {
    Options {
        generator_id: 0,
        ..Options::default()
    }
}

#[test]
fn matches_reference_sequence_and_millisecond_reset() {
    let mut generator = Maylily::new(options()).unwrap();
    for expected in [
        "1257485893632000000",
        "1257485893632000001",
        "1257485893632000002",
    ] {
        assert_eq!(generator.generate_at(JANUARY_2019).unwrap(), expected);
    }
    assert_eq!(
        generator.generate_at(JANUARY_2019 + 1).unwrap(),
        "1257485893634097152"
    );
    assert_eq!(
        generator.generate_at(JANUARY_2019),
        Err(Error::ClockRollback {
            now_ms: JANUARY_2019,
            last_ms: JANUARY_2019 + 1,
        })
    );
    assert_eq!(
        generator.generate_at(JANUARY_2019 + 1).unwrap(),
        "1257485893634097153"
    );
}

#[test]
fn matches_reference_radices() {
    let now = 1_554_076_800_000;
    for (radix, sequence, expected) in [
        (10, 0, "1273793347584000000"),
        (
            2,
            1,
            "1000110101101011011000100110100000000000000000000000000000001",
        ),
        (4, 2, "1012231123010310000000000000002"),
        (8, 3, "106553304640000000003"),
        (16, 4, "11ad6c4d00000004"),
        (32, 5, "13bbc9k000005"),
        (36, 6, "9oea21h9w5c6"),
    ] {
        let config = Options { radix, ..options() };
        assert_eq!(
            config.pack_at(now, sequence).unwrap().to_str_radix(radix),
            expected
        );
        let mut generator = Maylily::new(config).unwrap();
        for _ in 0..sequence {
            generator.generate_at(now).unwrap();
        }
        assert_eq!(generator.generate_at(now).unwrap(), expected);
    }
}

#[test]
fn exhaustion_retries_without_wrap_or_state_loss() {
    let mut generator = Maylily::new(Options {
        sequence_bits: 2,
        ..options()
    })
    .unwrap();
    let now = 1_548_979_200_000;
    for expected in [
        "19735982899200000",
        "19735982899200001",
        "19735982899200002",
        "19735982899200003",
    ] {
        assert_eq!(generator.generate_at(now).unwrap(), expected);
    }
    for _ in 0..2 {
        assert_eq!(generator.generate_at(now), Err(Error::SequenceOverflow));
    }
    assert_eq!(generator.generate_at(now + 1).unwrap(), "19735982899232768");
}

#[test]
fn output_is_not_limited_to_machine_integer_precision() {
    let config = Options {
        time_base: 0,
        machine_id: u64::MAX,
        machine_bits: 64,
        generator_id: u64::MAX,
        generator_bits: 64,
        sequence_bits: 64,
        ..options()
    };
    assert_eq!(
        config.pack_at(u64::MAX, u64::MAX).unwrap().to_str_radix(16),
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
    );
    let mut generator = Maylily::new(config).unwrap();
    assert_eq!(
        generator.generate_at(0).unwrap(),
        "6277101735386680763835789423207666416083908700390324961280"
    );
    assert_eq!(
        generator.generate_at(0).unwrap(),
        "6277101735386680763835789423207666416083908700390324961281"
    );
}

#[test]
fn validates_configuration_and_timestamp_boundaries() {
    for radix in [0, 1, 37, u32::MAX] {
        assert!(matches!(
            Maylily::new(Options { radix, ..options() }),
            Err(Error::InvalidRadix)
        ));
    }
    for (machine_bits, generator_bits, sequence_bits) in [(65, 1, 1), (1, 65, 1), (1, 1, 65)] {
        assert!(matches!(
            Maylily::new(Options {
                machine_bits,
                generator_bits,
                sequence_bits,
                ..options()
            }),
            Err(Error::InvalidBits)
        ));
    }
    for config in [
        Options {
            machine_id: 8,
            ..options()
        },
        Options {
            generator_id: 1024,
            ..options()
        },
    ] {
        assert!(matches!(Maylily::new(config), Err(Error::IdentityOverflow)));
        assert_eq!(
            config.pack_at(JANUARY_2019, 0),
            Err(Error::IdentityOverflow)
        );
    }
    let config = options();
    assert_eq!(
        config.pack_at(config.time_base - 1, 0),
        Err(Error::BeforeEpoch)
    );
    assert_eq!(
        config.pack_at(config.time_base, 256),
        Err(Error::SequenceOverflow)
    );
    let mut generator = Maylily::new(config).unwrap();
    assert_eq!(
        generator.generate_at(config.time_base - 1),
        Err(Error::BeforeEpoch)
    );
    assert_eq!(generator.generate_at(config.time_base).unwrap(), "0");
}

#[test]
fn zero_width_fields_and_custom_epoch_are_supported() {
    let config = Options {
        time_base: u64::MAX,
        machine_bits: 0,
        generator_bits: 0,
        sequence_bits: 0,
        ..options()
    };
    let mut generator = Maylily::new(config).unwrap();
    assert_eq!(generator.generate_at(u64::MAX).unwrap(), "0");
    assert_eq!(
        generator.generate_at(u64::MAX),
        Err(Error::SequenceOverflow)
    );
    assert!(matches!(
        Maylily::new(Options {
            machine_id: 1,
            ..config
        }),
        Err(Error::IdentityOverflow)
    ));
}

#[test]
fn uses_the_system_clock_and_keeps_instances_independent() {
    let mut a = Maylily::new(Options {
        generator_id: 1,
        ..options()
    })
    .unwrap();
    let mut b = Maylily::new(Options {
        generator_id: 2,
        ..options()
    })
    .unwrap();
    assert_ne!(
        a.generate_at(JANUARY_2019).unwrap(),
        b.generate_at(JANUARY_2019).unwrap()
    );
    let first = a.generate().unwrap().parse::<u128>().unwrap();
    let second = a.generate().unwrap().parse::<u128>().unwrap();
    assert!(second > first);
}

#[test]
fn every_valid_radix_encodes_the_same_id_without_precision_loss() {
    for radix in 2..=36 {
        let mut generator = Maylily::new(Options { radix, ..options() }).unwrap();
        let encoded = generator.generate_at(JANUARY_2019).unwrap();
        let decoded = num_bigint::BigUint::parse_bytes(encoded.as_bytes(), radix).unwrap();
        assert_eq!(decoded.to_string(), "1257485893632000000", "radix {radix}");
    }
}

#[test]
fn distinct_field_combinations_do_not_collide() {
    let mut ids = std::collections::HashSet::new();
    for now in 100..104 {
        for machine_id in 0..4 {
            for generator_id in 0..4 {
                let config = Options {
                    time_base: 100,
                    machine_id,
                    machine_bits: 2,
                    generator_id,
                    generator_bits: 2,
                    sequence_bits: 2,
                    ..options()
                };
                let mut generator = Maylily::new(config).unwrap();
                for _ in 0..4 {
                    assert!(ids.insert(generator.generate_at(now).unwrap()));
                }
                assert_eq!(generator.generate_at(now), Err(Error::SequenceOverflow));
            }
        }
    }
    assert_eq!(ids.len(), 256);
    assert!(ids.contains("0"));
    assert!(ids.contains("255"));
}
