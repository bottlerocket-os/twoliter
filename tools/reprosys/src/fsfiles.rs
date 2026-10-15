//! File-level comparison report types.

use serde::Serialize;

/// How a region or a file of the published image compares to the rebuilt one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Match,
    Mismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: String,
    pub status: Status,
    pub detail: String,
}
