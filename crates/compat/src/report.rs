//! The result of a compatibility check.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_domain::{ProductId, labeled_enum};

use crate::power::PowerDraw;
use crate::rule::{RuleId, Severity};

/// One problem (or remark) about some parts of a build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Issue {
    /// Always `rule.severity()`; stored so consumers need not look it up.
    pub severity: Severity,
    /// The rule that produced it.
    pub rule: RuleId,
    /// The parts involved, most relevant first.
    pub parts: Vec<ProductId>,
    /// Customer-facing explanation of what is wrong and why, with names and numbers.
    pub message: String,
}

impl Issue {
    /// An issue from `rule` about `parts`.
    pub fn new<'p>(
        rule: RuleId,
        parts: impl IntoIterator<Item = &'p ProductId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: rule.severity(),
            rule,
            parts: parts.into_iter().cloned().collect(),
            message: message.into(),
        }
    }

    /// Whether `id` is one of the parts involved.
    pub fn involves(&self, id: &ProductId) -> bool {
        self.parts.contains(id)
    }
}

labeled_enum! {
    /// Overall result of a check.
    pub enum Verdict {
        /// No errors and no warnings (notes are fine).
        Compatible => ("compatible", "Compatible"),
        /// Works, but at least one warning.
        Warnings => ("warnings", "Compatible with warnings"),
        /// At least one error.
        Incompatible => ("incompatible", "Incompatible"),
    }
}

impl Verdict {
    /// The verdict for a worst severity (`None` when there are no issues).
    pub const fn from_worst(worst: Option<Severity>) -> Self {
        match worst {
            Some(Severity::Error) => Self::Incompatible,
            Some(Severity::Warning) => Self::Warnings,
            Some(Severity::Note) | None => Self::Compatible,
        }
    }
}

/// What [`check_build`](crate::check_build) found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Report {
    /// Overall verdict.
    pub verdict: Verdict,
    /// Issues, most severe first, then in rule order.
    pub issues: Vec<Issue>,
    /// Estimated draw in watts (components plus 10 %).
    pub estimated_watts: u32,
    /// The PSU's rating, if the build has one.
    pub psu_watts: Option<u32>,
    /// Spare PSU capacity in percent of its rating (negative when overloaded).
    pub headroom_pct: Option<i32>,
    /// Per-part draws behind `estimated_watts`.
    pub power_draws: Vec<PowerDraw>,
    /// Rules that had the parts they need and were evaluated, in rule order. A rule missing
    /// here was not checked, so its absence from `issues` says nothing.
    pub checked_rules: Vec<RuleId>,
}

impl Report {
    /// No errors (warnings and notes allowed).
    pub fn is_compatible(&self) -> bool {
        self.verdict != Verdict::Incompatible
    }

    /// The worst severity, if any issue.
    pub fn worst(&self) -> Option<Severity> {
        self.issues.iter().map(|i| i.severity).max()
    }

    /// Issues of one severity.
    pub fn with_severity(&self, severity: Severity) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.severity == severity)
    }

    /// Issues that involve `id`.
    pub fn issues_for<'r>(&'r self, id: &'r ProductId) -> impl Iterator<Item = &'r Issue> {
        self.issues.iter().filter(move |i| i.involves(id))
    }

    /// The worst severity among issues involving `id` (the builder's per-slot status icon).
    pub fn worst_for(&self, id: &ProductId) -> Option<Severity> {
        self.issues_for(id).map(|i| i.severity).max()
    }

    /// Whether `rule` was evaluated.
    pub fn checked(&self, rule: RuleId) -> bool {
        self.checked_rules.contains(&rule)
    }
}
