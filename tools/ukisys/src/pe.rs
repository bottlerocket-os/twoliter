//! Parses and patches the PE32+ COFF image structures of a Unified
//! Kernel Image: the DOS/PE headers, COFF file header, PE32+ optional
//! header including the Certificate Table data directory, and section
//! table, per the Microsoft PE Format specification
//! (<https://learn.microsoft.com/en-us/windows/win32/debug/pe-format>).
//!
//! The UKI section names this module truncates are `.osrel`, `.cmdline`,
//! `.uname`, and `.linux`. They are defined by the UAPI Group's Unified
//! Kernel Images specification
//! (<https://uapi-group.org/specifications/specs/unified_kernel_image/>).
//!
//! # Layout reference, PE32+, i.e. `PE32+` optional header magic 0x20B
//!
//! ```text
//! 0x3C                      e_lfanew: u32 LE -- file offset of "PE\0\0"
//! e_lfanew + 0              "PE\0\0" signature (4 bytes)
//! e_lfanew + 4              COFF File Header begins
//!   +4 (=coff+0)            Machine (u16)
//!   +6  (=coff+2)           NumberOfSections (u16)
//!   +20 (=coff+16)          SizeOfOptionalHeader (u16)
//!   +22 (=coff+18)          Characteristics (u16)
//! e_lfanew + 24             Optional Header begins ("opt")
//!   opt+0                   Magic (u16) -- must be 0x20B for PE32+
//!   opt+4                   SizeOfCode (u32)
//!   opt+8                   SizeOfInitializedData (u32)
//!   opt+32                  SectionAlignment (u32)
//!   opt+36                  FileAlignment (u32)
//!   opt+56                  SizeOfImage (u32)
//!   opt+60                  SizeOfHeaders (u32)
//!   opt+64                  CheckSum (u32)
//!   opt+108                 NumberOfRvaAndSizes (u32)
//!   opt+112                 DataDirectory[0] begins, 8 bytes/entry:
//!                             +0 VirtualAddress (u32), +4 Size (u32)
//!   opt+112 + 4*8           DataDirectory[4] (SECURITY): opt+144 / opt+148
//! opt + SizeOfOptionalHeader  Section table begins, 40 bytes/entry:
//!   +0   Name (8 bytes, not necessarily NUL-terminated if 8 chars long)
//!   +8   VirtualSize (u32)
//!   +12  VirtualAddress (u32)
//!   +16  SizeOfRawData (u32)
//!   +20  PointerToRawData (u32)
//!   +24  PointerToRelocations (u32)
//!   +28  PointerToLinenumbers (u32)
//!   +32  NumberOfRelocations (u16)
//!   +34  NumberOfLinenumbers (u16)
//!   +36  Characteristics (u32)
//! ```

use snafu::{OptionExt, Snafu};
use std::fs;
use std::path::Path;

#[derive(Debug, Snafu)]
pub enum PeError {
    #[snafu(display("Failed I/O on '{path}': {source}"))]
    Io {
        path: String,
        source: std::io::Error,
    },

    #[snafu(display("Bad optional header magic 0x{magic:04x}, expected PE32+ (0x020b)"))]
    NotPe32Plus { magic: u16 },

    #[snafu(display(
        "Failed to access {context}: file too short, needed at least {needed} bytes, have {have}"
    ))]
    TruncatedFile {
        needed: usize,
        have: usize,
        context: &'static str,
    },

    #[snafu(display(
        "Failed to access {context}: offset overflows usize (max valid offset {max}), file is {have} bytes"
    ))]
    FileTooLarge {
        context: &'static str,
        max: usize,
        have: usize,
    },

    #[snafu(display("Failed to set {context}: computed value {value} does not fit in a u32"))]
    ValueOverflow { context: &'static str, value: u64 },

    #[snafu(display("Missing 'MZ' DOS signature"))]
    BadDosSignature,

    #[snafu(display("Missing 'PE\\0\\0' signature at e_lfanew"))]
    BadPeSignature,

    #[snafu(display("Expected trailing sections {expected:?}, found {found:?}"))]
    UnexpectedTrailingSections {
        expected: Vec<String>,
        found: Vec<String>,
    },
}

/// One entry from the PE section table, plus its own index and byte offset
/// within the file, so callers can locate or patch the raw table entry.
#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub pointer_to_raw_data: u32,
    pub table_entry_offset: usize,
}

