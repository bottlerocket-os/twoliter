//! Provides a utility for merging toml tables

/// Represents a type that can be "merged" with another version of itself.
///
/// This is primarily intended to be used with Map-like types, where keys can be recursively
/// upserted into `self` from `other`.
pub(crate) trait Merge {
    fn merge(&mut self, other: &Self);
}

impl Merge for toml::Value {
    fn merge(&mut self, other: &Self) {
        match (self, other) {
            (toml::Value::Table(a), toml::Value::Table(b)) => a.merge(b),
            (a, b) => {
                *a = b.clone();
            }
        }
    }
}

impl Merge for toml::Table {
    fn merge(&mut self, other: &Self) {
        for (k, v) in other {
            if let Some(a_val) = self.get_mut(k) {
                a_val.merge(v);
            } else {
                self.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::Merge;
    use toml::toml;

    #[test]
    fn test_flat_merge() {
        let mut a = toml! {
            a = 1
            b = 2
            c = 3
        };

        let b = toml! {
            b = 4
            c = 5
            d = 6
        };

        let expected = toml! {
            a = 1
            b = 4
            c = 5
            d = 6
        };

        a.merge(&b);

        assert_eq!(a, expected);
    }

    #[test]
    fn test_nested_merge() {
        let mut a = toml! {
            a = 1
            b = { c = 2, d = 3 }
        };

        let b = toml! {
            b = { d = { g = 6 }, e = 4 }
            f = 5
        };

        let expected = toml! {
            a = 1
            b = { c = 2, d = { g = 6 }, e = 4 }
            f = 5
        };

        a.merge(&b);

        assert_eq!(a, expected);
    }

    #[test]
    fn test_empty_merge() {
        let mut a = toml! {
            a = 1
            b = 2
        };
        let expected = a.clone();

        a.merge(&toml::map::Map::new());
        assert_eq!(a, expected);
    }
}
