use crate::crds::TestInput;
use crate::error::{self, Result};
use crate::run::KnownTestType;
use bottlerocket_types::agent_config::{SonobuoyConfig, SonobuoyMode};
use maplit::btreemap;
use model::Test;
use snafu::ResultExt;
use std::fmt::Display;

/// Create a Sonobuoy CRD for K8s conformance and quick testing.
pub(crate) fn sonobuoy_crd(test_input: TestInput) -> Result<Test> {
    let cluster_resource_name = test_input
        .cluster_crd_name
        .as_ref()
        .expect("A cluster name is required for migrations");
    let bottlerocket_resource_name = test_input
        .bottlerocket_crd_name
        .as_ref()
        .expect("A cluster name is required for migrations");
    let sonobuoy_mode = match test_input.test_type {
        KnownTestType::Conformance => SonobuoyMode::CertifiedConformance,
        KnownTestType::Quick | KnownTestType::Migration => SonobuoyMode::Quick,
    };

    let labels = test_input.crd_input.labels(btreemap! {
        "testsys/type".to_string() => test_input.test_type.to_string(),
        "testsys/cluster".to_string() => cluster_resource_name.to_string(),
    });

    SonobuoyConfig::builder()
        .resources(bottlerocket_resource_name)
        .resources(cluster_resource_name)
        .set_depends_on(Some(test_input.prev_tests))
        .set_retries(Some(5))
        .image(
            test_input
                .crd_input
                .images
                .sonobuoy_test_agent_image
                .to_owned()
                .expect("The default Sonobuoy testing image is missing"),
        )
        .set_image_pull_secret(
            test_input
                .crd_input
                .images
                .testsys_agent_pull_secret
                .to_owned(),
        )
        .keep_running(true)
        .kubeconfig_base64_template(cluster_resource_name, "encodedKubeconfig")
        .plugin("e2e")
        .mode(sonobuoy_mode)
        .e2e_repo_config_base64(
            test_input
                .crd_input
                .config
                .conformance_registry
                .to_owned()
                .map(e2e_repo_config_base64),
        )
        .kube_conformance_image(test_input.crd_input.config.conformance_image.to_owned())
        .assume_role(test_input.crd_input.config.agent_role.to_owned())
        .set_secrets(Some(test_input.crd_input.config.secrets.to_owned()))
        .set_labels(Some(labels))
        .build(format!(
            "{}{}",
            cluster_resource_name,
            test_input.name_suffix.unwrap_or("-test")
        ))
        .context(error::BuildSnafu {
            what: "Sonobuoy CRD",
        })
}

fn e2e_repo_config_base64<S>(e2e_registry: S) -> String
where
    S: Display,
{
    base64::encode(format!(
        r#"buildImageRegistry: {e2e_registry}
dockerGluster: {e2e_registry}
dockerLibraryRegistry: {e2e_registry}
e2eRegistry: {e2e_registry}
e2eVolumeRegistry: {e2e_registry}
gcRegistry: {e2e_registry}
gcEtcdRegistry: {e2e_registry}
promoterE2eRegistry: {e2e_registry}
sigStorageRegistry: {e2e_registry}"#
    ))
}