/// A parsed view over an in-memory PE32+ image.
///
/// Holds the raw bytes plus the header offsets needed to read or patch
/// fields in place.
pub struct PeImage {
    pub bytes: Vec<u8>,
    /// Offset of the "PE\0\0" signature.
    pub pe_offset: usize,
    /// Offset of the COFF File Header, i.e. `pe_offset + 4`.
    pub coff_offset: usize,
    /// Offset of the Optional Header, i.e. `pe_offset + 24`.
    pub opt_offset: usize,
    pub size_of_optional_header: u16,
    pub sections: Vec<Section>,
}

impl PeImage {
    /// Loads and parses a PE32+ image from disk.
    pub fn load(path: &Path) -> Result<Self, PeError> {
        let bytes = fs::read(path).map_err(|e| PeError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Self::parse(bytes)
    }

    /// Parses a PE32+ image already resident in memory.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, PeError> {
        if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
            return Err(PeError::BadDosSignature);
        }

        let pe_offset = read_u32(&bytes, OFF_E_LFANEW, "e_lfanew")? as usize;
        let pe_offset_end = pe_offset.checked_add(4).context(FileTooLargeSnafu {
            context: "PE signature",
            max: bytes.len().saturating_sub(4),
            have: bytes.len(),
        })?;
        if bytes.len() < pe_offset_end || &bytes[pe_offset..pe_offset_end] != b"PE\0\0" {
            // Either the file is too short to reach pe_offset_end, which
            // only a bogus e_lfanew or a genuinely truncated file can
            // cause, or the file is long enough but those 4 bytes simply
            // aren't "PE\0\0".
            return Err(PeError::BadPeSignature);
        }

        let coff_offset = pe_offset + 4;
        let num_sections = read_u16(&bytes, coff_offset + COFF_NUM_SECTIONS, "NumberOfSections")?;
        let size_of_optional_header = read_u16(
            &bytes,
            coff_offset + COFF_SIZE_OPT_HDR,
            "SizeOfOptionalHeader",
        )?;

        let opt_offset = pe_offset + 24;
        let magic = read_u16(&bytes, opt_offset + OPT_MAGIC, "OptionalHeader.Magic")?;
        if magic != PE32_PLUS_MAGIC {
            return Err(PeError::NotPe32Plus { magic });
        }

        let section_table_offset = opt_offset + size_of_optional_header as usize;
        let mut sections = Vec::with_capacity(num_sections as usize);
        for i in 0..num_sections as usize {
            // Guard against offset overflow.
            let entry_offset = i
                .checked_mul(SECTION_ENTRY_SIZE)
                .and_then(|span| section_table_offset.checked_add(span))
                .context(FileTooLargeSnafu {
                    context: "section table entry",
                    max: bytes.len().saturating_sub(SECTION_ENTRY_SIZE),
                    have: bytes.len(),
                })?;
            let entry_end =
                entry_offset
                    .checked_add(SECTION_ENTRY_SIZE)
                    .context(FileTooLargeSnafu {
                        context: "section table entry",
                        max: bytes.len().saturating_sub(SECTION_ENTRY_SIZE),
                        have: bytes.len(),
                    })?;
            // This entry's offset is larger than the file itself.
            if entry_end > bytes.len() {
                return Err(PeError::TruncatedFile {
                    needed: entry_end,
                    have: bytes.len(),
                    context: "section table entry",
                });
            }
            let mut raw_name = [0u8; 8];
            raw_name.copy_from_slice(&bytes[entry_offset..entry_offset + 8]);
            let name = decode_section_name(&raw_name);
            let virtual_size = read_u32(&bytes, entry_offset + 8, "Section.VirtualSize")?;
            let virtual_address = read_u32(&bytes, entry_offset + 12, "Section.VirtualAddress")?;
            let pointer_to_raw_data =
                read_u32(&bytes, entry_offset + 20, "Section.PointerToRawData")?;

            sections.push(Section {
                name,
                virtual_size,
                virtual_address,
                pointer_to_raw_data,
                table_entry_offset: entry_offset,
            });
        }

