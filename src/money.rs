//! Money is always an integer number of CNY cents. JSON uses strings, never f64.
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Signed so an available balance can explicitly represent a post-payout debt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, sqlx::Type)]
#[sqlx(transparent)]
pub struct Money(pub i64);

pub const MAX_OPERATION_MINOR: i64 = 100_000_000_000_000;

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let digits = value.strip_prefix('-').unwrap_or(&value);
        if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return Err(de::Error::custom("金额必须为整数字符串，单位为分"));
        }
        value.parse::<i64>().map(Self).map_err(de::Error::custom)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monetary_json_never_uses_float_or_js_unsafe_number() {
        let amount = Money(9_007_199_254_740_993);
        assert_eq!(serde_json::to_string(&amount).unwrap(), "\"9007199254740993\"");
        assert_eq!(serde_json::from_str::<Money>("\"-17\"").unwrap(), Money(-17));
        for invalid in ["12", "12.3", "\"1.2\"", "\"+1\"", "\" 1\"", "null"] {
            assert!(serde_json::from_str::<Money>(invalid).is_err());
        }
    }
}
