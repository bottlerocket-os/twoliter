//! The results module owns the reporting of SSM validation results.

use crate::aws::ami::RegionAccount;
use crate::aws::validate_ssm::Result;
use aws_sdk_ssm::config::Region;
use serde::{Deserialize, Serialize};
use serde_plain::{derive_display_from_serialize, derive_fromstr_from_deserialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display};
use tabled::{Table, Tabled};

/// Represent the possible status of an SSM validation
#[derive(Debug, Eq, Hash, PartialEq, Serialize, Deserialize, Clone)]
pub enum SsmValidationResultStatus {
    /// The expected value was equal to the actual value
    Correct,

    /// The expected value was different from the actual value
    Incorrect,

    /// The parameter was expected but not included in the actual parameters
    Missing,

    /// The parameter was present in the actual parameters but not expected
    Unexpected,

    /// The region containing the parameter is not reachable
    Unreachable,
}

derive_display_from_serialize!(SsmValidationResultStatus);
derive_fromstr_from_deserialize!(SsmValidationResultStatus);

/// Represents a single SSM validation result
#[derive(Debug, Eq, Hash, PartialEq, Serialize)]
pub struct SsmValidationResult {
    /// The name of the parameter
    pub(crate) name: String,

    /// The expected value of the parameter
    pub(crate) expected_value: Option<String>,

    /// The actual retrieved value of the parameter
    pub(crate) actual_value: Option<String>,

    /// The region the parameter resides in
    #[serde(serialize_with = "serialize_region")]
    pub(crate) region: Region,

    /// The account the parameter resides in
    pub(crate) account_id: String,

    /// The validation status of the parameter
    pub(crate) status: SsmValidationResultStatus,
}

fn serialize_region<S>(region: &Region, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(region.to_string().as_str())
}

impl SsmValidationResult {
    pub(crate) fn new(
        name: String,
        expected_value: Option<String>,
        actual_value: Result<Option<String>>,
        region: Region,
        account_id: String,
    ) -> SsmValidationResult {
        // Determine the validation status based on equality, presence, and absence of expected and
        // actual parameter values
        let status = match (&expected_value, &actual_value) {
            (Some(expected_value), Ok(Some(actual_value))) if actual_value.eq(expected_value) => {
                SsmValidationResultStatus::Correct
            }
            (Some(_), Ok(Some(_))) => SsmValidationResultStatus::Incorrect,
            (_, Ok(None)) => SsmValidationResultStatus::Missing,
            (None, Ok(_)) => SsmValidationResultStatus::Unexpected,
            (_, Err(_)) => SsmValidationResultStatus::Unreachable,
        };
        SsmValidationResult {
            name,
            expected_value,
            actual_value: actual_value.unwrap_or_default(),
            region,
            account_id,
            status,
        }
    }
}

#[derive(Tabled, Serialize)]
struct SsmValidationRegionSummary {
    correct: u64,
    incorrect: u64,
    missing: u64,
    unexpected: u64,
    unreachable: u64,
}

impl From<&HashSet<SsmValidationResult>> for SsmValidationRegionSummary {
    fn from(results: &HashSet<SsmValidationResult>) -> Self {
        let mut region_validation = SsmValidationRegionSummary {
            correct: 0,
            incorrect: 0,
            missing: 0,
            unexpected: 0,
            unreachable: 0,
        };
        for validation_result in results {
            match validation_result.status {
                SsmValidationResultStatus::Correct => region_validation.correct += 1,
                SsmValidationResultStatus::Incorrect => region_validation.incorrect += 1,
                SsmValidationResultStatus::Missing => region_validation.missing += 1,
                SsmValidationResultStatus::Unexpected => region_validation.unexpected += 1,
                SsmValidationResultStatus::Unreachable => region_validation.unreachable += 1,
            }
        }
        region_validation
    }
}

/// Represents all SSM validation results
#[derive(Debug)]
pub struct SsmValidationResults {
    pub(crate) results: HashMap<RegionAccount, HashSet<SsmValidationResult>>,
}

impl Default for SsmValidationResults {
    fn default() -> Self {
        Self::new(HashMap::new())
    }
}

impl Display for SsmValidationResults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Create a summary for each (region, account), counting the number of parameters per
        // status
        let validations: HashMap<RegionAccount, SsmValidationRegionSummary> =
            self.get_results_summary();

        // Represent the HashMap of summaries as a `Table`
        let table = Table::new(
            validations
                .iter()
                .map(|(key, results)| (format!("{} ({})", key.region, key.account_id), results))
                .collect::<Vec<(String, &SsmValidationRegionSummary)>>(),
        )
        .to_string();
        write!(f, "{table}")
    }
}

impl SsmValidationResults {
    pub fn new(results: HashMap<RegionAccount, HashSet<SsmValidationResult>>) -> Self {
        SsmValidationResults { results }
    }

