use semver::Version;
use serde::Deserialize;

use crate::lock::{Lock, LockedImage};
use crate::project::ValidIdentifier;
use crate::{cargo_make::CargoMake, project::Project, test::data_dir};

#[tokio::test]
async fn test_cargo_make() {
    let path = data_dir().join("Twoliter-1.toml");
    let project = Project::load(path).await.unwrap();
    let version = Version::new(1, 2, 3);
    let vendor_id = ValidIdentifier("my-vendor".into());
    let vendor = project.vendor().get(&vendor_id).unwrap();
    let lock = Lock {
        schema_version: project.schema_version(),
        release_version: project.release_version().to_string(),
        digest: project.digest().unwrap(),
        kit: Vec::new(),
        sdk: LockedImage {
            name: "my-bottlerocket-sdk".to_string(),
            version: version,
            vendor: "my-vendor".to_string(),
            source: format!("{}/{}:v{}", vendor.registry, "my-bottlerocket-sdk", "1.2.3"),
            digest: "abc".to_string(),
            manifest: Vec::new(),
        },
    };
    let cargo_make = CargoMake::new(&lock.sdk.source)
        .unwrap()
        .makefile(data_dir().join("Makefile.toml"));
    cargo_make.exec("verify-twoliter-env").await.unwrap();
    cargo_make
        .clone()
        .env("FOO", "bar")
        .exec_with_args("verify-env-set-with-arg", ["FOO"])
        .await
        .unwrap();
    cargo_make
        .clone()
        .env("FOO", "bar")
        .exec_with_args("verify-env-value-with-arg", ["FOO", "bar"])
        .await
        .unwrap();
    cargo_make
        .clone()
        .project_dir(data_dir())
        .exec_with_args(
            "verify-current-dir-with-arg",
            [data_dir().display().to_string()],
        )
        .await
        .unwrap();
}