        Ok(PeImage {
            bytes,
            pe_offset,
            coff_offset,
            opt_offset,
            size_of_optional_header,
            sections,
        })
    }

    /// Reads the Security Directory, which is Data Directory entry 4.
    pub fn security_directory(&self) -> Result<(u32, u32), PeError> {
        let dir_offset = self.opt_offset + OPT_DATA_DIRECTORY + SECURITY_DIRECTORY_INDEX * 8;
        let rva = read_u32(&self.bytes, dir_offset, "SecurityDirectory.VirtualAddress")?;
        let size = read_u32(&self.bytes, dir_offset + 4, "SecurityDirectory.Size")?;
        Ok((rva, size))
    }

    /// Reads `NumberOfSections` from the COFF File Header.
    pub fn number_of_sections(&self) -> Result<u16, PeError> {
        read_u16(
            &self.bytes,
            self.coff_offset + COFF_NUM_SECTIONS,
            "NumberOfSections",
        )
    }

    /// Reads `SizeOfInitializedData` from the Optional Header.
    pub fn size_of_initialized_data(&self) -> Result<u32, PeError> {
        read_u32(
            &self.bytes,
            self.opt_offset + OPT_SIZE_OF_INIT_DATA,
            "SizeOfInitializedData",
        )
    }

    /// Reads `SectionAlignment` from the Optional Header.
    pub fn section_alignment(&self) -> Result<u32, PeError> {
        read_u32(
            &self.bytes,
            self.opt_offset + OPT_SECTION_ALIGNMENT,
            "SectionAlignment",
        )
    }

    /// Removes the Authenticode signature from the image, if present.
    ///
    /// An Authenticode signature is not a PE section: it is a certificate
    /// blob appended after all section data, referenced only by Data
    /// Directory entry 4, the "Security Directory". Removing it means:
    ///   1. Clear that directory entry's VirtualAddress and Size to 0.
    ///   2. Truncate the file so the trailing certificate blob is dropped.
    pub fn remove_signature(&mut self) -> Result<(), PeError> {
        let (rva, size) = self.security_directory()?;
        if size == 0 {
            return Ok(());
        }
        let cert_start = rva as usize;
        if cert_start > 0 && cert_start < self.bytes.len() {
            self.bytes.truncate(cert_start);
        }
        self.set_security_directory(0, 0)?;
        Ok(())
    }

    /// Removes a contiguous run of *trailing* sections from the image,
    /// which turns removal into a pure truncation rather than a general
    /// splice, since no kept section's file offset needs to move.
    ///
    /// `trailing_names` must exactly match, in order, the last
    /// `trailing_names.len()` entries of the section table.
    ///
    /// On success, patches in place:
    ///   - `NumberOfSections`: decremented by `trailing_names.len()`
    ///   - `SizeOfInitializedData`: decremented by the sum of the removed
    ///     sections' VirtualSize
    ///   - `SizeOfImage`: set to the last remaining section's
    ///     VirtualAddress + VirtualSize, rounded up to SectionAlignment
    ///   - `CheckSum`: set to 0
    ///
    /// and removes/zeroes the freed section-table entries.
    pub fn derive_stub_by_truncating_trailing_sections(
        &mut self,
        trailing_names: &[&str],
    ) -> Result<(), PeError> {
        let n = trailing_names.len();
        let total = self.sections.len();
        if n == 0 || n > total {
            return Err(PeError::UnexpectedTrailingSections {
                expected: trailing_names.iter().map(|s| s.to_string()).collect(),
                found: self.sections.iter().map(|s| s.name.clone()).collect(),
            });
        }

        let first_trailing_idx = total - n;
        let actual_tail: Vec<String> = self.sections[first_trailing_idx..]
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let expected_tail: Vec<String> = trailing_names.iter().map(|s| s.to_string()).collect();
        if actual_tail != expected_tail {
            return Err(PeError::UnexpectedTrailingSections {
                expected: expected_tail,
                found: actual_tail,
            });
        }

        let truncation_point = self.sections[first_trailing_idx].pointer_to_raw_data as usize;

        let removed_vsize_sum: u64 = self.sections[first_trailing_idx..]
            .iter()
            .map(|s| s.virtual_size as u64)
            .sum();

        self.bytes.truncate(truncation_point);

        // Zeroed for hygiene/determinism even though NumberOfSections will
        // no longer claim these entries exist. Some trailing entries may
        // already be gone if their table offset fell within the truncated
        // region, so skip rather than index out of the shorter buffer.
        for sec in &self.sections[first_trailing_idx..] {
            let off = sec.table_entry_offset;
            if off + SECTION_ENTRY_SIZE > self.bytes.len() {
                continue;
            }
            for b in &mut self.bytes[off..off + SECTION_ENTRY_SIZE] {
                *b = 0;
            }
        }

        let old_num_sections = self.number_of_sections()?;
        let new_num_sections = old_num_sections - n as u16;
        self.set_number_of_sections(new_num_sections)?;

        let old_init_data = self.size_of_initialized_data()?;
        let new_init_data = (old_init_data as u64).saturating_sub(removed_vsize_sum) as u32;
        self.set_size_of_initialized_data(new_init_data)?;

        let last_remaining = &self.sections[first_trailing_idx - 1];
        let raw_end = last_remaining.virtual_address as u64 + last_remaining.virtual_size as u64;
        let section_alignment = self.section_alignment()? as u64;
        let new_size_of_image = if section_alignment > 0 {
            raw_end.div_ceil(section_alignment) * section_alignment
        } else {
            raw_end
        };
        let new_size_of_image =
            u32::try_from(new_size_of_image).map_err(|_| PeError::ValueOverflow {
                context: "SizeOfImage",
                value: new_size_of_image,
            })?;
        self.set_size_of_image(new_size_of_image)?;

        // CheckSum is zeroed rather than recomputed, matching `ukify
        // build`'s own behavior.
        self.set_checksum(0)?;

        self.sections.truncate(first_trailing_idx);

        Ok(())
    }

    /// Writes the image's current bytes to disk at `path`.
    pub fn write_to(&self, path: &Path) -> Result<(), PeError> {
        fs::write(path, &self.bytes).map_err(|e| PeError::Io {
            path: path.display().to_string(),
            source: e,
        })
    }

    fn set_security_directory(&mut self, rva: u32, size: u32) -> Result<(), PeError> {
        let dir_offset = self.opt_offset + OPT_DATA_DIRECTORY + SECURITY_DIRECTORY_INDEX * 8;
        write_u32(
            &mut self.bytes,
            dir_offset,
            rva,
            "SecurityDirectory.VirtualAddress",
        )?;
        write_u32(
            &mut self.bytes,
            dir_offset + 4,
            size,
            "SecurityDirectory.Size",
        )?;
        Ok(())
    }

    fn set_number_of_sections(&mut self, value: u16) -> Result<(), PeError> {
        write_u16(
            &mut self.bytes,
            self.coff_offset + COFF_NUM_SECTIONS,
            value,
            "NumberOfSections",
        )
    }

    fn set_size_of_initialized_data(&mut self, value: u32) -> Result<(), PeError> {
        write_u32(
            &mut self.bytes,
            self.opt_offset + OPT_SIZE_OF_INIT_DATA,
            value,
            "SizeOfInitializedData",
        )
    }

    fn set_size_of_image(&mut self, value: u32) -> Result<(), PeError> {
        write_u32(
            &mut self.bytes,
            self.opt_offset + OPT_SIZE_OF_IMAGE,
            value,
            "SizeOfImage",
        )
    }

    fn set_checksum(&mut self, value: u32) -> Result<(), PeError> {
        write_u32(
            &mut self.bytes,
            self.opt_offset + OPT_CHECKSUM,
            value,
            "CheckSum",
        )
    }
}

