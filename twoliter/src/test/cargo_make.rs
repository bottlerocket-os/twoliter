use crate::{cargo_make::CargoMake, project::Project, test::data_dir};

#[tokio::test]
async fn test_cargo_make() {
    let path = data_dir().join("Twoliter-1.toml");
    let project = Project::load(path).await.unwrap();
    let cargo_make = CargoMake::new(&project, "arch")
        .unwrap()
        .makefile(data_dir().join("Makefile.toml"));
    cargo_make._exec("verify-twoliter-env").await.unwrap();
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
    cargo_make
        .clone()
        ._arg("--env")
        ._arg("FOO=bar")
        .exec_with_args("verify-env-value-with-arg", ["FOO", "bar"])
        .await
        .unwrap();
    cargo_make
        .clone()
        ._args(["--env", "FOO=bar"])
        .exec_with_args("verify-env-value-with-arg", ["FOO", "bar"])
        .await
        .unwrap();
    cargo_make
        .clone()
        ._envs([("FOO", "bar"), ("BAR", "baz")])
        .exec_with_args("verify-env-value-with-arg", ["BAR", "baz"])
        .await
        .unwrap();
}
