//! Kernel module signatures, which are appended to modules and differ per kernel build.

use crate::error::Result;
use flate2::read::GzDecoder;
use snafu::{ensure_whatever, ResultExt};
use std::io::{BufReader, Read, Write};

const MODULE_SIG_MAGIC: &[u8] = b"~Module signature appended~\n";
/// Size of `struct module_signature`, whose last field is the big-endian signature length.
const MODULE_SIG_INFO_SIZE: usize = 12;
/// Largest decompressed module accepted, against decompression bombs.
const MAX_MODULE_SIZE: u64 = 256 << 20;

/// Whether `path` names a kernel module, compressed or not.
pub fn is_module(path: &[u8]) -> bool {
    [".ko", ".ko.xz", ".ko.gz", ".ko.zst"]
        .iter()
        .any(|s| path.ends_with(s.as_bytes()))
}

/// Decompress a module according to its file name.
pub fn decompress(path: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if path.ends_with(b".xz") {
        let mut writer = LimitedWriter(&mut out);
        lzma_rs::xz_decompress(&mut BufReader::new(data), &mut writer)
            .whatever_context("failed to decompress xz module")?;
    } else if path.ends_with(b".gz") {
        read_limited(GzDecoder::new(data), &mut out)
            .whatever_context("failed to decompress gzip module")?;
    } else if path.ends_with(b".zst") {
        let decoder =
            zstd::Decoder::new(data).whatever_context("failed to decompress zstd module")?;
        read_limited(decoder, &mut out).whatever_context("failed to decompress zstd module")?;
    } else {
        out.extend_from_slice(data);
    }
    Ok(out)
}

fn read_limited<R: Read>(reader: R, out: &mut Vec<u8>) -> std::io::Result<()> {
    reader.take(MAX_MODULE_SIZE + 1).read_to_end(out)?;
    if out.len() as u64 > MAX_MODULE_SIZE {
        return Err(std::io::Error::other("decompressed module is too large"));
    }
    Ok(())
}

struct LimitedWriter<'a>(&'a mut Vec<u8>);

impl Write for LimitedWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if (self.0.len() + buf.len()) as u64 > MAX_MODULE_SIZE {
            return Err(std::io::Error::other("decompressed module is too large"));
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Split a module into its body and signature block, or None if it is unsigned.
pub fn split_signature(module: &[u8]) -> Result<Option<(&[u8], &[u8])>> {
    let Some(body) = module.strip_suffix(MODULE_SIG_MAGIC) else {
        return Ok(None);
    };
    ensure_whatever!(
        body.len() >= MODULE_SIG_INFO_SIZE,
        "module signature trailer is truncated"
    );
    let info_start = body.len() - MODULE_SIG_INFO_SIZE;
    let sig_len = u32::from_be_bytes(body[body.len() - 4..].try_into().expect("4 bytes"));
    let unsigned_len = usize::try_from(sig_len)
        .ok()
        .and_then(|len| info_start.checked_sub(len));
    let Some(unsigned_len) = unsigned_len else {
        snafu::whatever!("module signature length {sig_len} exceeds the module size");
    };
    Ok(Some(module.split_at(unsigned_len)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Append a signature of `sig` to `body` the way `sign-file` does.
    pub fn sign(body: &[u8], sig: &[u8]) -> Vec<u8> {
        let mut out = body.to_vec();
        out.extend_from_slice(sig);
        out.extend_from_slice(&[0, 0, 2, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&(sig.len() as u32).to_be_bytes());
        out.extend_from_slice(MODULE_SIG_MAGIC);
        out
    }

    #[test]
    fn splits_signature() {
        let signed = sign(b"\x7fELF module", b"pkcs7");
        let (body, sig) = split_signature(&signed).unwrap().unwrap();
        assert_eq!(body, b"\x7fELF module");
        assert!(sig.starts_with(b"pkcs7"));
        assert!(split_signature(b"\x7fELF unsigned").unwrap().is_none());

        // A signature length beyond the module.
        let mut signed = signed;
        let len_at = signed.len() - MODULE_SIG_MAGIC.len() - 4;
        signed[len_at..len_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(split_signature(&signed).is_err());
        assert!(split_signature(MODULE_SIG_MAGIC).is_err());
    }
}