    /// Returns a HashSet containing all validation results whose status is present in
    /// `requested_status`
    pub fn get_results_for_status(
        &self,
        requested_status: &[SsmValidationResultStatus],
    ) -> HashSet<&SsmValidationResult> {
        let mut results = HashSet::new();
        for region_results in self.results.values() {
            results.extend(
                region_results
                    .iter()
                    .filter(|result| requested_status.contains(&result.status))
                    .collect::<HashSet<&SsmValidationResult>>(),
            )
        }
        results
    }

    /// Returns a `HashSet` containing all validation results
    pub(crate) fn get_all_results(&self) -> HashSet<&SsmValidationResult> {
        let mut results = HashSet::new();
        for region_results in self.results.values() {
            results.extend(region_results)
        }
        results
    }

    fn get_results_summary(&self) -> HashMap<RegionAccount, SsmValidationRegionSummary> {
        self.results
            .iter()
            .map(|(key, region_result)| {
                (key.clone(), SsmValidationRegionSummary::from(region_result))
            })
            .collect()
    }

    pub(crate) fn get_json_summary(&self) -> serde_json::Value {
        serde_json::json!(self
            .get_results_summary()
            .into_iter()
            .map(|(key, results)| (format!("{} ({})", key.region, key.account_id), results))
            .collect::<HashMap<String, SsmValidationRegionSummary>>())
    }
}

#[cfg(test)]
mod test {
    use std::collections::{HashMap, HashSet};

    use crate::aws::ami::RegionAccount;
    use crate::aws::validate_ssm::results::{
        SsmValidationResult, SsmValidationResultStatus, SsmValidationResults,
    };
    use aws_sdk_ssm::config::Region;

    // These tests assert that the `get_results_for_status` function returns the correct values.

