use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Represents a docker image URI such as `public.ecr.aws/myregistry/myrepo:v0.1.0`. The registry is
/// optional as it is when using `docker`. That is, it will be looked for locally first, then at
/// `dockerhub.io` when the registry is absent.
#[derive(Debug, Default, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
pub(crate) struct ImageUri {
    /// e.g. public.ecr.aws/bottlerocket
    pub(crate) registry: Option<String>,
    /// e.g. my-repo
    pub(crate) repo: String,
    /// e.g. v0.31.0
    pub(crate) tag: String,
}

impl ImageUri {
    /// Create a new `ImageUri`.
    #[allow(unused)]
    pub(crate) fn new<S1, S2>(registry: Option<String>, repo: S1, tag: S2) -> Self
    where
        S1: AsRef<str>,
        S2: AsRef<str>,
    {
        Self {
            registry,
            repo: repo.as_ref().into(),
            tag: tag.as_ref().into(),
        }
    }

    /// Returns the `ImageUri` for use with docker, e.g. `public.ecr.aws/myregistry/myrepo:v0.1.0`
    pub(crate) fn uri(&self) -> String {
        match &self.registry {
            None => format!("{}:{}", self.repo, self.tag),
            Some(registry) => format!("{}/{}:{}", registry, self.repo, self.tag),
        }
    }
}

impl Display for ImageUri {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.uri(), f)
    }
}

impl From<ImageUri> for String {
    fn from(value: ImageUri) -> Self {
        value.to_string()
    }
}

#[test]
fn image_uri_no_registry() {
    let uri = ImageUri::new(None, "foo", "v1.2.3");
    let formatted = uri.uri();
    let expected = "foo:v1.2.3";
    assert_eq!(expected, formatted);
}

#[test]
fn image_uri_with_registry() {
    let uri = ImageUri::new(Some("example.com/a/b/c".to_string()), "foo", "v1.2.3");
    let formatted = uri.uri();
    let expected = "example.com/a/b/c/foo:v1.2.3";
    assert_eq!(expected, formatted);
}
