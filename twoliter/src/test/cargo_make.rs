use std::collections::BTreeMap;

use semver::Version;
use serde::Deserialize;

use crate::project::ValidIdentifier;
use crate::{cargo_make::CargoMake, project::Project, test::data_dir};

#[tokio::test]
async fn test_cargo_make() {
    let path = data_dir().join("Twoliter-1.toml");
    let project = Project::load(path).await.unwrap();
    let version = Version::new(1, 2, 3);
    let vendor_id = ValidIdentifier("my-vendor".into());
    let registry = "a.com/b";
    let source = format!("{}/{}:v{}", registry, "my-bottlerocket-sdk", "1.2.3");

    let cargo_make = CargoMake::new(&source)
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
