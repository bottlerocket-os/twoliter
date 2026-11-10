//! Masking of dm-verity tables, so boot configurations can be compared beyond
//! their verity root hash, salt and data size.

use regex::bytes::Regex;
use std::sync::LazyLock;

/// The verity target of a device-mapper table, as in `dm-mod.create=`, with the
/// start and length (in 512-byte sectors) of the target when given.
static TABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:\b(?P<target_start>\d+) (?P<sectors>\d+) )?verity (?P<version>\d+) (?P<data_dev>\S+) (?P<hash_dev>\S+) (?P<dbs>\d+) (?P<hbs>\d+) (?P<blocks>\d+) (?P<start>\d+) (?P<alg>[a-z0-9]+) (?P<root>[0-9a-fA-F]+) (?P<salt>[0-9a-fA-F]+|-)",
    )
    .expect("valid regex")
});

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
