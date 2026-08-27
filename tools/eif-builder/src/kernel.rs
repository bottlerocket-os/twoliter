// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kernel image pre-processing for the EIF kernel section.
//!
//! Firecracker's arm64 kernel loader implements the arm64 Linux boot protocol:
//! it expects a flat PE/COFF-wrapped `Image` starting with `MZ` and carrying
//! the `ARM\x64` magic at offset 56. It does **not** know how to unwrap an
//! EFI zboot outer image (a self-decompressing PE stub that carries the real
//! `Image` as a compressed payload). Recent Bottlerocket arm64 kernel-kit
//! RPMs ship `vmlinuz` as an EFI zboot image, so we have to unwrap it here
//! before handing it to Firecracker.
//!
//! On x86_64 the input is passed through unchanged; `rpm2eif` picks the
//! format (bzImage `vmlinuz` for the sidecar Nitro Enclaves loader, or ELF
//! `vmlinux` for a bare-metal PVH loader) via `--eif-kernel-format`.

use std::io::Read;

use snafu::{ensure, OptionExt, ResultExt, Snafu};

use crate::TargetArch;

/// Header of an EFI zboot image.
///
/// See `drivers/firmware/efi/libstub/zboot-header.S` in the Linux kernel tree.
/// All multi-byte integer fields are little-endian.
///
/// ```text
///   +0   msdos_magic       [u8; 2]   "MZ"
///   +2   reserved0         [u8; 2]
///   +4   zimg              [u8; 4]   "zimg"
///   +8   payload_offset    u32       offset of compressed payload
///   +12  payload_size      u32       size of compressed payload
///   +16  reserved1         [u8; 8]
///   +24  compression_type  [u8; 32]  NUL-terminated (e.g. "gzip", "zstd")
///   +56  linux_magic       [u8; 4]   "ARM\x64" (0x644d5241 LE) on arm64
///   +60  pe_header_offset  u32
/// ```
const ZBOOT_HEADER_LEN: usize = 64;
const ZBOOT_ZIMG_OFFSET: usize = 4;
const ZBOOT_PAYLOAD_OFFSET_OFFSET: usize = 8;
const ZBOOT_PAYLOAD_SIZE_OFFSET: usize = 12;
const ZBOOT_COMPRESSION_TYPE_OFFSET: usize = 24;
const ZBOOT_COMPRESSION_TYPE_LEN: usize = 32;

/// Offset of the arm64 Linux header magic (`ARM\x64`) inside the flat `Image`
/// header. Also present at the same offset in the zboot outer image (which
/// masquerades as an arm64 kernel to bootloaders that peek at the magic).
const ARM64_LINUX_MAGIC_OFFSET: usize = 56;
const ARM64_LINUX_MAGIC: &[u8; 4] = b"ARM\x64";

const MSDOS_MAGIC: &[u8; 2] = b"MZ";
const ZBOOT_MAGIC: &[u8; 4] = b"zimg";

/// Reasonable upper bound for a decompressed arm64 kernel `Image`. Real
/// Bottlerocket kernels are ~30-60 MiB uncompressed; 256 MiB gives us plenty
/// of headroom while still bounding memory use if we're handed a hostile or
/// malformed compressed stream.
const MAX_DECOMPRESSED_KERNEL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum KernelPrepError {
    /// The zboot header claims a payload region that doesn't fit in the file.
    #[snafu(display(
        "zboot payload region [{offset}..{}] exceeds file length ({file_len})",
        u64::from(*offset) + u64::from(*size)
    ))]
    ZbootPayloadOutOfBounds {
        offset: u32,
        size: u32,
        file_len: usize,
    },

    /// The zboot header's `compression_type` field isn't valid UTF-8 or isn't
    /// one of the compression types we know how to decode.
    #[snafu(display("unsupported zboot compression type {compression:?}: expected gzip or zstd"))]
    ZbootUnsupportedCompression { compression: String },

    /// Decompressing the zboot payload failed.
    #[snafu(display("failed to decompress zboot payload ({compression}): {source}"))]
    ZbootDecompress {
        compression: String,
        source: std::io::Error,
    },

    /// The (possibly decompressed) arm64 kernel image doesn't start with `MZ`
    /// or lacks the `ARM\x64` magic at offset 56, so Firecracker's PE loader
    /// will reject it. This is a caller error: they gave us something that
    /// isn't a bootable arm64 kernel Image.
    #[snafu(display(
        "arm64 kernel image is not a bootable PE Image (missing 'MZ' at offset 0 \
         or 'ARM\\x64' magic at offset 56); Firecracker's PE loader will reject it. \
         Supply an uncompressed arch/arm64/boot/Image, or an EFI zboot vmlinuz \
         wrapping one"
    ))]
    Arm64NotPeImage,
}

