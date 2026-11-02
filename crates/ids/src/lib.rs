//! Webdock's signed-ID and durable-checkpoint policy over Maylily packing.
//!
//! Packing alone does not provide uniqueness: every producer needs exclusive
//! ownership of its identity and a durable high-water mark. See the crate README
//! for the transaction, restart, fencing, and migration contracts.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

impl From<maylily::Error> for Error {
    fn from(error: maylily::Error) -> Self {
        match error {
            maylily::Error::IdentityOverflow => Self::IdentityOverflow,
            maylily::Error::BeforeEpoch => Self::BeforeEpoch,
            maylily::Error::SequenceOverflow => Self::SequenceOverflow,
            maylily::Error::ClockRollback { now_ms, last_ms } => {
                Self::ClockRollback { now_ms, last_ms }
            }
            maylily::Error::InvalidId => Self::InvalidId,
            maylily::Error::TimestampOverflow => Self::TimestampOverflow,
            maylily::Error::InvalidRadix
            | maylily::Error::InvalidBits
            | maylily::Error::SystemClock => Self::InvalidLayout,
        }
    }
}

pub const PLATFORM_EPOCH_MS: u64 = 1_767_225_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("layout must have timestamp bits, at most 63 total bits, and a representable end time")]
    InvalidLayout,
    #[error("identity does not fit the layout")]
    IdentityOverflow,
    #[error("platform identity slots 0 and 1 belong to legacy generators")]
    ReservedIdentity,
    #[error("timestamp precedes the epoch")]
    BeforeEpoch,
    #[error("timestamp exceeds the layout lifetime")]
    TimestampOverflow,
    #[error("sequence exhausted; wait for the next millisecond")]
    SequenceOverflow,
    #[error("clock moved backwards from {last_ms} to {now_ms}")]
    ClockRollback { now_ms: u64, last_ms: u64 },
    #[error("checkpoint belongs to another identity")]
    CheckpointIdentityMismatch,
    #[error("ID exceeds the layout")]
    IdOutsideLayout,
    #[error("ID must be canonical nonnegative decimal within signed 64-bit range")]
    InvalidId,
}

/// Nonnegative signed-64-bit value. JSON always uses a decimal string.
/// Generic layouts can produce zero; platform identities always produce positive IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(i64);

