use techdepo_domain::labeled_enum;

labeled_enum! {
    /// Product category. The wire name is also the YAML file stem (`data/catalog/<wire>.yaml`).
    pub enum Category {
        Cpu => ("cpu", "CPU"),
        Motherboard => ("motherboard", "Motherboard"),
        Memory => ("memory", "Memory"),
        Storage => ("storage", "Storage"),
        Gpu => ("gpu", "Graphics card"),
        Case => ("case", "Case"),
        Psu => ("psu", "Power supply"),
        Cooler => ("cooler", "CPU cooler"),
    }
}

impl Category {
    /// Number of categories.
    pub const COUNT: usize = Self::ALL.len();

    /// Position in [`Category::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }
}

labeled_enum! {
    /// Price tier within a category.
    pub enum Tier {
        Budget => ("budget", "Budget"),
        Mid => ("mid", "Mid-range"),
        High => ("high", "High-end"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_matches_all() {
        for (i, c) in Category::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
        }
    }
}
