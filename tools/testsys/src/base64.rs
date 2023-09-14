use base64::alphabet::STANDARD;
use base64::engine::{GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;

/// This function became deprecated in the base64 library but its interface is much simpler than
/// what replaced it. Rather than change all of our call sites we retain the simple interface here.
pub(crate) fn encode<T: AsRef<[u8]>>(input: T) -> String {
    GeneralPurpose::new(&STANDARD, GeneralPurposeConfig::default()).encode(input)
}
