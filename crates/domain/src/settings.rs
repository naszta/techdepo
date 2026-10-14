//! Per-org settings (stored as JSON in `orgs.settings`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{Date, Duration};

use crate::calendar::{WorkCalendar, iso_dates};
use crate::fault::{FaultError, FaultToggles};
use crate::money::VatRate;

/// How AI-authored replies are released (support.md §6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplyPolicy {
    /// Send as soon as the guardrails pass.
    AlwaysSend,
    /// Every AI-authored email becomes a review draft.
    AlwaysReview,
    /// Send if reply confidence ≥ the threshold, otherwise create a review draft.
    #[default]
    SendAboveConfidence,
}

/// What to do with an AI-authored reply that passed the guardrails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplyDisposition {
    /// Send it now.
    Send,
    /// Hold it as a review draft for a person.
    Review,
}

/// Settings are out of range.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SettingsError {
    /// A probability-like value is outside `0.0..=1.0`.
    #[error("{field} must be between 0 and 1, got {value}")]
    NotAProbability {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: f64,
    },
    /// The VAT rate is above 100 %.
    #[error("VAT rate must be at most 10000 basis points, got {0}")]
    VatRate(u32),
    /// A duration is zero.
    #[error("{0} must be greater than zero")]
    ZeroDuration(&'static str),
    /// A duration is longer than [`MAX_SETTINGS_DURATION_SECS`].
    #[error("{0} must be at most 30 days")]
    DurationTooLong(&'static str),
    /// The holiday list is longer than [`MAX_HOLIDAYS`].
    #[error("at most {MAX_HOLIDAYS} holidays, got {0}")]
    TooManyHolidays(usize),
    /// A default fault is out of range (the same rules as the per-order toggles).
    #[error("{0}")]
    Fault(#[from] FaultError),
}

/// Longest reminder, auto-close or SLA delay (30 days). Keeps deadlines computed from them
/// well inside the representable date range.
pub const MAX_SETTINGS_DURATION_SECS: u64 = 30 * 24 * 60 * 60;

/// Longest holiday list (a year of days).
pub const MAX_HOLIDAYS: usize = 366;

/// Per-org configuration. Missing fields deserialize to their defaults, so stored JSON keeps
/// working when fields are added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct OrgSettings {
    /// Escalate if the top intent's probability is below this.
    pub escalation_threshold: f64,
    /// Escalate if the top intent leads the runner-up by less than this.
    pub escalation_margin: f64,
    /// How AI-authored replies are released.
    pub reply_policy: ReplyPolicy,
    /// Threshold for [`ReplyPolicy::SendAboveConfidence`].
    pub reply_confidence_threshold: f64,
    /// VAT rate in basis points (2700 = 27 %).
    pub vat_rate_bp: u32,
    /// Fault toggles pre-armed on new orders.
    pub faults: FaultToggles,
    /// Remind the customer after this much silence.
    pub reminder_after_secs: u64,
    /// Close the ticket after this much further silence following the reminder.
    pub auto_close_after_secs: u64,
    /// Nudge an admin when a human task has waited this long.
    pub human_sla_secs: u64,
    /// Days that are not workdays besides weekends (`YYYY-MM-DD`; empty by default). They move
    /// carrier ETAs, `+1 workday` and the "workdays since shipment" of the shipping policy.
    #[serde(with = "iso_dates")]
    #[schemars(with = "Vec<String>")]
    pub holidays: Vec<Date>,
}

impl Default for OrgSettings {
    fn default() -> Self {
        Self {
            escalation_threshold: 0.70,
            escalation_margin: 0.15,
            reply_policy: ReplyPolicy::default(),
            reply_confidence_threshold: 0.80,
            vat_rate_bp: VatRate::DEFAULT.basis_points(),
            faults: FaultToggles::default(),
            reminder_after_secs: 10 * 60,
            auto_close_after_secs: 10 * 60,
            human_sla_secs: 15 * 60,
            holidays: Vec::new(),
        }
    }
}

impl OrgSettings {
    /// The org's VAT rate.
    pub const fn vat_rate(&self) -> VatRate {
        VatRate::from_basis_points(self.vat_rate_bp)
    }

    /// Whether a classification is too uncertain to route automatically.
    ///
    /// `top` and `runner_up` are the two highest intent probabilities.
    pub fn should_escalate(&self, top: f64, runner_up: f64) -> bool {
        top < self.escalation_threshold || top - runner_up < self.escalation_margin
    }

    /// Whether an AI-authored reply with this confidence is sent or held for review.
    pub fn reply_disposition(&self, confidence: f64) -> ReplyDisposition {
        let send = match self.reply_policy {
            ReplyPolicy::AlwaysSend => true,
            ReplyPolicy::AlwaysReview => false,
            ReplyPolicy::SendAboveConfidence => confidence >= self.reply_confidence_threshold,
        };
        if send {
            ReplyDisposition::Send
        } else {
            ReplyDisposition::Review
        }
    }

    /// Reminder delay.
    pub fn reminder_after(&self) -> Duration {
        secs(self.reminder_after_secs)
    }

    /// Auto-close delay after the reminder.
    pub fn auto_close_after(&self) -> Duration {
        secs(self.auto_close_after_secs)
    }

    /// Human task SLA.
    pub fn human_sla(&self) -> Duration {
        secs(self.human_sla_secs)
    }

    /// The org's workday calendar (Monday–Friday minus [`holidays`](Self::holidays)).
    pub fn calendar(&self) -> WorkCalendar {
        WorkCalendar::with_holidays(self.holidays.iter().copied())
    }

    /// Checks value ranges (for the admin settings form).
    pub fn validate(&self) -> Result<(), SettingsError> {
        for (field, value) in [
            ("escalation_threshold", self.escalation_threshold),
            ("escalation_margin", self.escalation_margin),
            (
                "reply_confidence_threshold",
                self.reply_confidence_threshold,
            ),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(SettingsError::NotAProbability { field, value });
            }
        }
        if self.vat_rate_bp > 10_000 {
            return Err(SettingsError::VatRate(self.vat_rate_bp));
        }
        for (field, value) in [
            ("reminder_after_secs", self.reminder_after_secs),
            ("auto_close_after_secs", self.auto_close_after_secs),
            ("human_sla_secs", self.human_sla_secs),
        ] {
            if value == 0 {
                return Err(SettingsError::ZeroDuration(field));
            }
            if value > MAX_SETTINGS_DURATION_SECS {
                return Err(SettingsError::DurationTooLong(field));
            }
        }
        if self.holidays.len() > MAX_HOLIDAYS {
            return Err(SettingsError::TooManyHolidays(self.holidays.len()));
        }
        self.faults.validate()?;
        Ok(())
    }
}

