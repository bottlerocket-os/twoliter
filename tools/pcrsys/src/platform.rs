//! Platform definitions for PCR prediction variants.

use clap::ValueEnum;
use std::fmt;

/// Target platform for PCR predictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Platform {
    /// AWS Nitro (EC2 instances)
    Aws,
    /// VMware vSphere
    Vmware,
    /// Bare metal servers
    Metal,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::Aws => write!(f, "aws"),
            Platform::Vmware => write!(f, "vmware"),
            Platform::Metal => write!(f, "metal"),
        }
    }
}
