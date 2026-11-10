//! dm-verity table parsing, so tables can be compared beyond their root hash and salt.

use crate::error::Result;
use regex::bytes::Regex;
use snafu::{ensure_whatever, OptionExt, ResultExt};
use std::sync::LazyLock;

/// The verity target of a device-mapper table, as in `dm-mod.create=`.
static TABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"verity (?P<version>\d+) (?P<data_dev>\S+) (?P<hash_dev>\S+) (?P<dbs>\d+) (?P<hbs>\d+) (?P<blocks>\d+) (?P<start>\d+) (?P<alg>[a-z0-9]+) (?P<root>[0-9a-fA-F]+) (?P<salt>[0-9a-fA-F]+|-)",
    )
    .expect("valid regex")
});

/// Parameters of a dm-verity device as given in its device-mapper table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerityTable {
    pub version: u32,
    pub data_device: String,
    pub hash_device: String,
    pub data_block_size: u32,
    pub hash_block_size: u32,
    pub data_blocks: u64,
    pub hash_start_block: u64,
    pub algorithm: String,
    pub root_hash: Vec<u8>,
    pub salt: Vec<u8>,
}

impl VerityTable {
    /// Parse the verity table in `text`, which must contain exactly one.
    pub fn find_unique(text: &[u8]) -> Result<Self> {
        let mut tables = TABLE_RE.captures_iter(text);
        let caps = tables.next().whatever_context("no dm-verity table found")?;
        ensure_whatever!(
            tables.next().is_none(),
            "more than one dm-verity table found"
        );
        let s = |name: &str| String::from_utf8_lossy(&caps[name]).into_owned();
        (|| {
            let salt = match s("salt").as_str() {
                "-" => Vec::new(),
                hex_salt => hex::decode(hex_salt).whatever_context("invalid verity salt")?,
            };
            Ok(Self {
                version: s("version")
                    .parse()
                    .whatever_context("invalid verity version")?,
                data_device: s("data_dev"),
                hash_device: s("hash_dev"),
                data_block_size: s("dbs")
                    .parse()
                    .whatever_context("invalid data block size")?,
                hash_block_size: s("hbs")
                    .parse()
                    .whatever_context("invalid hash block size")?,
                data_blocks: s("blocks")
                    .parse()
                    .whatever_context("invalid data blocks")?,
                hash_start_block: s("start").parse().whatever_context("invalid hash start")?,
                algorithm: s("alg"),
                root_hash: hex::decode(s("root")).whatever_context("invalid verity root hash")?,
                salt,
            })
        })()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const ROOT: &str = "6bd753646492b7e0be9495e309313af09f67e10fffb415ac588c0feb1712e9e2";

    fn table() -> VerityTable {
        let cmdline = format!(
            "root=/dev/dm-0 dm-mod.create=\"root,,,ro,0 2400 verity 1 PARTUUID=$boot_uuid/PARTNROFF=1 \
             PARTUUID=$boot_uuid/PARTNROFF=2 4096 4096 300 1 sha256 {ROOT} \
             00112233445566778899aabbccddeeff 2 restart_on_corruption ignore_zero_blocks\" --"
        );
        VerityTable::find_unique(cmdline.as_bytes()).unwrap()
    }

    #[test]
    fn parses_table() {
        let table = table();
        assert_eq!(table.version, 1);
        assert_eq!(table.data_device, "PARTUUID=$boot_uuid/PARTNROFF=1");
        assert_eq!(table.hash_device, "PARTUUID=$boot_uuid/PARTNROFF=2");
        assert_eq!(table.data_block_size, 4096);
        assert_eq!(table.hash_block_size, 4096);
        assert_eq!(table.data_blocks, 300);
        assert_eq!(table.hash_start_block, 1);
        assert_eq!(table.algorithm, "sha256");
        assert_eq!(hex::encode(&table.root_hash), ROOT);
        assert_eq!(table.salt.len(), 16);
    }

    #[test]
    fn requires_exactly_one_table() {
        assert!(VerityTable::find_unique(b"root=/dev/xvda1 ro").is_err());
        let table = format!("verity 1 a b 4096 4096 10 1 sha256 {ROOT} -");
        assert!(VerityTable::find_unique(table.as_bytes())
            .unwrap()
            .salt
            .is_empty());
        let decoy = format!("{table} # {table}");
        assert!(VerityTable::find_unique(decoy.as_bytes()).is_err());
    }
}
