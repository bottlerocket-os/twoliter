//! Core PCR prediction types and functions.

use serde::Serialize;

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::efi::EfiVars;
use crate::gpt::PartitionLayout;
use crate::platform::Platform;

use crate::error::Result;

/// Initial PCR value (32 zero bytes).
pub const PCR_INIT_VAL: [u8; 32] = [0u8; 32];

/// TPM Platform Configuration Register index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PcrIndex {
    Pcr0 = 0,
    Pcr1 = 1,
    Pcr2 = 2,
    Pcr3 = 3,
    Pcr4 = 4,
    Pcr5 = 5,
    Pcr6 = 6,
    Pcr7 = 7,
    #[allow(dead_code)]
    Pcr8 = 8,
    Pcr9 = 9,
    Pcr10 = 10,
    Pcr11 = 11,
    Pcr12 = 12,
    Pcr13 = 13,
    Pcr14 = 14,
    Pcr15 = 15,
}

impl Serialize for PcrIndex {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}

/// All extracted boot context needed for PCR predictions.
#[derive(bon::Builder)]
pub struct PcrContext<'a> {
    pub platform: Platform,
    pub efi_vars: &'a EfiVars,
    pub partitions: &'a PartitionLayout,
    #[builder(default)]
    pub gpt_bin: &'a [u8],
    #[builder(default)]
    pub shim: &'a [u8],
    #[builder(default)]
    pub grub: &'a [u8],
    #[builder(default)]
    pub vmlinuz: &'a [u8],
    #[builder(default)]
    pub grub_cfg: &'a [u8],
    #[builder(default)]
    pub bootconfig: &'a [u8],
    #[builder(default)]
    pub boot_partuuid: &'a str,
}

/// Collection of PCR predictions for JSON output.
#[derive(Default, Serialize)]
pub struct PcrPredictions {
    /// Map of PCR index to record.
    pub pcrs: BTreeMap<PcrIndex, PcrRecord>,
}

impl PcrPredictions {
    /// Create a new empty set of PCR predictions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Try a prediction function; insert result if Some, propagate errors.
    ///
    /// This enables fluent chaining of PCR predictions.
    pub fn try_extend(
        mut self,
        f: impl FnOnce() -> Result<Option<(PcrIndex, PcrRecord)>>,
    ) -> Result<Self> {
        if let Some((index, record)) = f()? {
            self.pcrs.insert(index, record);
        }
        Ok(self)
    }
}

/// A single PCR prediction record.
#[derive(Debug, Serialize)]
pub struct PcrRecord {
    /// SHA-256 digest values for this PCR.
    pub sha256: Vec<String>,
}

impl PcrRecord {
    /// Create a new PCR record with a single SHA-256 digest.
    pub fn new(digest: [u8; 32]) -> Self {
        Self {
            sha256: vec![hex::encode(digest)],
        }
    }

    /// Create a new PCR record with multiple SHA-256 digests.
    pub fn new_multi(digests: Vec<[u8; 32]>) -> Self {
        Self {
            sha256: digests.into_iter().map(hex::encode).collect(),
        }
    }
}

/// Extend a PCR value with a digest: `SHA256(current || digest)`.
pub fn extend_pcr(current: &[u8; 32], digest: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(current);
    hasher.update(digest);
    hasher.finalize().into()
}

/// Extend a PCR with a string by first hashing the string.
pub fn extend_pcr_string(current: &[u8; 32], s: &str) -> [u8; 32] {
    let string_digest: [u8; 32] = Sha256::digest(s.as_bytes()).into();
    extend_pcr(current, &string_digest)
}

/// Extend a PCR with the EV_SEPARATOR event (4 zero bytes).
pub fn extend_pcr_separator(current: &[u8; 32]) -> [u8; 32] {
    let separator_digest: [u8; 32] = Sha256::digest([0u8; 4]).into();
    extend_pcr(current, &separator_digest)
}

#[cfg(test)]
pub mod test_support {
    use super::PcrContext;
    use crate::efi::EfiVars;
    use crate::gpt::{PartitionInfo, PartitionLayout};
    use crate::platform::Platform;

    /// SHA-256 of extending PCR init value with the EV_SEPARATOR event.
    pub const SEPARATOR_HASH: &str =
        "3d458cfe55cc03ea1f443f1562beec8df51c75e14a9fcf9a7234a13f198e7969";

