//! The kernel image `/boot/vmlinuz`, which embeds the per-build module signing certificate.
//!
//! The certificate is inside the compressed kernel, so the kernel is unwrapped from its boot
//! wrapper (an x86 bzImage or an aarch64 EFI zboot image) and decompressed. The kernels must then
//! be the same length, and differ only within one certificate-sized window and, on x86, in the
//! GNU build ID. The boot wrapper itself is not compared.

use crate::error::Result;
use snafu::{ensure_whatever, OptionExt, ResultExt};
use std::io::Read;

/// The most bytes the certificate's differences may span. An RSA-4096 certificate is under 2 KiB.
const MAX_CERT_SIZE: usize = 4096;

/// The header of an ELF note holding a 20-byte GNU build ID.
const GNU_BUILD_ID: &[u8] = b"\x04\0\0\0\x14\0\0\0\x03\0\0\0GNU\0";

fn le32(data: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// The decompressed kernel in a kernel image, and whether the image is an x86 bzImage.
fn unwrap(image: &[u8]) -> Result<(Vec<u8>, bool)> {
    let x86 = image.get(0x202..0x206) == Some(b"HdrS");
    // The bzImage payload offset is relative to the code after the setup sectors.
    let (start, len) = if x86 {
        let setup_sects = match image[0x1f1] {
            0 => 4,
            n => usize::from(n),
        };
        let setup = (setup_sects + 1) * 512;
        (le32(image, 0x248).map(|o| setup + o), le32(image, 0x24c))
    } else {
        ensure_whatever!(
            image.get(4..8) == Some(b"zimg"),
            "not a bzImage or zboot image"
        );
        (le32(image, 8), le32(image, 12))
    };
    let payload = start
        .zip(len)
        .and_then(|(start, len)| image.get(start..start.checked_add(len)?))
        .whatever_context("kernel payload is outside the image")?;
    let mut kernel = Vec::new();
    // The kernel build compresses with `zstd` or `gzip -n` and appends the decompressed size, and
    // nothing else may follow, since nothing would compare it.
    let rest = if payload.starts_with(b"\x28\xb5\x2f\xfd") {
        let len = zstd::zstd_safe::find_frame_compressed_size(payload).unwrap_or(usize::MAX);
        let frame = payload
            .get(..len)
            .whatever_context("bad zstd kernel payload")?;
        kernel = zstd::decode_all(frame).whatever_context("bad zstd kernel payload")?;
        &payload[len..]
    } else {
        ensure_whatever!(
            payload.get(3) == Some(&0),
            "kernel payload is not zstd or plain gzip"
        );
        let mut decoder = flate2::bufread::GzDecoder::new(payload);
        decoder
            .read_to_end(&mut kernel)
            .whatever_context("bad gzip kernel payload")?;
        decoder.into_inner()
    };
    ensure_whatever!(
        rest.is_empty() || rest == (kernel.len() as u32).to_le_bytes(),
        "kernel payload has data after the compressed kernel"
    );
    Ok((kernel, x86))
}

/// Compare a rebuilt kernel image against a published one, returning a description of the
/// expected difference or the reason the images differ in other ways.
pub fn compare_vmlinuz(rebuilt: &[u8], published: &[u8]) -> Result<String> {
    let (mut r, x86) = unwrap(rebuilt).whatever_context("rebuilt kernel image")?;
    let (mut p, _) = unwrap(published).whatever_context("published kernel image")?;
    ensure_whatever!(
        r.len() == p.len(),
        "the kernels are {} and {} bytes",
        r.len(),
        p.len()
    );
    if x86 {
        let find = |data: &[u8]| {
            data.windows(GNU_BUILD_ID.len())
                .position(|w| w == GNU_BUILD_ID)
        };
        let at = find(&r).filter(|at| find(&p) == Some(*at) && at + 36 <= r.len());
        let at = at.whatever_context("the kernels have no GNU build ID at the same offset")? + 16;
        r[at..at + 20].fill(0);
        p[at..at + 20].fill(0);
    }
    let differs = |(a, b): (&u8, &u8)| a != b;
    let first = r.iter().zip(&p).position(differs);
    let first = first.whatever_context("the kernels are identical; the boot wrappers differ")?;
    let last = r.iter().zip(&p).rposition(differs).expect("a byte differs");
    ensure_whatever!(
        last - first < MAX_CERT_SIZE,
        "the kernels differ at offsets {first:#x} to {last:#x}, more than a certificate spans"
    );
    Ok(format!(
        "the decompressed kernels differ only at offsets {first:#x} to {last:#x}, where the \
         module signing certificate is; the boot wrapper is not compared"
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An x86 kernel image with a build ID, a certificate and code, as the kernel build makes it.
    pub(crate) fn x86_image(build_id: u8, cert: u8, code: &[u8]) -> Vec<u8> {
        let kernel = [
            GNU_BUILD_ID,
            &[build_id; 20],
            &[cert; 600],
            &[0; 5000],
            code,
        ]
        .concat();
        let mut payload = zstd::encode_all(&kernel[..], 0).unwrap();
        payload.extend((kernel.len() as u32).to_le_bytes());
        let mut image = vec![0; 2560];
        image[0x202..0x206].copy_from_slice(b"HdrS");
        image[0x24c..0x250].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        [image, payload].concat()
    }

    #[test]
    fn only_certificate_and_build_id_differences_are_expected() {
        let rebuilt = x86_image(1, 1, b"code");
        assert!(compare_vmlinuz(&rebuilt, &x86_image(2, 2, b"code")).is_ok());
        assert!(compare_vmlinuz(&rebuilt, &x86_image(2, 2, b"evil")).is_err());
        // Data after the compressed kernel, within the payload.
        let mut trailing = x86_image(2, 2, b"code");
        trailing.push(0);
        let len = u32::from_le_bytes(trailing[0x24c..0x250].try_into().unwrap()) + 1;
        trailing[0x24c..0x250].copy_from_slice(&len.to_le_bytes());
        assert!(compare_vmlinuz(&rebuilt, &trailing).is_err());
    }
}
