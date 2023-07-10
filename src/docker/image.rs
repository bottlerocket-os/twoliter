/// Represents a docker image URI such as `public.ecr.aws/myregistry/myrepo:v0.1.0`. The registry is
/// optional as it is when using `docker`. That is, it will be looked for locally first, then at
/// `dockerhub.io` when the registry is absent.
#[derive(Debug, Default, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct ImageUri {
    /// e.g. public.ecr.aws/bottlerocket
    registry: Option<String>,
    /// e.g. my-repo
    repo: String,
    /// e.g. v0.31.0
    tag: String,
}

impl ImageUri {
    /// Create a new `ImageUri`.
    pub(crate) fn new<S1, S2>(registry: Option<String>, repo: S1, tag: S2) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        Self {
            registry,
            repo: repo.into(),
            tag: tag.into(),
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

/// Represents a container URI that is specialized for a target compilation architecture. For
/// example: `public.ecr.aws/bottlerocket/bottlerocket-sdk:v0.1.0`. The registry is
/// optional as it is when using `docker`. That is, it will be looked for locally first, then at
/// `dockerhub.io` when the registry is absent. The `name` is automatically suffixed with the target
/// architecture when creating a docker image URI.
#[derive(Debug, Default, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct ImageArchUri {
    /// e.g. public.ecr.aws/bottlerocket
    registry: Option<String>,
    /// e.g. bottlerocket-sdk
    name: String,
    /// e.g. x86_64
    arch: String,
    /// e.g. v0.31.0
    tag: String,
}

impl ImageArchUri {
    /// Create a new `ImageArchUri`.
    pub(crate) fn new<S1, S2, S3>(registry: Option<String>, name: S1, arch: S2, tag: S3) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
        S3: Into<String>,
    {
        Self {
            registry,
            name: name.into(),
            arch: arch.into(),
            tag: tag.into(),
        }
    }

    /// Returns the `ImageArchUri` for use with docker, e.g.
    /// `public.ecr.aws/bottlerocket/bottlerocket-sdk-x86_64:v0.1.0`
    pub(crate) fn uri(&self) -> String {
        match &self.registry {
            None => format!("{}-{}:{}", self.name, self.arch, self.tag),
            Some(registry) => format!("{}/{}-{}:{}", registry, self.name, self.arch, self.tag),
        }
    }
}

#[test]
fn image_arch_uri_no_registry() {
    let uri = ImageArchUri::new(None, "my-sdk", "i386", "v0.33.1");
    let formatted = uri.uri();
    let expected = "my-sdk-i386:v0.33.1";
    assert_eq!(expected, formatted);
}

#[test]
fn image_arch_uri_with_registry() {
    let uri = ImageArchUri::new(
        Some("example.com/a/b/c".to_string()),
        "my-sdk",
        "i386",
        "v0.33.1",
    );
    let formatted = uri.uri();
    let expected = "example.com/a/b/c/my-sdk-i386:v0.33.1";
    assert_eq!(expected, formatted);
}
