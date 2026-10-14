//! A minimal left-aligned text table for terminal summaries.

use std::fmt;

/// Rows of cells printed with each column padded to its widest cell.
pub struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// A table with these column headings.
    pub fn new<I, S>(header: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            header: header.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    /// Appends a row (missing trailing cells print empty).
    pub fn row<I, S>(&mut self, cells: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows.push(cells.into_iter().map(Into::into).collect());
    }
}

impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let columns = self.header.len();
        let mut widths: Vec<usize> = self.header.iter().map(|h| h.chars().count()).collect();
        for row in &self.rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }
        let line = |f: &mut fmt::Formatter<'_>, cells: &[String]| -> fmt::Result {
            let mut out = String::new();
            for (i, width) in widths.iter().enumerate().take(columns) {
                let cell = cells.get(i).map(String::as_str).unwrap_or("");
                if i + 1 == columns {
                    out.push_str(cell);
                } else {
                    let pad = width - cell.chars().count();
                    out.push_str(cell);
                    out.extend(std::iter::repeat_n(' ', pad + 2));
                }
            }
            writeln!(f, "  {}", out.trim_end())
        };
        line(f, &self.header)?;
        let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        line(f, &rule)?;
        for row in &self.rows {
            line(f, row)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_columns() {
        let mut table = Table::new(["a", "bb"]);
        table.row(["long", "x"]);
        assert_eq!(table.to_string(), "  a     bb\n  ----  --\n  long  x\n");
    }
}
