//! Parser for AWS UEFI variable store format.

use super::zlib_dict::AWS_ZLIB_DICT;
use crate::efi::{EfiVar, EfiVars};
use crate::error::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{Decompress, FlushDecompress, Status};
use snafu::{ensure_whatever, ResultExt};
use std::io::{Cursor, Read};
use uuid::Uuid;

/// Size of the AWS UEFI data header: magic (8) + padding (4) + version (4).
const UEFI_HEADER_SIZE: usize = 16;

/// Parse AWS UEFI data (base64 encoded) into EfiVars.
///
/// Decodes the base64 data, verifies the "AMZNUEFI" magic header,
/// decompresses using zlib with the AWS preset dictionary, and
/// extracts the PK, KEK, db, and dbx variables.
pub fn parse_aws_uefi_data(b64_data: &str) -> Result<EfiVars> {
    let binary = STANDARD
        .decode(b64_data.trim())
        .whatever_context("failed to decode base64")?;

    ensure_whatever!(binary.len() >= UEFI_HEADER_SIZE, "UEFI data too short");
    ensure_whatever!(&binary[0..8] == b"AMZNUEFI", "invalid magic");

    let version_bytes: [u8; 4] = binary[12..16]
        .try_into()
        .whatever_context("failed to read version bytes")?;
    let version = u32::from_le_bytes(version_bytes);
    ensure_whatever!(version == 0, "unsupported version: {version}");

    let decompressed = decompress_with_dict(&binary[16..])?;
    parse_variables(&decompressed)
}

/// Decompress zlib data using the AWS preset dictionary.
///
/// Uses a growing buffer approach to handle arbitrary output sizes.
/// The dictionary is set when zlib signals it needs one.
fn decompress_with_dict(compressed: &[u8]) -> Result<Vec<u8>> {
    let mut decomp = Decompress::new(true);
    let mut output = Vec::new();
    let mut in_pos = 0;

    loop {
        // Grow buffer to make room for more output
        let out_pos = output.len();
        output.resize(out_pos + 32 * 1024, 0);

        let result = decomp.decompress(
            &compressed[in_pos..],
            &mut output[out_pos..],
            FlushDecompress::Finish,
        );

        let new_out = usize::try_from(decomp.total_out()).whatever_context("total_out overflow")?;
        output.truncate(new_out);

        match result {
            Ok(Status::StreamEnd) => return Ok(output),
            Ok(Status::Ok | Status::BufError) => {
                in_pos =
                    usize::try_from(decomp.total_in()).whatever_context("total_in overflow")?;
                // Continue loop to grow buffer
            }
            Err(e) if e.needs_dictionary().is_some() => {
                decomp
                    .set_dictionary(&AWS_ZLIB_DICT)
                    .whatever_context("set dictionary")?;
                in_pos =
                    usize::try_from(decomp.total_in()).whatever_context("total_in overflow")?;
            }
            Err(e) => return Err(e).whatever_context("decompression failed"),
        }
    }
}

/// EFI variable attribute for time-based authenticated write access.
const EFI_VARIABLE_TIME_BASED_AUTHENTICATED_WRITE_ACCESS: u32 = 0x20;

/// Parse decompressed AWS UEFI variable data into EfiVars.
///
/// The format is: variable count (u64), followed by each variable containing
/// name, data, GUID, attributes, and optionally timestamp/digest for
/// authenticated variables.
fn parse_variables(data: &[u8]) -> Result<EfiVars> {
    let mut cursor = Cursor::new(data);
    let num_vars = read_u64(&mut cursor)?;
    let mut variables = Vec::new();

    for _ in 0..num_vars {
        let name = read_string(&mut cursor)?;
        let var_data = read_data(&mut cursor)?;
        let guid = read_guid(&mut cursor)?;
        let attr = read_u32(&mut cursor)?;

        if attr & EFI_VARIABLE_TIME_BASED_AUTHENTICATED_WRITE_ACCESS != 0 {
            let _timestamp = read_bytes(&mut cursor, 16)?;
            let _digest = read_data(&mut cursor)?;
        }

        if matches!(name.as_str(), "PK" | "KEK" | "db" | "dbx") {
            variables.push(EfiVar::new(name, guid, hex::encode(&var_data)));
        }
    }

    Ok(EfiVars { variables })
}

/// Read a little-endian u64 from the cursor.
fn read_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64> {
    let mut buf = [0u8; 8];
    cursor.read_exact(&mut buf).whatever_context("read u64")?;
    Ok(u64::from_le_bytes(buf))
}

/// Read a little-endian u32 from the cursor.
fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32> {
    let mut buf = [0u8; 4];
    cursor.read_exact(&mut buf).whatever_context("read u32")?;
    Ok(u32::from_le_bytes(buf))
}

