/// Encode bootconfig text in the binary format the kernel reads.
pub fn encoded_bootconfig_data(bootconfig: &str) -> Vec<u8> {
    let mut data = bootconfig.as_bytes().to_vec();
    data.push(0);
    data.resize(data.len().next_multiple_of(4), 0);
    let size = u32::try_from(data.len()).expect("bootconfig fits in u32");
    let checksum = data.iter().map(|&byte| u32::from(byte)).sum::<u32>();
    data.extend(size.to_le_bytes());
    data.extend(checksum.to_le_bytes());
    data.extend(b"#BOOTCONFIG\n");
    data
}
