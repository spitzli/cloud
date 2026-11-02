//! Maylily-style IDs: elapsed milliseconds, machine, generator, then sequence.
//!
//! Output uses arbitrary-precision integers and radix 2–36. Each instance owns
//! its options and sequence. Callers must assign unique machine/generator pairs
//! and coordinate restarts; PID defaults alone do not guarantee uniqueness.

#![doc = include_str!("../README.md")]

use num_bigint::BigUint;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "parse")]
mod parse;
#[cfg(feature = "parse")]
pub use parse::ParsedId;

pub const DEFAULT_TIME_BASE: u64 = 946_684_800_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("radix must be between 2 and 36")]
    InvalidRadix,
    #[error("field widths must be between 0 and 64 bits")]
    InvalidBits,
    #[error("machine or generator ID does not fit its field")]
    IdentityOverflow,
    #[error("timestamp precedes the time base")]
    BeforeEpoch,
    #[error("sequence exhausted; wait for the next millisecond")]
    SequenceOverflow,
    #[error("clock moved backwards from {last_ms} to {now_ms}")]
    ClockRollback { now_ms: u64, last_ms: u64 },
    #[error("system clock is outside the supported Unix millisecond range")]
    SystemClock,
    #[cfg(feature = "parse")]
    #[error("ID must use canonical lowercase digits for the configured radix (at most 256 digits)")]
    InvalidId,
    #[cfg(feature = "parse")]
    #[error("decoded timestamp exceeds the Unix millisecond range")]
    TimestampOverflow,
}

/// Immutable configuration for a generator. Never change a namespace's layout
/// while IDs from it remain in use. Individual fields support up to 64 bits;
/// the packed result can exceed 64 or 128 bits without losing precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Options {
    pub radix: u32,
    pub time_base: u64,
    pub machine_id: u64,
    pub machine_bits: u8,
    pub generator_id: u64,
    pub generator_bits: u8,
    pub sequence_bits: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            radix: 10,
            time_base: DEFAULT_TIME_BASE,
            machine_id: 0,
            machine_bits: 3,
            generator_id: u64::from(std::process::id() % 1024),
            generator_bits: 10,
            sequence_bits: 8,
        }
    }
}

impl Options {
    fn validate_layout(self) -> Result<(), Error> {
        if !(2..=36).contains(&self.radix) {
            return Err(Error::InvalidRadix);
        }
        if self.machine_bits > 64 || self.generator_bits > 64 || self.sequence_bits > 64 {
            return Err(Error::InvalidBits);
        }
        Ok(())
    }

    fn validate(self) -> Result<(), Error> {
        self.validate_layout()?;
        if !fits(self.machine_id, self.machine_bits)
            || !fits(self.generator_id, self.generator_bits)
        {
            return Err(Error::IdentityOverflow);
        }
        Ok(())
    }

    /// Pack an explicitly supplied sequence. This computes a candidate only;
    /// it neither reserves the ID nor updates any generator state.
    pub fn pack_at(self, now_ms: u64, sequence: u64) -> Result<BigUint, Error> {
        self.validate()?;
        let elapsed = now_ms
            .checked_sub(self.time_base)
            .ok_or(Error::BeforeEpoch)?;
        if !fits(sequence, self.sequence_bits) {
            return Err(Error::SequenceOverflow);
        }
        Ok(
            (((((BigUint::from(elapsed) << self.machine_bits) + self.machine_id)
                << self.generator_bits)
                + self.generator_id)
                << self.sequence_bits)
                + sequence,
        )
    }
}

fn fits(value: u64, bits: u8) -> bool {
    bits == 64 || value < (1_u64 << bits)
}

fn system_time_ms() -> Result<u64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::SystemClock)?
        .as_millis()
        .try_into()
        .map_err(|_| Error::SystemClock)
}

/// One in-memory generator. Not Clone: a copy would reuse its identity and state.
/// Neither construction nor the default PID assigns an exclusive identity.
#[derive(Debug)]
pub struct Maylily {
    options: Options,
    previous: Option<(u64, u64)>,
}

impl Maylily {
    pub fn new(options: Options) -> Result<Self, Error> {
        options.validate()?;
        Ok(Self {
            options,
            previous: None,
        })
    }

    /// Generate at a supplied Unix timestamp. Exhaustion is returned to the caller
    /// for retry; rollback and rejected attempts leave the previous state intact.
    pub fn generate_at(&mut self, now_ms: u64) -> Result<String, Error> {
        let sequence = match self.previous {
            Some((last_ms, _)) if now_ms < last_ms => {
                return Err(Error::ClockRollback { now_ms, last_ms });
            }
            Some((last_ms, sequence)) if now_ms == last_ms => {
                sequence.checked_add(1).ok_or(Error::SequenceOverflow)?
            }
            _ => 0,
        };
        let id = self.options.pack_at(now_ms, sequence)?;
        self.previous = Some((now_ms, sequence));
        Ok(id.to_str_radix(self.options.radix))
    }

    /// Generate using the system clock. Like the reference implementation, retry
    /// after sequence exhaustion. This synchronous method blocks while waiting;
    /// async callers can use `generate_async` with the `async` feature, or call
    /// `generate_at` and wait with their runtime's timer.
    pub fn generate(&mut self) -> Result<String, Error> {
        self.generate_with(system_time_ms, || {
            std::thread::sleep(Duration::from_millis(1))
        })
    }

    /// Generate with the system clock, awaiting a Tokio timer on exhaustion.
    /// Shares the sequence with synchronous calls. Dropping a pending call while
    /// waiting does not consume an ID. Requires the `async` feature.
    ///
    /// # Panics
    /// May panic on exhaustion without a Tokio runtime with timers enabled.
    #[cfg(feature = "async")]
    pub async fn generate_async(&mut self) -> Result<String, Error> {
        self.generate_async_with(system_time_ms).await
    }

    #[cfg(feature = "async")]
    async fn generate_async_with(
        &mut self,
        mut now_ms: impl FnMut() -> Result<u64, Error>,
    ) -> Result<String, Error> {
        loop {
            match self.generate_at(now_ms()?) {
                Err(Error::SequenceOverflow) => tokio::time::sleep(Duration::from_millis(1)).await,
                result => return result,
            }
        }
    }

    fn generate_with(
        &mut self,
        mut now_ms: impl FnMut() -> Result<u64, Error>,
        mut wait: impl FnMut(),
    ) -> Result<String, Error> {
        loop {
            match self.generate_at(now_ms()?) {
                Err(Error::SequenceOverflow) => wait(),
                result => return result,
            }
        }
    }
}

#[cfg(test)]
mod tests;