/// True if `data` looks like an EFI zboot image: `MZ` at offset 0 and `"zimg"`
/// at offset 4. A plain arm64 `Image` also starts with `MZ` (via its own
/// PE stub), so the `"zimg"` marker is what distinguishes the two.
fn is_zboot(data: &[u8]) -> bool {
    data.len() >= ZBOOT_HEADER_LEN
        && &data[..2] == MSDOS_MAGIC
        && &data[ZBOOT_ZIMG_OFFSET..ZBOOT_ZIMG_OFFSET + 4] == ZBOOT_MAGIC
}

/// True if `data` looks like a flat arm64 PE `Image` that Firecracker's PE
/// loader will accept: `MZ` at offset 0 and `ARM\x64` at offset 56.
fn is_arm64_pe_image(data: &[u8]) -> bool {
    data.len() >= ARM64_LINUX_MAGIC_OFFSET + 4
        && &data[..2] == MSDOS_MAGIC
        && &data[ARM64_LINUX_MAGIC_OFFSET..ARM64_LINUX_MAGIC_OFFSET + 4] == ARM64_LINUX_MAGIC
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().expect("4-byte slice"))
}

/// Parse a NUL-terminated ASCII string out of a fixed-size field.
fn read_c_string(data: &[u8]) -> Result<&str, std::str::Utf8Error> {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    std::str::from_utf8(&data[..end])
}

fn decompress_gzip(payload: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut decoder = flate2::read::GzDecoder::new(payload).take(MAX_DECOMPRESSED_KERNEL_BYTES + 1);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    if out.len() as u64 > MAX_DECOMPRESSED_KERNEL_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("decompressed kernel exceeds {MAX_DECOMPRESSED_KERNEL_BYTES} byte limit"),
        ));
    }
    Ok(out)
}

fn decompress_zstd(payload: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut decoder =
        zstd::stream::read::Decoder::new(payload)?.take(MAX_DECOMPRESSED_KERNEL_BYTES + 1);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    if out.len() as u64 > MAX_DECOMPRESSED_KERNEL_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("decompressed kernel exceeds {MAX_DECOMPRESSED_KERNEL_BYTES} byte limit"),
        ));
    }
    Ok(out)
}

/// Unwrap an EFI zboot image, returning the decompressed payload. Assumes the
/// caller already verified the zboot magic via `is_zboot`.
fn unwrap_zboot(data: &[u8]) -> Result<Vec<u8>, KernelPrepError> {
    let payload_offset = read_u32_le(data, ZBOOT_PAYLOAD_OFFSET_OFFSET);
    let payload_size = read_u32_le(data, ZBOOT_PAYLOAD_SIZE_OFFSET);

    let bounds_ctx = || ZbootPayloadOutOfBoundsSnafu {
        offset: payload_offset,
        size: payload_size,
        file_len: data.len(),
    };
    let end = (payload_offset as usize)
        .checked_add(payload_size as usize)
        .context(bounds_ctx())?;
    ensure!(end <= data.len(), bounds_ctx());

    let ctype_field = &data
        [ZBOOT_COMPRESSION_TYPE_OFFSET..ZBOOT_COMPRESSION_TYPE_OFFSET + ZBOOT_COMPRESSION_TYPE_LEN];
    let compression = read_c_string(ctype_field)
        .ok()
        .context(ZbootUnsupportedCompressionSnafu {
            compression: "<non-utf8>",
        })?
        .to_string();

    let payload = &data[payload_offset as usize..end];
    let decompressed = match compression.as_str() {
        "gzip" => decompress_gzip(payload).context(ZbootDecompressSnafu {
            compression: compression.clone(),
        })?,
        "zstd" => decompress_zstd(payload).context(ZbootDecompressSnafu {
            compression: compression.clone(),
        })?,
        // The Linux zboot infrastructure supports lz4/xz/lzma/lzo in theory,
        // but no arm64 distro we've seen ships those. Fail loudly rather than
        // silently misinterpret.
        _ => return ZbootUnsupportedCompressionSnafu { compression }.fail(),
    };
    Ok(decompressed)
}

