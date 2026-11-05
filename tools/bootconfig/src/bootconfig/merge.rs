//! Merging the bootconfig drop-ins packages contribute.

use crate::bootconfig::syntax::parse_entries;
use crate::error::Result;
use std::collections::BTreeMap;

/// Keep the last assignment of each key in the drop-ins, as written, sorted by key.
pub fn merge_parameters(text: &str) -> Result<String> {
    let mut merged: BTreeMap<String, Option<String>> = BTreeMap::new();

    for entry in parse_entries(text)? {
        let written = (!entry.is_flag()).then(|| entry.get_setting());
        // The kernel reads `-` and `_` in a parameter name alike.
        let name = entry.name.replace('-', "_");
        merged.insert(name, written);
    }

    Ok(merged
        .into_iter()
        .map(|(name, value)| match value {
            Some(value) => format!("{name} = {value}\n"),
            None => format!("{name}\n"),
        })
        .collect())
}