/// Read exactly `len` bytes from the cursor.
fn read_bytes(cursor: &mut Cursor<&[u8]>, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    cursor.read_exact(&mut buf).whatever_context("read bytes")?;
    Ok(buf)
}

/// Read a length-prefixed byte array (u64 length followed by data).
fn read_data(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u8>> {
    let len_u64 = read_u64(cursor)?;
    let len = usize::try_from(len_u64).whatever_context("data length overflow")?;
    read_bytes(cursor, len)
}

/// Read a length-prefixed UTF-8 string.
fn read_string(cursor: &mut Cursor<&[u8]>) -> Result<String> {
    let data = read_data(cursor)?;
    String::from_utf8(data).whatever_context("invalid UTF-8")
}

/// Read a 16-byte GUID and return it as a lowercase hyphenated string.
fn read_guid(cursor: &mut Cursor<&[u8]>) -> Result<String> {
    let mut buf = [0u8; 16];
    cursor.read_exact(&mut buf).whatever_context("read GUID")?;
    Ok(Uuid::from_bytes_le(buf).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::{Compress, Compression, FlushCompress, Status};
    use std::io::Write;

    // Sample AWS UEFI data from DescribeImageAttribute (base64 encoded)
    const SAMPLE_AWS_UEFI_B64: &str = "\
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\
HM4Mz6wVsBV91Fzph80pt9gZGbF1NAAgo3UM";

    /// Compress data with standard zlib (no preset dictionary).
    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn test_parse_aws_uefi_data() {
        let result = parse_aws_uefi_data(SAMPLE_AWS_UEFI_B64).unwrap();

        // Should have PK, KEK, db, dbx with correct GUIDs
        assert_eq!(result.variables.len(), 4);
    }

    #[test]
    fn test_parse_aws_uefi_data_content() {
        let result = parse_aws_uefi_data(SAMPLE_AWS_UEFI_B64).unwrap();

        // Verify PK data starts with expected signature list header
        let pk = result.variables.iter().find(|v| v.name == "PK").unwrap();
        assert!(pk.data.starts_with("a159c0a5e494a74a87b5ab155c2bf072"));

        // Verify dbx has expected empty hash content (SHA-256 of empty)
        let dbx = result.variables.iter().find(|v| v.name == "dbx").unwrap();
        assert!(dbx.data.contains("e3b0c44298fc1c149afbf4c8996fb924"));
    }

    #[test]
    fn test_invalid_magic() {
        assert!(parse_aws_uefi_data("AAAAAAAAAAAAAAAAAAAAAA==").is_err());
    }

    #[test]
    fn test_invalid_base64() {
        assert!(parse_aws_uefi_data("not valid base64!!!").is_err());
    }

    #[test]
    fn test_too_short() {
        assert!(parse_aws_uefi_data("QUFBQQ==").is_err());
    }

    #[test]
    fn test_unsupported_version() {
        // "AMZNUEFI" + 4 bytes padding + version=1 (unsupported)
        let mut data = b"AMZNUEFI".to_vec();
        data.extend_from_slice(&[0u8; 4]); // padding
        data.extend_from_slice(&1u32.to_le_bytes()); // version = 1
        let b64 = STANDARD.encode(&data);
        let err = parse_aws_uefi_data(&b64).unwrap_err();
        assert!(err.to_string().contains("unsupported version"));
    }

    #[test]
    fn test_decompress_no_dict() {
        let original = b"hello world, this is a test of standard zlib compression";
        let result = decompress_with_dict(&zlib_compress(original)).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_decompress_with_dict_small() {
        let binary = STANDARD.decode(SAMPLE_AWS_UEFI_B64.trim()).unwrap();
        let result = decompress_with_dict(&binary[16..]).unwrap();
        assert!(result.len() > 100);
    }

    #[test]
    fn test_decompress_large_output() {
        let original: Vec<u8> = (0..65536).map(|i| (i % 256) as u8).collect();
        let result = decompress_with_dict(&zlib_compress(&original)).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_decompress_malformed() {
        assert!(decompress_with_dict(b"not valid zlib").is_err());
    }

    #[test]
    fn test_decompress_large_with_dict() {
        // Compress 64KB with the AWS preset dictionary
        let original: Vec<u8> = (0..65536).map(|i| (i % 256) as u8).collect();

        let mut comp = Compress::new(Compression::default(), true);
        comp.set_dictionary(&AWS_ZLIB_DICT).unwrap();

        let mut compressed = vec![0u8; original.len() + 1024];
        let status = comp
            .compress(&original, &mut compressed, FlushCompress::Finish)
            .unwrap();
        assert_eq!(status, Status::StreamEnd);
        compressed.truncate(comp.total_out() as usize);

        let result = decompress_with_dict(&compressed).unwrap();
        assert_eq!(result, original);
    }
}