    /// Create empty EFI variables for testing.
    pub fn mock_efi_vars() -> EfiVars {
        EfiVars { variables: vec![] }
    }

    /// Create a minimal partition layout for testing.
    pub fn mock_layout() -> PartitionLayout {
        PartitionLayout {
            efi_a: PartitionInfo {
                number: 1,
                start_lba: 0,
                end_lba: 1,
            },
            boot_a: PartitionInfo {
                number: 2,
                start_lba: 2,
                end_lba: 3,
            },
            boot_b: None,
            private: PartitionInfo {
                number: 3,
                start_lba: 4,
                end_lba: 5,
            },
        }
    }

    /// Create a dual-bank (A/B) partition layout for testing.
    pub fn mock_layout_dual_bank() -> PartitionLayout {
        PartitionLayout {
            efi_a: PartitionInfo {
                number: 1,
                start_lba: 0,
                end_lba: 1,
            },
            boot_a: PartitionInfo {
                number: 2,
                start_lba: 2,
                end_lba: 3,
            },
            boot_b: Some(PartitionInfo {
                number: 4,
                start_lba: 6,
                end_lba: 7,
            }),
            private: PartitionInfo {
                number: 3,
                start_lba: 4,
                end_lba: 5,
            },
        }
    }

    pub use crate::pe::tests::build_test_shim;

    /// Test context that owns EfiVars and PartitionLayout for convenient test setup.
    pub struct MockCtx {
        pub efi_vars: EfiVars,
        pub layout: PartitionLayout,
    }

    impl MockCtx {
        /// Create a new MockCtx with single-bank layout.
        pub fn new() -> Self {
            Self {
                efi_vars: mock_efi_vars(),
                layout: mock_layout(),
            }
        }

        /// Create a MockCtx with dual-bank (A/B) layout.
        pub fn dual_bank() -> Self {
            Self {
                efi_vars: mock_efi_vars(),
                layout: mock_layout_dual_bank(),
            }
        }

        /// Build a PcrContext with the given platform.
        pub fn build(&self, platform: Platform) -> PcrContext<'_> {
            PcrContext::builder()
                .platform(platform)
                .efi_vars(&self.efi_vars)
                .partitions(&self.layout)
                .build()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extend_pcr() {
        let current = [0u8; 32];
        let digest = Sha256::digest(b"test").into();
        let result = extend_pcr(&current, &digest);
        assert_eq!(
            hex::encode(result),
            "516caf854bba78a30ba2a84f9e400642c01c1a3fa429268ff5b47c32a655d4b3"
        );
    }

    #[test]
    fn test_extend_pcr_string() {
        let current = [0u8; 32];
        let result = extend_pcr_string(&current, "test");
        // Should be SHA256(zeros || SHA256("test"))
        let expected_inner: [u8; 32] = Sha256::digest(b"test").into();
        let expected = extend_pcr(&current, &expected_inner);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_extend_pcr_separator() {
        let current = [0u8; 32];
        let result = extend_pcr_separator(&current);
        // Should be SHA256(zeros || SHA256([0,0,0,0]))
        let separator_digest: [u8; 32] = Sha256::digest([0u8; 4]).into();
        let expected = extend_pcr(&current, &separator_digest);
        assert_eq!(result, expected);
        // This should match PCR 2/3/6 separator-only value
        assert_eq!(
            hex::encode(result),
            "3d458cfe55cc03ea1f443f1562beec8df51c75e14a9fcf9a7234a13f198e7969"
        );
    }

    #[test]
    fn test_pcr_record_new() {
        let digest = [0xab; 32];
        let record = PcrRecord::new(digest);
        assert_eq!(record.sha256.len(), 1);
        assert_eq!(record.sha256[0], hex::encode(digest));
    }

    #[test]
    fn test_pcr_record_new_multi() {
        let d1 = [0xab; 32];
        let d2 = [0xcd; 32];
        let record = PcrRecord::new_multi(vec![d1, d2]);
        assert_eq!(record.sha256.len(), 2);
        assert_eq!(record.sha256[0], hex::encode(d1));
        assert_eq!(record.sha256[1], hex::encode(d2));
    }
}
