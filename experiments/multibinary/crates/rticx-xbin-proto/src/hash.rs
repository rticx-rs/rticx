//! Deterministic 64-bit hashing shared by the driver, the compilation pass
//! and the generated code.
//!
//! All hashes in the multi-binary pipeline (`source_hash`, `layout_hash`,
//! `topology_hash`) are FNV-1a 64-bit values over a canonical text, so that
//! identical inputs always produce identical bytes. In JSON they are rendered
//! as lowercase `0x`-prefixed 16-digit hex strings (`"0x0123456789abcdef"`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::HashParseError;

/// An FNV-1a 64-bit hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Hash64(u64);

impl Hash64 {
    /// The all-zero hash.
    pub const ZERO: Self = Self(0);

    /// Wraps a raw hash value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw hash value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Computes the FNV-1a 64-bit hash of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = OFFSET_BASIS;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        Self(hash)
    }

    /// Renders the hash as lowercase `0x`-prefixed hex with 16 digits.
    pub fn to_hex(self) -> String {
        format!("0x{:016x}", self.0)
    }

    /// Parses the canonical hex form (with or without `0x`/`0X`, 1..=16
    /// hexadecimal digits).
    pub fn from_hex(text: &str) -> Result<Self, HashParseError> {
        let digits = text
            .strip_prefix("0x")
            .or_else(|| text.strip_prefix("0X"))
            .unwrap_or(text);
        if digits.is_empty() || digits.len() > 16 {
            return Err(HashParseError::invalid(text));
        }
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(HashParseError::invalid(text));
        }
        let value = u64::from_str_radix(digits, 16).map_err(|_| HashParseError::invalid(text))?;
        Ok(Self(value))
    }
}

impl fmt::Display for Hash64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for Hash64 {
    type Err = HashParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_hex(text)
    }
}

impl From<Hash64> for String {
    fn from(hash: Hash64) -> Self {
        hash.to_hex()
    }
}

impl TryFrom<String> for Hash64 {
    type Error = HashParseError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::from_hex(&text)
    }
}
