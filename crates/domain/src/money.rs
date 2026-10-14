//! Money in integer euro cents, and VAT.
//!
//! All prices are VAT-inclusive (gross). Invoices break a gross amount down into net + VAT
//! using the org's single VAT rate.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An amount of euros, stored as integer cents. May be negative (refunds, credit notes).
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Money(i64);

impl Money {
    /// €0.00.
    pub const ZERO: Self = Self(0);

    /// An amount in cents.
    pub const fn from_cents(cents: i64) -> Self {
        Self(cents)
    }

    /// A whole number of euros.
    pub const fn from_euros(euros: i64) -> Self {
        Self(euros * 100)
    }

    /// The amount in cents.
    pub const fn cents(self) -> i64 {
        self.0
    }

    /// `true` for amounts above zero.
    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    /// `true` for amounts below zero.
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// `true` for €0.00.
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Absolute value.
    pub const fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// Addition that returns `None` on overflow.
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Subtraction that returns `None` on overflow.
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Multiplication by a quantity that returns `None` on overflow.
    pub const fn checked_mul(self, qty: i64) -> Option<Self> {
        match self.0.checked_mul(qty) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Splits this gross (VAT-inclusive) amount into net and VAT.
    ///
    /// The net amount is rounded half away from zero; VAT is the remainder, so
    /// `net + vat == gross` always holds.
    pub fn vat_breakdown(self, rate: VatRate) -> VatBreakdown {
        let gross = i128::from(self.0);
        let divisor = i128::from(VatRate::BASIS + rate.0);
        let numerator = gross * i128::from(VatRate::BASIS);
        let half = divisor / 2;
        let net = if numerator >= 0 {
            (numerator + half) / divisor
        } else {
            (numerator - half) / divisor
        };
        // |net| <= |gross|, so the conversion cannot fail.
        let net = i64::try_from(net).unwrap_or(self.0);
        VatBreakdown {
            gross: self,
            net: Self(net),
            vat: Self(self.0 - net),
        }
    }

    /// Net part of this gross amount.
    pub fn net(self, rate: VatRate) -> Self {
        self.vat_breakdown(rate).net
    }

    /// VAT part of this gross amount.
    pub fn vat(self, rate: VatRate) -> Self {
        self.vat_breakdown(rate).vat
    }
}

impl fmt::Display for Money {
    /// `€1,435.90`, `-€5.00`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let euros = (abs / 100).to_string();
        let cents = abs % 100;

        let mut grouped = String::with_capacity(euros.len() + euros.len() / 3);
        for (i, ch) in euros.chars().enumerate() {
            if i > 0 && (euros.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        write!(f, "{sign}€{grouped}.{cents:02}")
    }
}

impl Add for Money {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for Money {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for Money {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Neg for Money {
    type Output = Self;

    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl Mul<i64> for Money {
    type Output = Self;

    fn mul(self, qty: i64) -> Self {
        Self(self.0 * qty)
    }
}

impl Mul<u32> for Money {
    type Output = Self;

    fn mul(self, qty: u32) -> Self {
        Self(self.0 * i64::from(qty))
    }
}

impl Sum for Money {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl<'a> Sum<&'a Money> for Money {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.copied().sum()
    }
}

/// A VAT rate in basis points (`2700` = 27 %).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct VatRate(u32);

impl VatRate {
    const BASIS: u32 = 10_000;

    /// The default rate: 27 % (Hungary).
    pub const DEFAULT: Self = Self(2700);

    /// A rate in basis points.
    pub const fn from_basis_points(bp: u32) -> Self {
        Self(bp)
    }

    /// The rate in basis points.
    pub const fn basis_points(self) -> u32 {
        self.0
    }
}

impl Default for VatRate {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for VatRate {
    /// `27%`, `5.5%`, `7.25%`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / 100;
        match self.0 % 100 {
            0 => write!(f, "{whole}%"),
            frac if frac % 10 == 0 => write!(f, "{whole}.{}%", frac / 10),
            frac => write!(f, "{whole}.{frac:02}%"),
        }
    }
}

/// A gross amount split into net and VAT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VatBreakdown {
    /// VAT-inclusive amount.
    pub gross: Money,
    /// Amount without VAT.
    pub net: Money,
    /// VAT portion.
    pub vat: Money,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_with_grouping() {
        assert_eq!(Money::from_cents(143_590).to_string(), "€1,435.90");
        assert_eq!(Money::from_cents(5).to_string(), "€0.05");
        assert_eq!(Money::ZERO.to_string(), "€0.00");
        assert_eq!(Money::from_cents(-500).to_string(), "-€5.00");
        assert_eq!(Money::from_euros(1_234_567).to_string(), "€1,234,567.00");
        assert_eq!(Money::from_euros(100).to_string(), "€100.00");
        assert_eq!(
            Money::from_cents(i64::MIN).to_string(),
            "-€92,233,720,368,547,758.08"
        );
    }

    #[test]
    fn arithmetic() {
        let a = Money::from_cents(1999);
        assert_eq!(a * 3_i64, Money::from_cents(5997));
        assert_eq!(a + a - a, a);
        assert_eq!(-a, Money::from_cents(-1999));
        assert_eq!([a, a].iter().sum::<Money>(), Money::from_cents(3998));
        assert_eq!(Money::from_cents(i64::MAX).checked_add(a), None);
    }

    #[test]
    fn vat_breakdown_default_rate() {
        // 127.00 gross at 27 % is exactly 100.00 net + 27.00 VAT.
        let b = Money::from_euros(127).vat_breakdown(VatRate::DEFAULT);
        assert_eq!(b.net, Money::from_euros(100));
        assert_eq!(b.vat, Money::from_euros(27));

        // 1,435.90 / 1.27 = 1,130.6299… → 1,130.63 net, 305.27 VAT.
        let b = Money::from_cents(143_590).vat_breakdown(VatRate::DEFAULT);
        assert_eq!(b.net, Money::from_cents(113_063));
        assert_eq!(b.vat, Money::from_cents(30_527));
        assert_eq!(b.net + b.vat, b.gross);
    }

    #[test]
    fn vat_breakdown_rounds_symmetrically() {
        let rate = VatRate::DEFAULT;
        for cents in [-143_590, -1, 0, 1, 2, 3, 99, 12_345] {
            let pos = Money::from_cents(cents).vat_breakdown(rate);
            let neg = Money::from_cents(-cents).vat_breakdown(rate);
            assert_eq!(pos.net, -neg.net, "cents {cents}");
            assert_eq!(pos.net + pos.vat, pos.gross);
        }
        let zero_rate = Money::from_cents(999).vat_breakdown(VatRate::from_basis_points(0));
        assert_eq!(zero_rate.vat, Money::ZERO);
    }

    #[test]
    fn vat_rate_display() {
        assert_eq!(VatRate::DEFAULT.to_string(), "27%");
        assert_eq!(VatRate::from_basis_points(550).to_string(), "5.5%");
        assert_eq!(VatRate::from_basis_points(725).to_string(), "7.25%");
    }
}
