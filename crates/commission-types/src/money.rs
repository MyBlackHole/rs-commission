//! Signed integer CNY cents. The wire representation is always a JSON string.
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type))]
#[cfg_attr(feature = "postgres", sqlx(transparent))]
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
impl Money {
    /// Decimal display without floating point, including i64::MIN and debts.
    pub fn yuan(self) -> String {
        let cents = self.0.unsigned_abs();
        format!(
            "{}{}.{:02}",
            if self.0 < 0 { "-" } else { "" },
            cents / 100,
            cents % 100
        )
    }
    /// Strict form input. Do not round user-entered fractions silently.
    pub fn from_yuan(value: &str) -> Result<Self, &'static str> {
        let (negative, value) = match value.strip_prefix('-') {
            Some(value) => (true, value),
            None => (false, value),
        };
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || fraction.len() > 2
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || value.ends_with('.')
        {
            return Err("金额格式无效，最多两位小数");
        }
        let whole = whole.parse::<i128>().map_err(|_| "金额超出范围")?;
        let fraction = match fraction.len() {
            0 => 0,
            1 => fraction.parse::<i128>().map_err(|_| "金额格式无效")? * 10,
            _ => fraction.parse::<i128>().map_err(|_| "金额格式无效")?,
        };
        let cents = whole
            .checked_mul(100)
            .and_then(|v| v.checked_add(fraction))
            .ok_or("金额超出范围")?;
        i64::try_from(if negative { -cents } else { cents })
            .map(Self)
            .map_err(|_| "金额超出范围")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_uses_strings() {
        let amount = Money(9_007_199_254_740_993);
        assert_eq!(
            serde_json::to_string(&amount).unwrap(),
            "\"9007199254740993\""
        );
        assert_eq!(
            serde_json::from_str::<Money>("\"-17\"").unwrap(),
            Money(-17)
        );
        for invalid in ["12", "12.3", "\"1.2\"", "\"+1\"", "\" 1\"", "null"] {
            assert!(serde_json::from_str::<Money>(invalid).is_err());
        }
    }
    #[test]
    fn decimal_roundtrip_including_boundaries() {
        for value in [
            i64::MIN,
            -101,
            -1,
            0,
            1,
            12345,
            9_007_199_254_740_993,
            i64::MAX,
        ] {
            assert_eq!(Money::from_yuan(&Money(value).yuan()), Ok(Money(value)));
        }
        assert_eq!(Money::from_yuan("12.3"), Ok(Money(1230)));
    }
    #[test]
    fn rejects_rounding_overflow_and_ambiguous_input() {
        for value in [
            "",
            "1.234",
            "1.",
            ".1",
            "+1",
            " 1",
            "1e2",
            "NaN",
            "--1",
            "92233720368547758.08",
        ] {
            assert!(Money::from_yuan(value).is_err(), "{value}");
        }
    }
}
