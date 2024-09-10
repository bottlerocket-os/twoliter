//! Utilities for interacting with Twoliter vendors.
//!
//! Most users of this module will need [`ArtifactVendor`], which represents a vendor which may have
//! been overridden in a `Twoliter.override` file.
use super::{Override, ValidIdentifier, VendedArtifact, Vendor};
use crate::docker::ImageUri;
use std::fmt::Debug;

/// `ArtifactVendor` represents a vendor associated with an image artifact used in a project.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum ArtifactVendor {
    /// The project only knows of the given vendor as it is written in Twoliter.toml
    Verbatim(VerbatimVendor),
    /// The project has an override expressed in Twoliter.override
    Overridden(OverriddenVendor),
}

impl ArtifactVendor {
    pub(crate) fn registry(&self) -> &str {
        match self {
            ArtifactVendor::Verbatim(vendor) => vendor.registry(),
            ArtifactVendor::Overridden(vendor) => vendor.registry(),
        }
    }

    pub(crate) fn repo_for<'a, V: VendedArtifact>(&'a self, image: &'a V) -> &'a str {
        match self {
            ArtifactVendor::Verbatim(vendor) => vendor.repo_for(image),
            ArtifactVendor::Overridden(vendor) => vendor.repo_for(image),
        }
    }

    pub(crate) fn image_uri_for<V: VendedArtifact>(&self, image: &V) -> ImageUri {
        ImageUri {
            registry: Some(self.registry().to_string()),
            repo: self.repo_for(image).to_string(),
            tag: format!("v{}", image.version()),
        }
    }

    pub(crate) fn vendor_name(&self) -> &ValidIdentifier {
        match self {
            ArtifactVendor::Verbatim(vendor) => &vendor.vendor_name,
            ArtifactVendor::Overridden(vendor) => &vendor.original_vendor_name,
        }
    }

    pub(crate) fn overridden(
        original_vendor_name: ValidIdentifier,
        original_vendor: Vendor,
        override_: Override,
    ) -> Self {
        Self::Overridden(OverriddenVendor {
            original_vendor_name,
            original_vendor,
            override_,
        })
    }

    pub(crate) fn verbatim(vendor_name: ValidIdentifier, vendor: Vendor) -> Self {
        Self::Verbatim(VerbatimVendor {
            vendor_name,
            vendor,
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct VerbatimVendor {
    vendor_name: ValidIdentifier,
    vendor: Vendor,
}

impl VerbatimVendor {
    /// The name of the vendor as it appears in the Twoliter.toml file
    pub(crate) fn registry(&self) -> &str {
        &self.vendor.registry
    }

    pub(crate) fn repo_for<'a, V: VendedArtifact>(&'a self, image: &'a V) -> &'a str {
        image.artifact_name().as_ref()
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct OverriddenVendor {
    original_vendor_name: ValidIdentifier,
    original_vendor: Vendor,
    override_: Override,
}

impl OverriddenVendor {
    /// The name of the vendor as it appears in the Twoliter.toml file
    pub(crate) fn registry(&self) -> &str {
        self.override_
            .registry
            .as_ref()
            .unwrap_or(&self.original_vendor.registry)
    }

    pub(crate) fn repo_for<'a, V: VendedArtifact>(&'a self, image: &'a V) -> &str {
        self.override_
            .name
            .as_deref()
            .unwrap_or(image.artifact_name().as_ref())
    }

    pub(crate) fn original_vendor(&self) -> VerbatimVendor {
        VerbatimVendor {
            vendor_name: self.original_vendor_name.clone(),
            vendor: self.original_vendor.clone(),
        }
    }
}
