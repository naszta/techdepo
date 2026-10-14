//! Per-org features (`orgs.features_json`): parts of the app only some orgs get. Unlike
//! [`OrgSettings`](crate::OrgSettings), admins cannot change them; the operator switches them
//! on per org, outside the app.

labeled_enum! {
    /// A feature an org can have.
    pub enum OrgFeature {
        /// The Muhaha admin page.
        Muhaha => ("muhaha", "Muhaha"),
    }
}

/// The known features among stored names. Unknown names (a feature removed or not yet shipped,
/// a typo in hand-written SQL) are skipped rather than failing the org's every request.
pub fn known_features(names: &[String]) -> Vec<OrgFeature> {
    let mut features: Vec<OrgFeature> = names.iter().filter_map(|n| n.parse().ok()).collect();
    features.sort_unstable();
    features.dedup();
    features
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_unknown_names() {
        let names = ["nope", "muhaha", "muhaha"].map(str::to_owned);
        assert_eq!(known_features(&names), [OrgFeature::Muhaha]);
        assert!(known_features(&[]).is_empty());
    }
}