/// Prepare a raw kernel image for embedding in the EIF kernel section.
///
/// For `x86_64`, returns the input verbatim. `rpm2eif` picks the format
/// (`vmlinuz` bzImage for the sidecar Nitro Enclaves loader, or an ELF
/// `vmlinux` for a bare-metal PVH loader) via `--eif-kernel-format`; both
/// are pass-through here.
///
/// For `aarch64`, if the input is an EFI zboot image (`MZ` + `"zimg"`),
/// extracts and decompresses the inner arm64 `Image`. Otherwise the input is
/// passed through. In both cases the result is validated to be a bootable
/// PE-wrapped arm64 `Image` (`MZ` + `ARM\x64` magic) so we fail here with a
/// clear message rather than shipping an EIF that Firecracker will reject at
/// launch with a cryptic "invalid Image magic number".
pub fn prepare_kernel(data: Vec<u8>, arch: TargetArch) -> Result<Vec<u8>, KernelPrepError> {
    match arch {
        TargetArch::X86_64 => Ok(data),
        TargetArch::Aarch64 => {
            let out = if is_zboot(&data) {
                unwrap_zboot(&data)?
            } else {
                data
            };
            ensure!(is_arm64_pe_image(&out), Arm64NotPeImageSnafu);
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    /// Build a minimal fake arm64 PE `Image`: `MZ` at 0, arbitrary bytes up to
    /// offset 56, `ARM\x64` magic at 56, then some payload. Not bootable, but
    /// good enough to satisfy the shape checks.
    fn fake_arm64_image(payload_len: usize) -> Vec<u8> {
        let mut img = vec![0u8; ARM64_LINUX_MAGIC_OFFSET + 4 + payload_len];
        img[0..2].copy_from_slice(MSDOS_MAGIC);
        img[ARM64_LINUX_MAGIC_OFFSET..ARM64_LINUX_MAGIC_OFFSET + 4]
            .copy_from_slice(ARM64_LINUX_MAGIC);
        // Fill the payload region with a recognizable pattern so we can
        // assert on it after a round-trip.
        for (i, byte) in img
            .iter_mut()
            .enumerate()
            .skip(ARM64_LINUX_MAGIC_OFFSET + 4)
        {
            *byte = (i & 0xff) as u8;
        }
        img
    }

    /// Wrap `payload` (a raw arm64 `Image`) in a fake gzip-zboot image with a
    /// valid header. The stub bytes preceding the payload are arbitrary; the
    /// only thing that matters is that the header fields point at the
    /// compressed payload region correctly.
    fn fake_zboot_gzip(payload: &[u8]) -> Vec<u8> {
        let mut compressed = Vec::new();
        {
            let mut enc = GzEncoder::new(&mut compressed, Compression::default());
            enc.write_all(payload).unwrap();
            enc.finish().unwrap();
        }

        // 512-byte "stub" region before the payload — mimics the real layout
        // where the header sits inside a small PE stub. The header itself
        // lives in the first 64 bytes.
        let stub_len: u32 = 512;
        let mut out = vec![0u8; stub_len as usize];
        out[0..2].copy_from_slice(MSDOS_MAGIC);
        out[ZBOOT_ZIMG_OFFSET..ZBOOT_ZIMG_OFFSET + 4].copy_from_slice(ZBOOT_MAGIC);
        out[ZBOOT_PAYLOAD_OFFSET_OFFSET..ZBOOT_PAYLOAD_OFFSET_OFFSET + 4]
            .copy_from_slice(&stub_len.to_le_bytes());
        out[ZBOOT_PAYLOAD_SIZE_OFFSET..ZBOOT_PAYLOAD_SIZE_OFFSET + 4]
            .copy_from_slice(&(compressed.len() as u32).to_le_bytes());
        let ctype = b"gzip";
        out[ZBOOT_COMPRESSION_TYPE_OFFSET..ZBOOT_COMPRESSION_TYPE_OFFSET + ctype.len()]
            .copy_from_slice(ctype);
        // The zboot outer image is itself PE/COFF and carries the same
        // Linux header magic at offset 56 (so bootloaders that peek at it
        // see an arm64 kernel). Include it so we can also verify our
        // `is_zboot` distinguishes from a plain Image.
        out[ARM64_LINUX_MAGIC_OFFSET..ARM64_LINUX_MAGIC_OFFSET + 4]
            .copy_from_slice(ARM64_LINUX_MAGIC);

        out.extend_from_slice(&compressed);
        out
    }

    fn fake_zboot_with_compression(payload: &[u8], compressed: &[u8], ctype: &[u8]) -> Vec<u8> {
        let stub_len: u32 = 512;
        let mut out = vec![0u8; stub_len as usize];
        out[0..2].copy_from_slice(MSDOS_MAGIC);
        out[ZBOOT_ZIMG_OFFSET..ZBOOT_ZIMG_OFFSET + 4].copy_from_slice(ZBOOT_MAGIC);
        out[ZBOOT_PAYLOAD_OFFSET_OFFSET..ZBOOT_PAYLOAD_OFFSET_OFFSET + 4]
            .copy_from_slice(&stub_len.to_le_bytes());
        out[ZBOOT_PAYLOAD_SIZE_OFFSET..ZBOOT_PAYLOAD_SIZE_OFFSET + 4]
            .copy_from_slice(&(compressed.len() as u32).to_le_bytes());
        out[ZBOOT_COMPRESSION_TYPE_OFFSET..ZBOOT_COMPRESSION_TYPE_OFFSET + ctype.len()]
            .copy_from_slice(ctype);
        out[ARM64_LINUX_MAGIC_OFFSET..ARM64_LINUX_MAGIC_OFFSET + 4]
            .copy_from_slice(ARM64_LINUX_MAGIC);
        let _ = payload; // used only by callers that also build `compressed`
        out.extend_from_slice(compressed);
        out
    }

    #[test]
    fn x86_pass_through() {
        // On x86 we don't touch the bytes even if they're pure garbage: the
        // caller (rpm2eif, per --eif-kernel-format) is responsible for
        // handing us either a bzImage vmlinuz or an ELF vmlinux; either is
        // a pass-through at this layer.
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let out = prepare_kernel(data.clone(), TargetArch::X86_64).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn arm64_plain_image_pass_through() {
        let img = fake_arm64_image(128);
        let out = prepare_kernel(img.clone(), TargetArch::Aarch64).unwrap();
        assert_eq!(out, img);
    }

    #[test]
    fn arm64_zboot_gzip_is_unwrapped() {
        let inner = fake_arm64_image(4096);
        let wrapped = fake_zboot_gzip(&inner);

        // Sanity: wrapped must be identifiable as zboot and different from inner.
        assert!(is_zboot(&wrapped));
        assert_ne!(wrapped, inner);

        let out = prepare_kernel(wrapped, TargetArch::Aarch64).unwrap();
        assert_eq!(out, inner, "decompressed payload must byte-match the input");
        assert!(is_arm64_pe_image(&out));
    }

    #[test]
    fn arm64_zboot_zstd_is_unwrapped() {
        let inner = fake_arm64_image(2048);
        let compressed = zstd::encode_all(inner.as_slice(), 3).unwrap();
        let wrapped = fake_zboot_with_compression(&inner, &compressed, b"zstd");

        let out = prepare_kernel(wrapped, TargetArch::Aarch64).unwrap();
        assert_eq!(out, inner);
    }

    #[test]
    fn arm64_zboot_unsupported_compression_errors() {
        // Use lz4 to represent a compression type we don't support. The
        // "compressed" bytes don't need to be valid — we should fail before
        // ever calling a decoder.
        let inner = fake_arm64_image(128);
        let wrapped = fake_zboot_with_compression(&inner, b"garbage", b"lz4");
        let err = prepare_kernel(wrapped, TargetArch::Aarch64).unwrap_err();
        assert!(
            matches!(err, KernelPrepError::ZbootUnsupportedCompression { ref compression } if compression == "lz4"),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn arm64_zboot_out_of_bounds_payload_errors() {
        let inner = fake_arm64_image(64);
        let compressed = flate_encode(&inner);
        let mut wrapped = fake_zboot_with_compression(&inner, &compressed, b"gzip");
        // Blow up the payload size so it exceeds the file length.
        let bad_size: u32 = (wrapped.len() as u32) + 1_000_000;
        wrapped[ZBOOT_PAYLOAD_SIZE_OFFSET..ZBOOT_PAYLOAD_SIZE_OFFSET + 4]
            .copy_from_slice(&bad_size.to_le_bytes());
        let err = prepare_kernel(wrapped, TargetArch::Aarch64).unwrap_err();
        assert!(
            matches!(err, KernelPrepError::ZbootPayloadOutOfBounds { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn arm64_non_pe_input_errors() {
        // Not zboot, not a PE Image — this is what triggered the bug report.
        // We should reject it up-front rather than letting Firecracker do so
        // with a cryptic message at launch.
        let err = prepare_kernel(vec![0x1f, 0x8b, 0x08, 0x00], TargetArch::Aarch64).unwrap_err();
        assert!(
            matches!(err, KernelPrepError::Arm64NotPeImage),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn arm64_decompressed_but_missing_magic_errors() {
        // If somehow the zboot inner payload isn't a real arm64 Image (e.g.
        // corrupted or misidentified), the final PE-shape check catches it.
        let bogus_inner = vec![0u8; 4096];
        let compressed = flate_encode(&bogus_inner);
        let wrapped = fake_zboot_with_compression(&bogus_inner, &compressed, b"gzip");
        let err = prepare_kernel(wrapped, TargetArch::Aarch64).unwrap_err();
        assert!(
            matches!(err, KernelPrepError::Arm64NotPeImage),
            "unexpected error: {err:?}"
        );
    }

    fn flate_encode(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = GzEncoder::new(&mut out, Compression::default());
        enc.write_all(payload).unwrap();
        enc.finish().unwrap();
        out
    }
}