    // Tests empty SsmValidationResults
    #[test]
    fn get_results_for_status_empty() {
        let results = SsmValidationResults::new(HashMap::from([
            (
                RegionAccount {
                    region: Region::new("us-west-2"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([]),
            ),
            (
                RegionAccount {
                    region: Region::new("us-east-1"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([]),
            ),
        ]));
        let results_filtered = results.get_results_for_status(&[
            SsmValidationResultStatus::Correct,
            SsmValidationResultStatus::Incorrect,
            SsmValidationResultStatus::Missing,
            SsmValidationResultStatus::Unexpected,
        ]);

        assert_eq!(results_filtered, HashSet::new());
    }

    // Tests the `Correct` status
    #[test]
    fn get_results_for_status_correct() {
        let results = SsmValidationResults::new(HashMap::from([
            (
                RegionAccount {
                    region: Region::new("us-west-2"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test3-parameter-name".to_string(),
                        Some("test3-parameter-value".to_string()),
                        Ok(None),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
            (
                RegionAccount {
                    region: Region::new("us-east-1"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test3-parameter-name".to_string(),
                        Some("test3-parameter-value".to_string()),
                        Ok(None),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
        ]));
        let results_filtered =
            results.get_results_for_status(&[SsmValidationResultStatus::Correct]);

        assert_eq!(
            results_filtered,
            HashSet::from([
                &SsmValidationResult::new(
                    "test1-parameter-name".to_string(),
                    Some("test1-parameter-value".to_string()),
                    Ok(Some("test1-parameter-value".to_string())),
                    Region::new("us-west-2"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test1-parameter-name".to_string(),
                    Some("test1-parameter-value".to_string()),
                    Ok(Some("test1-parameter-value".to_string())),
                    Region::new("us-east-1"),
                    "1234567890".to_string(),
                )
            ])
        );
    }

    // Tests a filter containing the `Correct` and `Incorrect` statuses
    #[test]
    fn get_results_for_status_correct_incorrect() {
        let results = SsmValidationResults::new(HashMap::from([
            (
                RegionAccount {
                    region: Region::new("us-west-2"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test3-parameter-name".to_string(),
                        Some("test3-parameter-value".to_string()),
                        Ok(None),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
            (
                RegionAccount {
                    region: Region::new("us-east-1"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test3-parameter-name".to_string(),
                        Some("test3-parameter-value".to_string()),
                        Ok(None),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
        ]));
        let results_filtered = results.get_results_for_status(&[
            SsmValidationResultStatus::Correct,
            SsmValidationResultStatus::Incorrect,
        ]);

        assert_eq!(
            results_filtered,
            HashSet::from([
                &SsmValidationResult::new(
                    "test1-parameter-name".to_string(),
                    Some("test1-parameter-value".to_string()),
                    Ok(Some("test1-parameter-value".to_string())),
                    Region::new("us-west-2"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test1-parameter-name".to_string(),
                    Some("test1-parameter-value".to_string()),
                    Ok(Some("test1-parameter-value".to_string())),
                    Region::new("us-east-1"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test2-parameter-name".to_string(),
                    Some("test2-parameter-value".to_string()),
                    Ok(Some("test2-parameter-value-wrong".to_string())),
                    Region::new("us-west-2"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test2-parameter-name".to_string(),
                    Some("test2-parameter-value".to_string()),
                    Ok(Some("test2-parameter-value-wrong".to_string())),
                    Region::new("us-east-1"),
                    "1234567890".to_string(),
                )
            ])
        );
    }

    // Tests a filter containing all statuses
    #[test]
    fn get_results_for_status_all() {
        let results = SsmValidationResults::new(HashMap::from([
            (
                RegionAccount {
                    region: Region::new("us-west-2"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test3-parameter-name".to_string(),
                        Some("test3-parameter-value".to_string()),
                        Ok(None),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
            (
                RegionAccount {
                    region: Region::new("us-east-1"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test3-parameter-name".to_string(),
                        Some("test3-parameter-value".to_string()),
                        Ok(None),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
            (
                RegionAccount {
                    region: Region::new("us-east-2"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([SsmValidationResult::new(
                    "test3-parameter-name".to_string(),
                    Some("test3-parameter-value".to_string()),
                    Err(crate::aws::validate_ssm::Error::UnreachableRegion {
                        region: "us-east-2".to_string(),
                    }),
                    Region::new("us-east-2"),
                    "1234567890".to_string(),
                )]),
            ),
        ]));
        let results_filtered = results.get_results_for_status(&[
            SsmValidationResultStatus::Correct,
            SsmValidationResultStatus::Incorrect,
            SsmValidationResultStatus::Missing,
            SsmValidationResultStatus::Unexpected,
            SsmValidationResultStatus::Unreachable,
        ]);

        assert_eq!(
            results_filtered,
            HashSet::from([
                &SsmValidationResult::new(
                    "test1-parameter-name".to_string(),
                    Some("test1-parameter-value".to_string()),
                    Ok(Some("test1-parameter-value".to_string())),
                    Region::new("us-west-2"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test1-parameter-name".to_string(),
                    Some("test1-parameter-value".to_string()),
                    Ok(Some("test1-parameter-value".to_string())),
                    Region::new("us-east-1"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test2-parameter-name".to_string(),
                    Some("test2-parameter-value".to_string()),
                    Ok(Some("test2-parameter-value-wrong".to_string())),
                    Region::new("us-west-2"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test2-parameter-name".to_string(),
                    Some("test2-parameter-value".to_string()),
                    Ok(Some("test2-parameter-value-wrong".to_string())),
                    Region::new("us-east-1"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test3-parameter-name".to_string(),
                    Some("test3-parameter-value".to_string()),
                    Ok(None),
                    Region::new("us-west-2"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test4-parameter-name".to_string(),
                    None,
                    Ok(Some("test4-parameter-value".to_string())),
                    Region::new("us-west-2"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test3-parameter-name".to_string(),
                    Some("test3-parameter-value".to_string()),
                    Ok(None),
                    Region::new("us-east-1"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test4-parameter-name".to_string(),
                    None,
                    Ok(Some("test4-parameter-value".to_string())),
                    Region::new("us-east-1"),
                    "1234567890".to_string(),
                ),
                &SsmValidationResult::new(
                    "test3-parameter-name".to_string(),
                    Some("test3-parameter-value".to_string()),
                    Err(crate::aws::validate_ssm::Error::UnreachableRegion {
                        region: "us-east-2".to_string()
                    }),
                    Region::new("us-east-2"),
                    "1234567890".to_string(),
                ),
            ])
        );
    }

    // Tests the `Missing` filter when none of the SsmValidationResults have this status
    #[test]
    fn get_results_for_status_missing_none() {
        let results = SsmValidationResults::new(HashMap::from([
            (
                RegionAccount {
                    region: Region::new("us-west-2"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-west-2"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
            (
                RegionAccount {
                    region: Region::new("us-east-1"),
                    account_id: "1234567890".to_string(),
                },
                HashSet::from([
                    SsmValidationResult::new(
                        "test1-parameter-name".to_string(),
                        Some("test1-parameter-value".to_string()),
                        Ok(Some("test1-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test2-parameter-name".to_string(),
                        Some("test2-parameter-value".to_string()),
                        Ok(Some("test2-parameter-value-wrong".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                    SsmValidationResult::new(
                        "test4-parameter-name".to_string(),
                        None,
                        Ok(Some("test4-parameter-value".to_string())),
                        Region::new("us-east-1"),
                        "1234567890".to_string(),
                    ),
                ]),
            ),
        ]));
        let results_filtered =
            results.get_results_for_status(&[SsmValidationResultStatus::Missing]);

        assert_eq!(results_filtered, HashSet::new());
    }
}
