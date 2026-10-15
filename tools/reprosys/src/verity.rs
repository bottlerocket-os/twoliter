//! dm-verity table parsing, so tables can be compared beyond their root hash, salt
//! and data size, and hash tree verification.

use crate::error::Result;
use regex::bytes::Regex;
use sha2_0_11::{Digest, Sha256};
use snafu::{ensure_whatever, OptionExt, ResultExt};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::LazyLock;

/// The verity target of a device-mapper table, as in `dm-mod.create=`, with the
/// start and length (in 512-byte sectors) of the target when given.
static TABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:\b(?P<target_start>\d+) (?P<sectors>\d+) )?verity (?P<version>\d+) (?P<data_dev>\S+) (?P<hash_dev>\S+) (?P<dbs>\d+) (?P<hbs>\d+) (?P<blocks>\d+) (?P<start>\d+) (?P<alg>[a-z0-9]+) (?P<root>[0-9a-fA-F]+) (?P<salt>[0-9a-fA-F]+|-)",
    )
    .expect("valid regex")
});

/// The size of a device-mapper sector.
pub const SECTOR_SIZE: u64 = 512;

/// Parameters of a dm-verity device as given in its device-mapper table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerityTable {
    /// The length of the device-mapper target in sectors, if the table gives it.
    pub sectors: Option<u64>,
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
            let sectors = caps
                .name("sectors")
                .map(|m| String::from_utf8_lossy(m.as_bytes()).parse())
                .transpose()
                .whatever_context("invalid device-mapper target length")?;
            Ok(Self {
                sectors,
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

    /// The size of the data device the table covers, in bytes.
    pub fn data_size(&self) -> Option<u64> {
        self.data_blocks
            .checked_mul(u64::from(self.data_block_size))
    }
}

/// Replace what depends on the contents of the data device in every dm-verity
/// table in `text` with placeholders: the target length, the data block count,
/// the root hash and the salt. Every other field is kept.
pub fn normalize(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut last = 0;
    for caps in TABLE_RE.captures_iter(text) {
        for (name, placeholder) in [
            ("sectors", &b"<data-sectors>"[..]),
            ("blocks", b"<data-blocks>"),
            ("root", b"<root-hash>"),
            ("salt", b"<salt>"),
        ] {
            if let Some(m) = caps.name(name) {
                out.extend_from_slice(&text[last..m.start()]);
                out.extend_from_slice(placeholder);
                last = m.end();
            }
        }
    }
    out.extend_from_slice(&text[last..]);
    out
}

/// A partition of a disk image.
#[derive(Debug, Clone, Copy)]
pub struct Device<'a> {
    pub file: &'a File,
    pub offset: u64,
    pub len: u64,
}

impl Device<'_> {
    fn read(&self, pos: u64, buf: &mut [u8], name: &str) -> Result<()> {
        let end = pos.checked_add(buf.len() as u64);
        ensure_whatever!(
            end.is_some_and(|end| end <= self.len),
            "the {name} device ({} bytes) is too small for the verity table",
            self.len
        );
        self.file
            .read_exact_at(buf, self.offset + pos)
            .whatever_context(format!("failed to read the {name} device"))
    }
}

/// The digest of `block` in dm-verity format version 1: sha256 of the salt, then the block.
fn digest(salt: &[u8], block: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(block);
    hasher.finalize().into()
}

/// The hash blocks holding `digests`, each padded with zeros.
fn hash_blocks(digests: &[[u8; 32]], hash_block_size: usize) -> Vec<u8> {
    let per_block = hash_block_size / 32;
    let mut out = Vec::with_capacity(digests.len().div_ceil(per_block) * hash_block_size);
    for chunk in digests.chunks(per_block) {
        let start = out.len();
        out.extend(chunk.iter().flatten());
        out.resize(start + hash_block_size, 0);
    }
    out
}