impl std::fmt::Debug for PeImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeImage")
            .field("size_bytes", &self.bytes.len())
            .field("pe_offset", &self.pe_offset)
            .field("coff_offset", &self.coff_offset)
            .field("opt_offset", &self.opt_offset)
            .field("size_of_optional_header", &self.size_of_optional_header)
            .field("sections", &self.sections)
            .finish()
    }
}

/// Offset of `e_lfanew`, the file offset of the "PE\0\0" signature, in the DOS header.
const OFF_E_LFANEW: usize = 0x3C;
/// Offset of `NumberOfSections`, relative to `coff_offset`.
const COFF_NUM_SECTIONS: usize = 2;
/// Offset of `SizeOfOptionalHeader`, relative to `coff_offset`.
const COFF_SIZE_OPT_HDR: usize = 16;
/// Offset of `Magic`, relative to `opt_offset`.
const OPT_MAGIC: usize = 0;
/// Offset of `SizeOfInitializedData`, relative to `opt_offset`.
const OPT_SIZE_OF_INIT_DATA: usize = 8;
/// Offset of `SectionAlignment`, relative to `opt_offset`.
const OPT_SECTION_ALIGNMENT: usize = 32;
/// Offset of `SizeOfImage`, relative to `opt_offset`.
const OPT_SIZE_OF_IMAGE: usize = 56;
/// Offset of `CheckSum`, relative to `opt_offset`.
const OPT_CHECKSUM: usize = 64;
/// Offset of `DataDirectory[0]`, relative to `opt_offset`.
const OPT_DATA_DIRECTORY: usize = 112;
/// Index of the Security Directory entry within the Data Directory table.
const SECURITY_DIRECTORY_INDEX: usize = 4;
/// Size in bytes of one section table entry.
const SECTION_ENTRY_SIZE: usize = 40;
/// Optional header `Magic` value identifying a PE32+, 64-bit, image.
const PE32_PLUS_MAGIC: u16 = 0x020B;

