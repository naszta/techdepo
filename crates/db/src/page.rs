//! Keyset pagination: an opaque cursor holding the sort key of the last row of a page.

use techdepo_api_types::page_limit;

/// The sort key of the last row on a page: an integer (an instant in epoch ms, a number) and a
/// tie-breaking ID. Encoded as `<int>.<id>`, which is opaque to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// Primary sort key.
    pub key: i64,
    /// Tie breaker (the row's ID).
    pub id: String,
}

impl Cursor {
    /// A cursor after `(key, id)`.
    pub fn new(key: i64, id: impl Into<String>) -> Self {
        Self { key, id: id.into() }
    }

    /// The wire form.
    pub fn encode(&self) -> String {
        format!("{}.{}", self.key, self.id)
    }

    /// Parses the wire form; `None` for anything malformed (treated as "first page").
    pub fn decode(s: &str) -> Option<Self> {
        let (key, id) = s.split_once('.')?;
        Some(Self {
            key: key.parse().ok()?,
            id: id.to_owned(),
        })
    }
}

/// A page request: where to start and how many rows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageRequest {
    /// Start after this row.
    pub after: Option<Cursor>,
    /// Page size (already clamped).
    pub limit: u32,
}

impl PageRequest {
    /// From the `cursor` and `limit` query parameters.
    pub fn from_query(cursor: Option<&str>, limit: Option<u32>) -> Self {
        Self {
            after: cursor.and_then(Cursor::decode),
            limit: page_limit(limit),
        }
    }

    /// The first page with the default size.
    pub fn first() -> Self {
        Self::from_query(None, None)
    }

    /// `LIMIT` to query: one more than the page, to know whether there is a next page.
    pub(crate) fn fetch(&self) -> i64 {
        i64::from(self.limit) + 1
    }

    /// Trims the extra row and returns the next cursor, built from the last kept row.
    pub(crate) fn finish<T>(
        &self,
        mut rows: Vec<T>,
        key: impl Fn(&T) -> Cursor,
    ) -> (Vec<T>, Option<String>) {
        let limit = usize::try_from(self.limit).unwrap_or(usize::MAX);
        if rows.len() > limit {
            rows.truncate(limit);
            let next = rows.last().map(|r| key(r).encode());
            (rows, next)
        } else {
            (rows, None)
        }
    }

    /// `(key, id)` of the cursor, or values that match every row for the first page of a
    /// descending list.
    pub(crate) fn desc_bounds(&self) -> (i64, String) {
        match &self.after {
            Some(c) => (c.key, c.id.clone()),
            None => (i64::MAX, String::new()),
        }
    }

    /// The same for an ascending list.
    pub(crate) fn asc_bounds(&self) -> (i64, String) {
        match &self.after {
            Some(c) => (c.key, c.id.clone()),
            None => (i64::MIN, String::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips() {
        let c = Cursor::new(1_791_892_800_000, "ord_ab.c");
        assert_eq!(Cursor::decode(&c.encode()), Some(c));
        assert_eq!(Cursor::decode("garbage"), None);
    }

    #[test]
    fn finish_trims_and_links() {
        let page = PageRequest::from_query(None, Some(2));
        assert_eq!(page.fetch(), 3);
        let (rows, next) = page.finish(vec![3, 2, 1], |r| Cursor::new(*r, "x"));
        assert_eq!(rows, vec![3, 2]);
        assert_eq!(next.as_deref(), Some("2.x"));
        let (rows, next) = page.finish(vec![1], |r| Cursor::new(*r, "x"));
        assert_eq!((rows, next), (vec![1], None));
    }
}
