use super::{Error, Options};
use num_bigint::BigUint;

/// Fields decoded using a caller-supplied epoch, radix and bit layout.
/// This is not proof of the producer's identity or a restart checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ParsedId {
    /// Absolute Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Milliseconds since the configured time base.
    pub elapsed_ms: u64,
    pub machine_id: u64,
    pub generator_id: u64,
    pub sequence: u64,
}

impl Options {
    /// Decode an ID using this configuration's radix, epoch and bit widths.
    /// Configured producer IDs are ignored; the encoded IDs are returned instead.
    ///
    /// Accepts the canonical lowercase output of `generate_at`, including zero.
    /// Layout metadata is not embedded in an ID: using another valid layout can
    /// return different values without an error. Parsing does not reserve IDs,
    /// change generator state, authenticate a producer or recover a checkpoint.
    pub fn parse(self, id: &str) -> Result<ParsedId, Error> {
        self.validate_layout()?;
        // The largest supported packed value is 256 bits (256 binary digits).
        if id.is_empty()
            || id.len() > 256
            || (id.len() > 1 && id.starts_with('0'))
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
        {
            return Err(Error::InvalidId);
        }
        let mut value = BigUint::parse_bytes(id.as_bytes(), self.radix).ok_or(Error::InvalidId)?;
        let sequence = take_field(&mut value, self.sequence_bits)?;
        let generator_id = take_field(&mut value, self.generator_bits)?;
        let machine_id = take_field(&mut value, self.machine_bits)?;
        let elapsed_ms = u64::try_from(value).map_err(|_| Error::TimestampOverflow)?;
        let timestamp_ms = self
            .time_base
            .checked_add(elapsed_ms)
            .ok_or(Error::TimestampOverflow)?;
        Ok(ParsedId {
            timestamp_ms,
            elapsed_ms,
            machine_id,
            generator_id,
            sequence,
        })
    }
}

fn take_field(value: &mut BigUint, bits: u8) -> Result<u64, Error> {
    let mask = (BigUint::from(1_u8) << bits) - 1_u8;
    let field = u64::try_from(&*value & mask).map_err(|_| Error::InvalidId)?;
    *value >>= bits;
    Ok(field)
}
