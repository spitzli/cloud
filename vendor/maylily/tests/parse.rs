#![cfg(feature = "parse")]

use maylily::{Error, Maylily, Options, ParsedId};

#[test]
fn parses_reference_vector_independently_of_configured_producer_identity() {
    let config = Options {
        machine_id: 7,
        generator_id: 1023,
        ..Options::default()
    };
    assert_eq!(
        config.parse("1257485893632000003").unwrap(),
        ParsedId {
            timestamp_ms: 1_546_300_800_000,
            elapsed_ms: 599_616_000_000,
            machine_id: 0,
            generator_id: 0,
            sequence: 3,
        }
    );
}

#[test]
fn custom_2017_epoch_generates_and_parses_the_original_timestamp() {
    let config = Options {
        time_base: 1_483_228_800_000,
        machine_id: 1,
        generator_id: 2,
        ..Options::default()
    };
    let mut generator = Maylily::new(config).unwrap();
    let first = generator.generate_at(1_483_228_800_001).unwrap();
    let second = generator.generate_at(1_483_228_800_001).unwrap();
    assert_eq!(first, "2359808");
    assert_eq!(second, "2359809");
    assert_eq!(
        config.parse(&second).unwrap(),
        ParsedId {
            timestamp_ms: 1_483_228_800_001,
            elapsed_ms: 1,
            machine_id: 1,
            generator_id: 2,
            sequence: 1,
        }
    );
    assert_eq!(
        config.pack_at(config.time_base - 1, 0),
        Err(Error::BeforeEpoch)
    );
}

#[test]
fn parses_every_radix_and_reconstructs_small_layouts() {
    for radix in 2..=36 {
        for machine_id in 0..4 {
            for generator_id in 0..4 {
                let config = Options {
                    radix,
                    time_base: 100,
                    machine_id,
                    generator_id,
                    machine_bits: 2,
                    generator_bits: 2,
                    sequence_bits: 2,
                };
                for now in 100..104 {
                    for sequence in 0..4 {
                        let id = config.pack_at(now, sequence).unwrap().to_str_radix(radix);
                        let parts = config.parse(&id).unwrap();
                        assert_eq!(
                            parts,
                            ParsedId {
                                timestamp_ms: now,
                                elapsed_ms: now - 100,
                                machine_id,
                                generator_id,
                                sequence
                            }
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn parses_256_bit_values_and_zero_width_fields() {
    let config = Options {
        radix: 16,
        time_base: 0,
        machine_id: 0,
        generator_id: 0,
        machine_bits: 64,
        generator_bits: 64,
        sequence_bits: 64,
    };
    assert_eq!(
        config.parse(&"f".repeat(64)).unwrap(),
        ParsedId {
            timestamp_ms: u64::MAX,
            elapsed_ms: u64::MAX,
            machine_id: u64::MAX,
            generator_id: u64::MAX,
            sequence: u64::MAX,
        }
    );
    let zero = Options {
        time_base: u64::MAX,
        machine_bits: 0,
        generator_bits: 0,
        sequence_bits: 0,
        ..Options::default()
    };
    let parts = zero.parse("0").unwrap();
    assert_eq!(parts.timestamp_ms, u64::MAX);
    assert_eq!(
        (parts.machine_id, parts.generator_id, parts.sequence),
        (0, 0, 0)
    );
}

#[test]
fn rejects_malformed_and_noncanonical_input() {
    let config = Options {
        radix: 16,
        ..Options::default()
    };
    for id in [
        "", "00", "01", "+1", "-1", " 1", "1 ", "1_0", "0xff", "FF", "g", "1.0", "١",
    ] {
        assert_eq!(config.parse(id), Err(Error::InvalidId), "{id}");
    }
    assert_eq!(config.parse(&"f".repeat(257)), Err(Error::InvalidId));
    assert_eq!(
        Options { radix: 2, ..config }.parse("2"),
        Err(Error::InvalidId)
    );
}

#[test]
fn rejects_timestamp_overflow_and_invalid_layouts_without_panics() {
    let config = Options {
        time_base: 1,
        machine_bits: 0,
        generator_bits: 0,
        sequence_bits: 0,
        ..Options::default()
    };
    assert_eq!(
        config.parse("18446744073709551615"),
        Err(Error::TimestampOverflow)
    );
    assert_eq!(
        config.parse("18446744073709551616"),
        Err(Error::TimestampOverflow)
    );
    let parsed = config.parse("18446744073709551614").unwrap();
    assert_eq!(parsed.timestamp_ms, u64::MAX);
    for bits in [65, 255] {
        for invalid in [
            Options {
                machine_bits: bits,
                ..config
            },
            Options {
                generator_bits: bits,
                ..config
            },
            Options {
                sequence_bits: bits,
                ..config
            },
        ] {
            assert_eq!(invalid.parse("0"), Err(Error::InvalidBits));
        }
    }
    for radix in [0, 1, 37, u32::MAX] {
        assert_eq!(
            Options { radix, ..config }.parse("0"),
            Err(Error::InvalidRadix)
        );
    }
}

#[test]
fn incorrect_epoch_is_not_detectable_from_the_id_alone() {
    let config = Options {
        time_base: 100,
        machine_id: 0,
        generator_id: 0,
        ..Options::default()
    };
    let id = config.pack_at(101, 0).unwrap().to_string();
    assert_eq!(config.parse(&id).unwrap().timestamp_ms, 101);
    assert_eq!(
        Options {
            time_base: 200,
            ..config
        }
        .parse(&id)
        .unwrap()
        .timestamp_ms,
        201
    );
}

#[cfg(feature = "serde")]
#[test]
fn parsed_fields_support_serde_when_both_features_are_enabled() {
    let parts = Options::default().parse("1257485893632000003").unwrap();
    let json = serde_json::to_string(&parts).unwrap();
    assert_eq!(serde_json::from_str::<ParsedId>(&json).unwrap(), parts);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json).unwrap()["sequence"],
        3
    );
}