/// `value` seconds, capped at [`MAX_SETTINGS_DURATION_SECS`] (stored settings predate the check).
fn secs(value: u64) -> Duration {
    Duration::seconds(i64::try_from(value.min(MAX_SETTINGS_DURATION_SECS)).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_tolerant() {
        let settings = OrgSettings::default();
        assert_eq!(settings.validate(), Ok(()));
        assert_eq!(settings.vat_rate(), VatRate::DEFAULT);
        let parsed: OrgSettings = serde_json::from_str(r#"{"escalation_threshold":0.5}"#).unwrap();
        assert_eq!(parsed.escalation_threshold, 0.5);
        assert_eq!(parsed.reply_confidence_threshold, 0.80);
    }

    #[test]
    fn holidays_shape_the_calendar() {
        use time::macros::date;

        let parsed: OrgSettings =
            serde_json::from_str(r#"{"holidays":["2026-10-23","2026-12-25"]}"#).unwrap();
        assert_eq!(
            parsed.holidays,
            vec![date!(2026 - 10 - 23), date!(2026 - 12 - 25)]
        );
        let json = serde_json::to_value(&parsed).unwrap();
        assert_eq!(json["holidays"][0], "2026-10-23");
        let cal = parsed.calendar();
        assert!(!cal.is_workday(date!(2026 - 10 - 23)));
        assert!(cal.is_workday(date!(2026 - 10 - 22)));
        assert_eq!(OrgSettings::default().calendar(), WorkCalendar::new());
        let s = OrgSettings {
            holidays: vec![date!(2026 - 10 - 23); MAX_HOLIDAYS + 1],
            ..OrgSettings::default()
        };
        assert_eq!(
            s.validate(),
            Err(SettingsError::TooManyHolidays(MAX_HOLIDAYS + 1))
        );
    }

    #[test]
    fn escalation() {
        let s = OrgSettings::default();
        assert!(!s.should_escalate(0.90, 0.05));
        assert!(s.should_escalate(0.65, 0.10), "below threshold");
        assert!(s.should_escalate(0.75, 0.65), "small margin");
    }

    #[test]
    fn reply_policy() {
        let mut s = OrgSettings::default();
        assert_eq!(s.reply_disposition(0.80), ReplyDisposition::Send);
        assert_eq!(s.reply_disposition(0.79), ReplyDisposition::Review);
        s.reply_policy = ReplyPolicy::AlwaysReview;
        assert_eq!(s.reply_disposition(1.0), ReplyDisposition::Review);
        s.reply_policy = ReplyPolicy::AlwaysSend;
        assert_eq!(s.reply_disposition(0.0), ReplyDisposition::Send);
    }

    #[test]
    fn validation() {
        let s = OrgSettings {
            escalation_threshold: 1.5,
            ..OrgSettings::default()
        };
        assert!(matches!(
            s.validate(),
            Err(SettingsError::NotAProbability { .. })
        ));
        let s = OrgSettings {
            human_sla_secs: 0,
            ..OrgSettings::default()
        };
        assert_eq!(
            s.validate(),
            Err(SettingsError::ZeroDuration("human_sla_secs"))
        );
        let s = OrgSettings {
            human_sla_secs: 1_000_000_000_000,
            ..OrgSettings::default()
        };
        assert_eq!(
            s.validate(),
            Err(SettingsError::DurationTooLong("human_sla_secs"))
        );
        assert_eq!(s.human_sla(), Duration::days(30));
        for minutes in [0, 100_000] {
            let s = OrgSettings {
                faults: FaultToggles {
                    email: crate::fault::EmailFault::Outage { minutes },
                    ..FaultToggles::default()
                },
                ..OrgSettings::default()
            };
            assert_eq!(
                s.validate(),
                Err(SettingsError::Fault(FaultError::EmailOutageMinutes(
                    minutes
                )))
            );
        }
    }
}
