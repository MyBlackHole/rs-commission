//! Pure business arithmetic: no database, network, current clock, or rounding floats.
use crate::{
    error::{Error, Result},
    money::{Money, MAX_OPERATION_MINOR},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terms {
    pub rate_bps: i32,
    pub fixed_minor: Money,
    pub cap_minor: Option<Money>,
    /// Referral shares are funded FROM the total platform fee, not on top of it.
    pub direct_bps: i32,
    pub indirect_bps: i32,
    pub freeze_seconds: i64,
}

impl Terms {
    pub fn validate(&self) -> Result<()> {
        if !(0..=10_000).contains(&self.rate_bps)
            || !(0..=10_000).contains(&self.direct_bps)
            || !(0..=10_000).contains(&self.indirect_bps)
            || self.direct_bps + self.indirect_bps > 10_000
        {
            return Err(Error::invalid(
                "费率须为 0..10000 基点，两级返佣比例之和不能超过 10000",
            ));
        }
        if !(0..=MAX_OPERATION_MINOR).contains(&self.fixed_minor.0)
            || self
                .cap_minor
                .is_some_and(|m| !(0..=MAX_OPERATION_MINOR).contains(&m.0))
            || !(0..=31_536_000).contains(&self.freeze_seconds)
        {
            return Err(Error::invalid("固定费用、封顶金额或冻结时间超出范围"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Split {
    pub fee_pool_minor: Money,
    pub merchant_minor: Money,
    pub platform_minor: Money,
    pub direct_minor: Money,
    pub indirect_minor: Money,
}

pub fn positive(amount: Money) -> Result<()> {
    if amount.0 <= 0 || amount.0 > MAX_OPERATION_MINOR {
        return Err(Error::invalid(
            "单次金额必须大于 0 且不超过 100000000000000 分",
        ));
    }
    Ok(())
}

pub fn split(
    paid: Money,
    base: Money,
    terms: &Terms,
    direct: bool,
    indirect: bool,
) -> Result<Split> {
    positive(paid)?;
    if base.0 < 0 || base > paid {
        return Err(Error::invalid("计佣基数必须在 0 与实付金额之间"));
    }
    if indirect && !direct {
        return Err(Error::invalid("不存在一级推广员时不能计算二级返佣"));
    }
    terms.validate()?;
    let raw_fee =
        i128::from(base.0) * i128::from(terms.rate_bps) / 10_000 + i128::from(terms.fixed_minor.0);
    let cap = terms.cap_minor.map_or(base.0, |m| m.0.min(base.0));
    let fee = raw_fee.min(i128::from(cap)) as i64;
    let first = if direct {
        ((i128::from(fee) * i128::from(terms.direct_bps)) / 10_000) as i64
    } else {
        0
    };
    let second = if indirect {
        ((i128::from(fee) * i128::from(terms.indirect_bps)) / 10_000) as i64
    } else {
        0
    };
    Ok(Split {
        fee_pool_minor: Money(fee),
        merchant_minor: Money(paid.0 - fee),
        platform_minor: Money(fee - first - second),
        direct_minor: Money(first),
        indirect_minor: Money(second),
    })
}

/// Cumulative, sequential proportional apportionment in a FIXED original slot order.
/// Independent floor(share * refund / total) plus a residual can make that residual
/// decrease on a later refund. This nested partition is monotone for EVERY slot.
/// The final slot receives the residual. Full refund exactly reverses all shares.
pub fn cumulative_refund(shares: &[Money], refunded: Money) -> Result<Vec<Money>> {
    if shares.is_empty() || shares.iter().any(|s| s.0 < 0) {
        return Err(Error::invalid("无效的原始分配"));
    }
    let total: i128 = shares.iter().map(|s| i128::from(s.0)).sum();
    if total <= 0 || refunded.0 < 0 || i128::from(refunded.0) > total {
        return Err(Error::invalid("累计退款超出实付金额"));
    }
    let mut remaining_total = total;
    let mut remaining_refund = i128::from(refunded.0);
    let mut result = Vec::with_capacity(shares.len());
    for (index, share) in shares.iter().enumerate() {
        let part = if remaining_total == 0 {
            0
        } else if index + 1 == shares.len() {
            remaining_refund
        } else {
            remaining_refund * i128::from(share.0) / remaining_total
        };
        result.push(Money(i64::try_from(part).map_err(|_| Error::Internal)?));
        remaining_refund -= part;
        remaining_total -= i128::from(share.0);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn terms() -> Terms {
        Terms {
            rate_bps: 1000,
            fixed_minor: Money(0),
            cap_minor: None,
            direct_bps: 3000,
            indirect_bps: 1000,
            freeze_seconds: 0,
        }
    }

    #[test]
    fn fees_come_from_one_pool() {
        assert_eq!(
            split(Money(10000), Money(10000), &terms(), true, true).unwrap(),
            Split {
                fee_pool_minor: Money(1000),
                merchant_minor: Money(9000),
                platform_minor: Money(600),
                direct_minor: Money(300),
                indirect_minor: Money(100),
            }
        );
    }

    #[test]
    fn absent_referrals_stay_with_platform() {
        let result = split(Money(10000), Money(8000), &terms(), false, false).unwrap();
        assert_eq!(result.platform_minor, Money(800));
        assert_eq!(result.merchant_minor, Money(9200));
    }

    #[test]
    fn caps_and_zero_base_are_safe() {
        let mut t = terms();
        t.fixed_minor = Money(2000);
        t.cap_minor = Some(Money(50));
        assert_eq!(
            split(Money(10000), Money(10000), &t, true, true)
                .unwrap()
                .fee_pool_minor,
            Money(50)
        );
        assert_eq!(
            split(Money(100), Money(0), &t, true, true)
                .unwrap()
                .fee_pool_minor,
            Money(0)
        );
        t.cap_minor = None;
        assert_eq!(
            split(Money(100), Money(100), &t, false, false)
                .unwrap()
                .merchant_minor,
            Money(0)
        );
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        let mut t = terms();
        assert!(split(Money(0), Money(0), &t, false, false).is_err());
        assert!(split(Money(1), Money(2), &t, false, false).is_err());
        assert!(split(Money(1), Money(1), &t, false, true).is_err());
        t.direct_bps = 9000;
        t.indirect_bps = 2000;
        assert!(t.validate().is_err());
    }

    #[test]
    fn tiny_refunds_never_make_a_previous_clawback_decrease() {
        for a in 0..12 {
            for b in 0..12 {
                for c in 0..12 {
                    let shares = [Money(a), Money(b), Money(c), Money(1)];
                    let total = a + b + c + 1;
                    let mut previous = vec![Money(0); 4];
                    for r in 0..=total {
                        let next = cumulative_refund(&shares, Money(r)).unwrap();
                        assert_eq!(next.iter().map(|m| m.0).sum::<i64>(), r);
                        for i in 0..4 {
                            assert!(next[i] >= previous[i] && next[i] <= shares[i]);
                        }
                        previous = next;
                    }
                    assert_eq!(previous, shares);
                }
            }
        }
    }

    proptest! {
        #[test]
        fn arbitrary_split_is_conservative(paid in 1_i64..MAX_OPERATION_MINOR, rate in 0_i32..=10000,
            first in 0_i32..=5000, second in 0_i32..=5000) {
            let mut t = terms(); t.rate_bps = rate; t.direct_bps = first; t.indirect_bps = second;
            let s = split(Money(paid), Money(paid), &t, true, true).unwrap();
            prop_assert_eq!(s.merchant_minor.0 + s.platform_minor.0 + s.direct_minor.0 + s.indirect_minor.0, paid);
        }

        #[test]
        fn refunds_are_monotone_conservative_and_exact_at_the_end(
            values in prop::collection::vec(0_i64..1_000_000_000, 2..8), fraction in 0_i64..10000) {
            let total: i64 = values.iter().sum();
            prop_assume!(total > 0);
            let shares: Vec<_> = values.iter().copied().map(Money).collect();
            let r = ((i128::from(total) * i128::from(fraction)) / 10000) as i64;
            let a = cumulative_refund(&shares, Money(r)).unwrap();
            let b = cumulative_refund(&shares, Money((r + 1).min(total))).unwrap();
            prop_assert_eq!(a.iter().map(|m| m.0).sum::<i64>(), r);
            for i in 0..shares.len() { prop_assert!(a[i] <= b[i] && b[i] <= shares[i]); }
            prop_assert_eq!(cumulative_refund(&shares, Money(total)).unwrap(), shares);
        }
    }
}