/// Reads a little-endian u16 out of `buf` at `offset`, bounds-checked.
fn read_u16(buf: &[u8], offset: usize, context: &'static str) -> Result<u16, PeError> {
    let end = offset.checked_add(2).context(FileTooLargeSnafu {
        context,
        max: buf.len().saturating_sub(2),
        have: buf.len(),
    })?;
    if end > buf.len() {
        return Err(PeError::TruncatedFile {
            needed: end,
            have: buf.len(),
            context,
        });
    }
    Ok(u16::from_le_bytes([buf[offset], buf[offset + 1]]))
}

/// Reads a little-endian u32 out of `buf` at `offset`, bounds-checked.
fn read_u32(buf: &[u8], offset: usize, context: &'static str) -> Result<u32, PeError> {
    let end = offset.checked_add(4).context(FileTooLargeSnafu {
        context,
        max: buf.len().saturating_sub(4),
        have: buf.len(),
    })?;
    if end > buf.len() {
        return Err(PeError::TruncatedFile {
            needed: end,
            have: buf.len(),
            context,
        });
    }
    Ok(u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]))
}

/// Writes a little-endian u16 into `buf` at `offset` in place, bounds-checked.
fn write_u16(
    buf: &mut [u8],
    offset: usize,
    value: u16,
    context: &'static str,
) -> Result<(), PeError> {
    let end = offset.checked_add(2).context(FileTooLargeSnafu {
        context,
        max: buf.len().saturating_sub(2),
        have: buf.len(),
    })?;
    if end > buf.len() {
        return Err(PeError::TruncatedFile {
            needed: end,
            have: buf.len(),
            context,
        });
    }
    buf[offset..end].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Writes a little-endian u32 into `buf` at `offset` in place, bounds-checked.
fn write_u32(
    buf: &mut [u8],
    offset: usize,
    value: u32,
    context: &'static str,
) -> Result<(), PeError> {
    let end = offset.checked_add(4).context(FileTooLargeSnafu {
        context,
        max: buf.len().saturating_sub(4),
        have: buf.len(),
    })?;
    if end > buf.len() {
        return Err(PeError::TruncatedFile {
            needed: end,
            have: buf.len(),
            context,
        });
    }
    buf[offset..end].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Decodes an 8-byte PE section name field into a `String`, stopping at the
/// first NUL.
fn decode_section_name(raw: &[u8; 8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(8);
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal synthetic PE32+ image in memory with the given
    /// section names/sizes, for testing header arithmetic without needing
    /// a real UKI on disk. Not a faithful PE, since it has no real code or
    /// data, just enough structure for the header/section-table logic under
    /// test.
    ///
    /// `section_specs` entries are `(name, virtual_size, virtual_address,
    /// size_of_raw_data, pointer_to_raw_data)`.
    fn build_synthetic_pe(section_specs: &[(&str, u32, u32, u32, u32)]) -> Vec<u8> {
        let num_sections = section_specs.len();
        let opt_header_size: u16 = 240;
        let e_lfanew: u32 = 0x80;
        let coff_offset = e_lfanew as usize + 4;
        let opt_offset = e_lfanew as usize + 24;
        let section_table_offset = opt_offset + opt_header_size as usize;
        let total_size = section_table_offset + num_sections * SECTION_ENTRY_SIZE + 0x2000;

        let mut buf = vec![0u8; total_size];
        buf[0] = b'M';
        buf[1] = b'Z';
        write_u32(&mut buf, OFF_E_LFANEW, e_lfanew, "test").unwrap();
        buf[e_lfanew as usize..e_lfanew as usize + 4].copy_from_slice(b"PE\0\0");
        write_u16(
            &mut buf,
            coff_offset + COFF_NUM_SECTIONS,
            num_sections as u16,
            "test",
        )
        .unwrap();
        write_u16(
            &mut buf,
            coff_offset + COFF_SIZE_OPT_HDR,
            opt_header_size,
            "test",
        )
        .unwrap();
        write_u16(&mut buf, opt_offset + OPT_MAGIC, PE32_PLUS_MAGIC, "test").unwrap();
        write_u32(&mut buf, opt_offset + OPT_SECTION_ALIGNMENT, 0x1000, "test").unwrap();
        write_u32(&mut buf, opt_offset + OPT_SIZE_OF_INIT_DATA, 0, "test").unwrap();
        write_u32(&mut buf, opt_offset + OPT_CHECKSUM, 0xdead_beef, "test").unwrap();

        let mut total_vsize: u64 = 0;
        for (i, (name, vsize, vaddr, rawsize, ptr)) in section_specs.iter().enumerate() {
            let off = section_table_offset + i * SECTION_ENTRY_SIZE;
            let name_bytes = name.as_bytes();
            buf[off..off + name_bytes.len().min(8)]
                .copy_from_slice(&name_bytes[..name_bytes.len().min(8)]);
            write_u32(&mut buf, off + 8, *vsize, "test").unwrap();
            write_u32(&mut buf, off + 12, *vaddr, "test").unwrap();
            write_u32(&mut buf, off + 16, *rawsize, "test").unwrap();
            write_u32(&mut buf, off + 20, *ptr, "test").unwrap();
            total_vsize += *vsize as u64;
        }
        write_u32(
            &mut buf,
            opt_offset + OPT_SIZE_OF_INIT_DATA,
            total_vsize as u32,
            "test",
        )
        .unwrap();

        buf
    }

    #[test]
    fn parses_synthetic_section_table() {
        // A basic three-section layout should parse into Sections in file order,
        // with each section's real PointerToRawData preserved.
        let bytes = build_synthetic_pe(&[
            (".text", 0x1000, 0x1000, 0x1000, 0x400),
            (".data", 0x800, 0x2000, 0x800, 0x1400),
            (".linux", 0x9000, 0x3000, 0x9000, 0x1c00),
        ]);
        let img = PeImage::parse(bytes).expect("parse should succeed");
        let names: Vec<&str> = img.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec![".text", ".data", ".linux"]);
        let linux_section = img
            .sections
            .iter()
            .find(|s| s.name == ".linux")
            .expect(".linux section should be present");
        assert_eq!(linux_section.pointer_to_raw_data, 0x1c00);
    }

    #[test]
    fn rejects_non_pe32_plus() {
        // Overwrite the Magic field with the plain PE32 value 0x010B and
        // confirm parsing rejects it instead of misreading it as PE32+.
        let mut bytes = build_synthetic_pe(&[(".text", 0x1000, 0x1000, 0x1000, 0x400)]);
        let e_lfanew = 0x80usize;
        let opt_offset = e_lfanew + 24;
        write_u16(&mut bytes, opt_offset + OPT_MAGIC, 0x010B, "test").unwrap();
        let err = PeImage::parse(bytes).unwrap_err();
        assert!(matches!(err, PeError::NotPe32Plus { magic: 0x010B }));
    }

    #[test]
    fn truncates_trailing_sections_and_patches_headers() {
        // Two payload sections, .text and .data, followed by the four UKI
        // trailing sections that derive-stub is expected to strip.
        let bytes = build_synthetic_pe(&[
            (".text", 0x1000, 0x1000, 0x1000, 0x400),
            (".data", 0x800, 0x2000, 0x800, 0x1400),
            (".osrel", 0x100, 0x3000, 0x200, 0x1c00),
            (".cmdline", 0x80, 0x3200, 0x200, 0x1e00),
            (".uname", 0x40, 0x3400, 0x200, 0x2000),
            (".linux", 0x9000, 0x3600, 0x9000, 0x2200),
        ]);
        let mut img = PeImage::parse(bytes).unwrap();
        let before_init_data = img.size_of_initialized_data().unwrap();

        img.derive_stub_by_truncating_trailing_sections(&[
            ".osrel", ".cmdline", ".uname", ".linux",
        ])
        .expect("truncation should succeed");

        let names: Vec<&str> = img.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec![".text", ".data"]);
        assert_eq!(img.number_of_sections().unwrap(), 2);
        // File is truncated at the first trailing section's raw data pointer.
        assert_eq!(img.bytes.len(), 0x1c00);

        // SizeOfInitializedData should drop by exactly the sum of the four
        // removed sections' VirtualSize.
        let removed_vsize = 0x100 + 0x80 + 0x40 + 0x9000;
        assert_eq!(
            img.size_of_initialized_data().unwrap(),
            before_init_data - removed_vsize
        );

        let size_of_image = read_u32(&img.bytes, img.opt_offset + OPT_SIZE_OF_IMAGE, "test");
        assert_eq!(size_of_image.unwrap(), 0x3000);
        let checksum = read_u32(&img.bytes, img.opt_offset + OPT_CHECKSUM, "test");
        assert_eq!(checksum.unwrap(), 0);
    }

    #[test]
    fn rejects_wrong_trailing_sections() {
        // Requesting removal of sections that don't match the actual tail
        // of the section table must fail rather than truncate blindly.
        let bytes = build_synthetic_pe(&[
            (".text", 0x1000, 0x1000, 0x1000, 0x400),
            (".linux", 0x9000, 0x2000, 0x9000, 0x1400),
        ]);
        let mut img = PeImage::parse(bytes).unwrap();
        let err = img
            .derive_stub_by_truncating_trailing_sections(&[".cmdline", ".uname", ".linux"])
            .unwrap_err();
        assert!(matches!(err, PeError::UnexpectedTrailingSections { .. }));
    }

    #[test]
    fn security_directory_roundtrip() {
        // Setting a Security Directory entry and then removing the
        // signature should truncate the file back to the certificate's
        // start and clear the directory entry.
        let bytes = build_synthetic_pe(&[(".text", 0x1000, 0x1000, 0x1000, 0x400)]);
        let mut img = PeImage::parse(bytes).unwrap();
        let cert_offset = img.bytes.len() as u32 - 0x400;
        img.set_security_directory(cert_offset, 0x400).unwrap();
        let (rva, size) = img.security_directory().unwrap();
        assert_eq!((rva, size), (cert_offset, 0x400));

        img.remove_signature().unwrap();
        let (rva2, size2) = img.security_directory().unwrap();
        assert_eq!((rva2, size2), (0, 0));
        assert_eq!(img.bytes.len(), cert_offset as usize);
    }

    #[test]
    fn read_reports_truncated_file_not_file_too_large() {
        // An offset that is in-bounds for `usize` but past the end of a
        // short buffer is a truncated file, not an overflowing offset.
        let buf = [0u8; 4];
        let err = read_u32(&buf, 2, "test").unwrap_err();
        assert!(matches!(
            err,
            PeError::TruncatedFile {
                needed: 6,
                have: 4,
                ..
            }
        ));
    }

    #[test]
    fn read_reports_file_too_large_on_offset_overflow() {
        // An offset within `checked_add`'s addend of `usize::MAX` cannot be
        // a valid location in any buffer that fits in memory, so this must
        // be reported as FileTooLarge rather than reusing TruncatedFile.
        let buf = [0u8; 4];
        let err = read_u32(&buf, usize::MAX - 1, "test").unwrap_err();
        assert!(matches!(
            err,
            PeError::FileTooLarge {
                max: 0,
                have: 4,
                ..
            }
        ));
        assert_eq!(
            err.to_string(),
            "Failed to access test: offset overflows usize (max valid offset 0), \
             file is 4 bytes"
        );
    }

    #[test]
    fn write_reports_file_too_large_on_offset_overflow() {
        let mut buf = [0u8; 4];
        let err = write_u16(&mut buf, usize::MAX, 0, "test").unwrap_err();
        assert!(matches!(
            err,
            PeError::FileTooLarge {
                max: 2,
                have: 4,
                ..
            }
        ));
    }
}
