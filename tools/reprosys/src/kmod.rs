//! Kernel module signatures, which are appended to modules and differ per kernel build.
//!
//! `verify ami` needs none of this: an AMI rebuild installs the published kit RPMs, whose
//! modules are already signed. `verify kit` rebuilds the kernel itself, which generates a new
//! per-build module signing key, so the module signatures differ from the published kit. The
//! signature trailer is stripped by the length it records; the kernel checks the signature when
//! it loads the module.

use crate::error::Result;
use snafu::{ensure_whatever, ResultExt};
use std::borrow::Cow;
use std::io::Read;

const MODULE_SIG_MAGIC: &[u8] = b"~Module signature appended~\n";
/// Size of `struct module_signature`, whose last field is the big-endian signature length.
const MODULE_SIG_INFO_SIZE: usize = 12;

/// Whether `path` names a kernel module. Bottlerocket kernels compress modules with gzip
/// (`CONFIG_MODULE_COMPRESS_GZIP`).
pub fn is_module(path: &[u8]) -> bool {
    path.ends_with(b".ko") || path.ends_with(b".ko.gz")
}

/// Decompress a module according to its file name. An uncompressed module is borrowed. A
/// compressed module must hold a single gzip member and nothing after it.
pub fn decompress<'a>(path: &[u8], data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
    if !path.ends_with(b".gz") {
        return Ok(Cow::Borrowed(data));
    }
    // The kernel compresses modules with `gzip -n`, so the header holds no file name, comment
    // or extra field, which could hold bytes nothing compares.
    ensure_whatever!(
        data.get(3).is_some_and(|flags| flags & 0x1c == 0),
        "gzip module has a header with a file name, comment or extra field"
    );
    let mut decoder = flate2::bufread::GzDecoder::new(data);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .whatever_context("failed to decompress gzip module")?;
    ensure_whatever!(
        decoder.into_inner().is_empty(),
        "gzip module has data after its compressed stream"
    );
    Ok(Cow::Owned(out))
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

    #[test]
    fn decompresses_single_gzip_member() {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Default::default());
        encoder.write_all(b"\x7fELF").unwrap();
        let mut gz = encoder.finish().unwrap();
        assert_eq!(&*decompress(b"a.ko.gz", &gz).unwrap(), b"\x7fELF");
        assert_eq!(&*decompress(b"a.ko", b"raw").unwrap(), b"raw");
        let mut named = gz.clone();
        named[3] |= 0x08;
        assert!(decompress(b"a.ko.gz", &named).is_err());
        gz.push(0);
        assert!(decompress(b"a.ko.gz", &gz).is_err());
    }
}
