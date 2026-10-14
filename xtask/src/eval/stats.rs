//! Small helpers for the eval reports: ratios, calibration buckets and markdown tables.

use std::fmt::Write as _;

/// `hits / total`, `None` when there is nothing to measure.
pub fn ratio(hits: usize, total: usize) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)] // eval sizes
    (total > 0).then(|| hits as f64 / total as f64)
}

/// `0.912` → `91.2 %`, `None` → `n/a`.
pub fn pct(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |v| format!("{:.1} %", v * 100.0))
}

/// `hits/total (pct)`.
pub fn frac(hits: usize, total: usize) -> String {
    format!("{hits}/{total} ({})", pct(ratio(hits, total)))
}

/// The mean of `values`, `None` when empty.
pub fn mean(values: &[f64]) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// One probability bucket: how often the top answer was right when the model said `p`.
#[derive(Debug, Clone, Default)]
pub struct Bucket {
    /// Lower bound (inclusive).
    pub lo: f64,
    /// Upper bound (exclusive, except the last).
    pub hi: f64,
    /// Probabilities that fell in.
    pub ps: Vec<f64>,
    /// How many of them were right.
    pub correct: usize,
}

/// Buckets `(p, correct)` pairs at `edges` (ascending, from 0.0 to 1.0).
pub fn calibration(points: &[(f64, bool)], edges: &[f64]) -> Vec<Bucket> {
    let mut buckets: Vec<Bucket> = edges
        .windows(2)
        .map(|w| Bucket {
            lo: w[0],
            hi: w[1],
            ..Bucket::default()
        })
        .collect();
    let last = buckets.len().saturating_sub(1);
    for &(p, correct) in points {
        let i = buckets
            .iter()
            .position(|b| p >= b.lo && p < b.hi)
            .unwrap_or(last);
        if let Some(b) = buckets.get_mut(i) {
            b.ps.push(p);
            b.correct += usize::from(correct);
        }
    }
    buckets
}

/// Expected calibration error: the count-weighted gap between mean confidence and accuracy.
pub fn ece(buckets: &[Bucket]) -> Option<f64> {
    let total: usize = buckets.iter().map(|b| b.ps.len()).sum();
    if total == 0 {
        return None;
    }
    let gap: f64 = buckets
        .iter()
        .filter(|b| !b.ps.is_empty())
        .map(|b| {
            let acc = ratio(b.correct, b.ps.len()).unwrap_or(0.0);
            let conf = mean(&b.ps).unwrap_or(0.0);
            #[allow(clippy::cast_precision_loss)]
            let weight = b.ps.len() as f64;
            weight * (acc - conf).abs()
        })
        .sum();
    #[allow(clippy::cast_precision_loss)]
    Some(gap / total as f64)
}

/// A markdown table.
pub struct Md {
    out: String,
}

impl Md {
    /// A table with these headings; `right` columns are right-aligned.
    pub fn new(header: &[&str], right: &[usize]) -> Self {
        let mut out = String::new();
        let _ = writeln!(out, "| {} |", header.join(" | "));
        let rule: Vec<&str> = (0..header.len())
            .map(|i| if right.contains(&i) { "---:" } else { "---" })
            .collect();
        let _ = writeln!(out, "|{}|", rule.join("|"));
        Self { out }
    }

    /// Appends a row (pipes in cells are escaped).
    pub fn row<S: AsRef<str>>(&mut self, cells: &[S]) {
        let cells: Vec<String> = cells
            .iter()
            .map(|c| c.as_ref().replace('|', "\\|").replace('\n', " "))
            .collect();
        let _ = writeln!(self.out, "| {} |", cells.join(" | "));
    }

    /// The table, followed by a blank line.
    pub fn finish(mut self) -> String {
        self.out.push('\n');
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_and_error() {
        let points = [
            (0.95, true),
            (0.92, true),
            (0.55, false),
            (0.65, true),
            (1.0, true),
        ];
        let buckets = calibration(&points, &[0.0, 0.6, 0.9, 1.0]);
        assert_eq!(
            buckets.iter().map(|b| b.ps.len()).collect::<Vec<_>>(),
            [1, 1, 3]
        );
        assert_eq!(buckets[2].correct, 3);
        let e = ece(&buckets).unwrap();
        assert!(e > 0.0 && e < 0.5, "{e}");
        assert_eq!(pct(ratio(1, 4)), "25.0 %");
        assert_eq!(pct(None), "n/a");
    }
}