/// Check, as `veritysetup verify` does, that `hash` holds the hash tree of
/// `data` given by `table`, and that its root is the table's root hash. Only
/// format version 1 with sha256 is supported; anything else fails.
pub fn verify(table: &VerityTable, data: Device<'_>, hash: Device<'_>) -> Result<()> {
    ensure_whatever!(
        table.version == 1 && table.algorithm == "sha256",
        "verity format {} with {} is not supported",
        table.version,
        table.algorithm
    );
    let block_size = |size: u32| {
        usize::try_from(size)
            .ok()
            .filter(|s| s.is_power_of_two() && (512..=1 << 16).contains(s))
    };
    let (Some(dbs), Some(hbs)) = (
        block_size(table.data_block_size),
        block_size(table.hash_block_size),
    ) else {
        snafu::whatever!(
            "unsupported verity block sizes {} and {}",
            table.data_block_size,
            table.hash_block_size
        );
    };
    ensure_whatever!(table.data_blocks > 0, "the verity table covers no data");

    // The leaves: a digest of every data block.
    let mut digests = Vec::new();
    let mut buf = vec![0u8; dbs * 256];
    let mut block = 0u64;
    while block < table.data_blocks {
        let n = (table.data_blocks - block).min(256);
        let len = usize::try_from(n).expect("at most 256") * dbs;
        data.read(block * dbs as u64, &mut buf[..len], "data")?;
        digests.extend(buf[..len].chunks(dbs).map(|b| digest(&table.salt, b)));
        block += n;
    }

    // Each level hashes the blocks of the one below, up to a single block.
    let mut levels = vec![hash_blocks(&digests, hbs)];
    while levels.last().expect("one level").len() > hbs {
        let below = levels.last().expect("one level");
        let digests: Vec<_> = below.chunks(hbs).map(|b| digest(&table.salt, b)).collect();
        levels.push(hash_blocks(&digests, hbs));
    }
    let root = digest(&table.salt, levels.last().expect("one level"));
    ensure_whatever!(
        root[..] == table.root_hash[..],
        "the hash tree of the data device has root hash {}, not {}",
        hex::encode(root),
        hex::encode(&table.root_hash)
    );

    // The hash device stores the levels from the top down.
    let mut pos = table
        .hash_start_block
        .checked_mul(hbs as u64)
        .whatever_context("invalid verity hash start block")?;
    for (i, level) in levels.iter().enumerate().rev() {
        let mut stored = vec![0u8; level.len()];
        hash.read(pos, &mut stored, "hash")?;
        ensure_whatever!(
            stored == *level,
            "level {i} of the hash tree on the hash device does not match the data device"
        );
        pos += level.len() as u64;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The root hash and hash device of `data` with 4096-byte blocks, as
    /// `veritysetup format` writes them after a one-block superblock.
    pub fn format(data: &[u8], salt: &[u8]) -> (String, Vec<u8>) {
        assert!(data.len().is_multiple_of(4096) && !data.is_empty());
        let digests: Vec<_> = data.chunks(4096).map(|b| digest(salt, b)).collect();
        let mut levels = vec![hash_blocks(&digests, 4096)];
        while levels.last().unwrap().len() > 4096 {
            let below = levels.last().unwrap();
            let digests: Vec<_> = below.chunks(4096).map(|b| digest(salt, b)).collect();
            levels.push(hash_blocks(&digests, 4096));
        }
        let root = hex::encode(digest(salt, levels.last().unwrap()));
        let mut hash = vec![0u8; 4096];
        hash[..8].copy_from_slice(b"verity\0\0");
        for level in levels.iter().rev() {
            hash.extend_from_slice(level);
        }
        (root, hash)
    }

    fn device(file: &File, len: usize) -> Device<'_> {
        Device {
            file,
            offset: 0,
            len: len as u64,
        }
    }

    fn table_for(data_blocks: u64, root: &str, salt: &str) -> VerityTable {
        let text = format!("verity 1 a b 4096 4096 {data_blocks} 1 sha256 {root} {salt}");
        VerityTable::find_unique(text.as_bytes()).unwrap()
    }

    #[test]
    fn verifies_hash_trees() {
        // 200 blocks need two levels.
        let data: Vec<u8> = (0..200 * 4096).map(|i| (i / 4096) as u8).collect();
        let (root, hash) = format(&data, &[0x11]);
        let file = |bytes: &[u8]| {
            let f = tempfile::tempfile().unwrap();
            f.write_all_at(bytes, 0).unwrap();
            f
        };
        let (data_file, hash_file) = (file(&data), file(&hash));
        let check = |table: &VerityTable, data: &File, hash: &File| {
            verify(
                table,
                device(data, 200 * 4096),
                device(hash, hash.metadata().unwrap().len() as usize),
            )
        };
        check(&table_for(200, &root, "11"), &data_file, &hash_file).unwrap();

        let err = check(&table_for(200, &root, "12"), &data_file, &hash_file).unwrap_err();
        assert!(err.to_string().contains("has root hash"), "{err}");
        // More data than the device holds.
        assert!(check(&table_for(201, &root, "11"), &data_file, &hash_file).is_err());

        let mut tampered = data.clone();
        tampered[150 * 4096] ^= 1;
        assert!(check(&table_for(200, &root, "11"), &file(&tampered), &hash_file).is_err());

        // A leaf hash block that does not match, even though the root does.
        let mut bad_leaf = hash.clone();
        bad_leaf[3 * 4096 - 1] ^= 1;
        let err = check(&table_for(200, &root, "11"), &data_file, &file(&bad_leaf)).unwrap_err();
        assert!(
            err.to_string().starts_with("level 0 of the hash tree"),
            "{err}"
        );
    }

    #[test]
    #[ignore = "needs veritysetup"]
    fn hash_trees_match_veritysetup() {
        let dir = tempfile::tempdir().unwrap();
        let (data_path, hash_path) = (dir.path().join("data"), dir.path().join("hash"));
        let data: Vec<u8> = (0..300 * 4096).map(|i| (i % 251) as u8).collect();
        std::fs::write(&data_path, &data).unwrap();
        let output = std::process::Command::new("veritysetup")
            .args(["format", "--salt", "0011", "--data-block-size", "4096"])
            .args(["--hash-block-size", "4096"])
            .arg(&data_path)
            .arg(&hash_path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let root = stdout
            .lines()
            .find_map(|l| l.strip_prefix("Root hash:"))
            .unwrap()
            .trim();
        let (computed_root, computed_hash) = format(&data, &[0x00, 0x11]);
        assert_eq!(computed_root, root);
        let hash = std::fs::read(&hash_path).unwrap();
        assert_eq!(hash[4096..], computed_hash[4096..]);
    }

    #[test]
    fn normalizes_data_size_and_digests_only() {
        let cmdline = |sectors: u64, blocks: u64, root: &str, salt: &str, rest: &str| {
            format!(
                "ro dm-mod.create=\"root,,,ro,0 {sectors} verity 1 d h 4096 4096 {blocks} 1 \
                 sha256 {root} {salt} 2 restart_on_corruption{rest}\" --"
            )
            .into_bytes()
        };
        let rebuilt = cmdline(2400, 300, ROOT, "00", "");
        assert_eq!(
            String::from_utf8(normalize(&rebuilt)).unwrap(),
            "ro dm-mod.create=\"root,,,ro,0 <data-sectors> verity 1 d h 4096 4096 <data-blocks> 1 \
             sha256 <root-hash> <salt> 2 restart_on_corruption\" --"
        );
        let other_root = "b".repeat(64);
        assert_eq!(
            normalize(&rebuilt),
            normalize(&cmdline(2408, 301, &other_root, "11", ""))
        );
        assert_ne!(
            normalize(&rebuilt),
            normalize(&cmdline(2400, 300, ROOT, "00", " ignore_zero_blocks"))
        );
        // Every other field still counts, including the target start.
        let moved = String::from_utf8(rebuilt.clone())
            .unwrap()
            .replace(",0 2400", ",8 2400");
        assert_ne!(normalize(&rebuilt), normalize(moved.as_bytes()));
        let hbs = String::from_utf8(rebuilt.clone())
            .unwrap()
            .replace("4096 300", "512 300");
        assert_ne!(normalize(&rebuilt), normalize(hbs.as_bytes()));
    }

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