impl Id {
    pub const fn as_i64(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for Id {
    type Error = Error;
    fn try_from(value: i64) -> Result<Self, Error> {
        if value < 0 {
            Err(Error::InvalidId)
        } else {
            Ok(Self(value))
        }
    }
}

impl FromStr for Id {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Error> {
        if value.is_empty()
            || value.len() > 19
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(Error::InvalidId);
        }
        value.parse::<i64>().map(Self).map_err(|_| Error::InvalidId)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Immutable validated bit widths in timestamp, machine, generator, sequence order.
/// A layout must stay fixed for an entire ID namespace, including after restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    epoch_ms: u64,
    bits: [u8; 4],
}

impl Layout {
    pub const PLATFORM: Self = Self {
        epoch_ms: PLATFORM_EPOCH_MS,
        bits: [41, 6, 4, 12],
    };

    pub fn new(
        epoch_ms: u64,
        timestamp: u8,
        machine: u8,
        generator: u8,
        sequence: u8,
    ) -> Result<Self, Error> {
        let bits = [timestamp, machine, generator, sequence];
        if timestamp == 0 || bits.iter().map(|&bits| u16::from(bits)).sum::<u16>() > 63 {
            return Err(Error::InvalidLayout);
        }
        if epoch_ms.checked_add(mask(timestamp)).is_none() {
            return Err(Error::InvalidLayout);
        }
        Ok(Self { epoch_ms, bits })
    }

    pub const fn epoch_ms(self) -> u64 {
        self.epoch_ms
    }
    pub const fn bits(self) -> [u8; 4] {
        self.bits
    }
    pub fn max_timestamp_ms(self) -> u64 {
        self.epoch_ms + mask(self.bits[0])
    }

    fn options(self, identity: Identity) -> maylily::Options {
        maylily::Options {
            radix: 10,
            time_base: self.epoch_ms,
            machine_id: identity.machine,
            machine_bits: self.bits[1],
            generator_id: identity.generator,
            generator_bits: self.bits[2],
            sequence_bits: self.bits[3],
        }
    }

    fn elapsed(self, timestamp_ms: u64) -> Result<u64, Error> {
        let elapsed = timestamp_ms
            .checked_sub(self.epoch_ms)
            .ok_or(Error::BeforeEpoch)?;
        if elapsed > mask(self.bits[0]) {
            return Err(Error::TimestampOverflow);
        }
        Ok(elapsed)
    }

    /// Calculate a candidate, without reserving it. Persist it before releasing it.
    /// Also rejects reserved identities even when obtained by decoding a legacy ID.
    pub fn pack(self, timestamp_ms: u64, identity: Identity, sequence: u64) -> Result<Id, Error> {
        Identity::new(self, identity.machine, identity.generator)?;
        self.elapsed(timestamp_ms)?;
        let value = self.options(identity).pack_at(timestamp_ms, sequence)?;
        i64::try_from(value)
            .map(Id)
            .map_err(|_| Error::TimestampOverflow)
    }

    /// Decode existing IDs, including legacy slots 0 and 1. This does not authorize
    /// generation with the decoded identity or prove which layout created an ID.
    pub fn decode(self, id: Id) -> Result<Parts, Error> {
        if id.0 as u64 > mask(self.bits.iter().sum()) {
            return Err(Error::IdOutsideLayout);
        }
        let decoded = self
            .options(Identity {
                machine: 0,
                generator: 0,
            })
            .parse(&id.to_string())?;
        Ok(Parts {
            timestamp_ms: decoded.timestamp_ms,
            identity: Identity {
                machine: decoded.machine_id,
                generator: decoded.generator_id,
            },
            sequence: decoded.sequence,
        })
    }

    /// Pure transaction primitive: compute the next candidate after a checkpoint.
    ///
    /// Lock the identity's durable checkpoint before reading `high_water`, keep
    /// that lock through commit, and return this candidate only AFTER commit.
    /// `None` is valid only for a provably never-used identity, not missing/lost state.
    /// Rejected clock/sequence/overflow attempts do not consume an ID.
    pub fn next_after(
        self,
        identity: Identity,
        high_water: Option<Id>,
        now_ms: u64,
    ) -> Result<Id, Error> {
        Identity::new(self, identity.machine, identity.generator)?;
        self.elapsed(now_ms)?;
        let mut sequence = 0;
        if let Some(checkpoint) = high_water {
            let previous = self.decode(checkpoint)?;
            if previous.identity != identity {
                return Err(Error::CheckpointIdentityMismatch);
            }
            if now_ms < previous.timestamp_ms {
                return Err(Error::ClockRollback {
                    now_ms,
                    last_ms: previous.timestamp_ms,
                });
            }
            if now_ms == previous.timestamp_ms {
                sequence = previous
                    .sequence
                    .checked_add(1)
                    .ok_or(Error::SequenceOverflow)?;
            }
        }
        self.pack(now_ms, identity, sequence)
    }
}

fn mask(bits: u8) -> u64 {
    (1_u64 << bits) - 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    machine: u64,
    generator: u64,
}

impl Identity {
    /// Explicitly assigned identity, never inferred from PID or randomness.
    pub fn new(layout: Layout, machine: u64, generator: u64) -> Result<Self, Error> {
        if machine > mask(layout.bits[1]) || generator > mask(layout.bits[2]) {
            return Err(Error::IdentityOverflow);
        }
        if layout == Layout::PLATFORM && machine == 0 && generator < 2 {
            return Err(Error::ReservedIdentity);
        }
        Ok(Self { machine, generator })
    }
    pub fn platform(machine: u64, generator: u64) -> Result<Self, Error> {
        Self::new(Layout::PLATFORM, machine, generator)
    }
    pub const fn machine(self) -> u64 {
        self.machine
    }
    pub const fn generator(self) -> u64 {
        self.generator
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    pub timestamp_ms: u64,
    pub identity: Identity,
    pub sequence: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError<E> {
    #[error(transparent)]
    Generation(#[from] Error),
    #[error("high-water persistence failed: {0}")]
    Persistence(E),
}

/// Stateful convenience for an identity held exclusively by the caller.
/// Not Clone: cloning a checkpoint is not an allocation of a new identity.
/// This type provides neither a process lock nor a distributed lease.
#[derive(Debug)]
pub struct Generator {
    layout: Layout,
    identity: Identity,
    high_water: Option<Id>,
}

impl Generator {
    /// Load a durable checkpoint while holding exclusive ownership of the identity.
    /// The checkpoint storage key MUST also bind the immutable layout/epoch.
    pub fn resume(
        layout: Layout,
        identity: Identity,
        high_water: Option<Id>,
    ) -> Result<Self, Error> {
        Identity::new(layout, identity.machine, identity.generator)?;
        if let Some(id) = high_water
            && layout.decode(id)?.identity != identity
        {
            return Err(Error::CheckpointIdentityMismatch);
        }
        Ok(Self {
            layout,
            identity,
            high_water,
        })
    }

    /// Persist before return. The callback must durably commit before returning Ok.
    /// It must not release the candidate to consumers itself. On failure (including
    /// uncertain commit acknowledgement), the in-memory candidate remains consumed.
    /// Restart always requires reloading the authoritative durable high-water mark.
    pub fn next_at<E>(
        &mut self,
        now_ms: u64,
        persist: impl FnOnce(Id) -> Result<(), E>,
    ) -> Result<Id, GenerateError<E>> {
        let candidate = self
            .layout
            .next_after(self.identity, self.high_water, now_ms)?;
        self.high_water = Some(candidate);
        persist(candidate).map_err(GenerateError::Persistence)?;
        Ok(candidate)
    }
}
