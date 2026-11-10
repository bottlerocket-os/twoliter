//! Normalization of PE binaries, so that ones differing only in signatures and chosen sections
//! compare equal.

use crate::error::Result;
use object::read::pe::PeFile64;
use object::{pe, LittleEndian as LE};
use snafu::OptionExt;

/// `pe_data` without its checksum or certificate table, and with the contents and virtual sizes
/// of the sections at `cleared` (indices in file order, as `pcrsys::pe::PeImage::sections`
/// lists them) zeroed, so that binaries differing only in their signatures and those sections
/// are equal.
pub fn normalize(pe_data: &[u8], cleared: &[usize]) -> Result<Vec<u8>> {
    let pe = PeFile64::parse(pe_data)
        .ok()
        .whatever_context("failed to parse PE32+ file")?;
    // The parsed headers borrow `pe_data`, so their offsets in it are where they point.
    let offset = |field: *const u8| field as usize - pe_data.as_ptr() as usize;
    let mut out = pe_data.to_vec();
    let mut clear = |at: usize, len: usize| -> Result<()> {
        out.get_mut(at..at.saturating_add(len))
            .whatever_context("PE section beyond the end of the file")?
            .fill(0);
        Ok(())
    };
    let check_sum = &pe.nt_headers().optional_header.check_sum;
    clear(offset(std::ptr::from_ref(check_sum).cast()), 4)?;
    let certificates = pe
        .data_directory(pe::IMAGE_DIRECTORY_ENTRY_SECURITY)
        .map(|dir| {
            let range = (dir.virtual_address.get(LE), dir.size.get(LE));
            (offset(std::ptr::from_ref(dir).cast()), range)
        });
    if let Some((entry, _)) = certificates {
        clear(entry, 8)?;
    }
    for (i, header) in pe.section_table().iter().enumerate() {
        if cleared.contains(&i) {
            clear(offset(std::ptr::from_ref(&header.virtual_size).cast()), 4)?;
            clear(
                header.pointer_to_raw_data.get(LE) as usize,
                header.size_of_raw_data.get(LE) as usize,
            )?;
        }
    }
    // Signatures differ in length, so remove the certificate table, which is at a file offset.
    if let Some((_, (start, len))) = certificates.filter(|(_, (_, len))| *len > 0) {
        let (start, len) = (start as usize, len as usize);
        let end = start
            .checked_add(len)
            .filter(|end| *end <= out.len())
            .whatever_context("certificate table beyond the end of the file")?;
        out.drain(start..end);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsfiles::tests::uki;

    #[test]
    fn test_normalize() {
        let pe = uki(b"kernel", b"cmdline");
        let normalized = normalize(&pe, &[1]).unwrap();
        assert_eq!(&normalized[0x600..0xa00], &[0; 0x400]); // .cmdline
        assert_eq!(&normalized[0x178..0x17c], &[0; 4]); // its VirtualSize
        assert_eq!(&normalized[..0x178], &pe[..0x178]);
        assert_eq!(&normalized[0x17c..0x600], &pe[0x17c..0x600]);

        // A signature is removed along with its directory entry and the checksum.
        let mut signed = pe.clone();
        signed[0x98..0x9c].copy_from_slice(&7u32.to_le_bytes()); // CheckSum
        let len = signed.len() as u32;
        signed[0xe8..0xec].copy_from_slice(&len.to_le_bytes()); // certificate table offset
        signed[0xec..0xf0].copy_from_slice(&16u32.to_le_bytes()); // and size
        signed.extend([7u8; 16]);
        assert_eq!(
            normalize(&signed, &[]).unwrap(),
            normalize(&pe, &[]).unwrap()
        );
    }
}
