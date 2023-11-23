use assert_cmd::prelude::*;
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::api::{apps::v1::StatefulSet, core::v1::Service};
use kube::{api::ListParams, client, Api};
use kubit::delete::cleanup_hack_resource_name;
use kubit::resources::AppInstance;
use std::fs::File;
use std::process::Command;

const TEST_FILE: &str = "tests/fixtures/fake-package.yml";

#[tokio::test]
async fn local_cleanup() {
    // Apply the AppInstance package
    let mut setup_cmd = Command::cargo_bin("kubit").unwrap();
    let _ = setup_cmd
        .args(["local", "apply", TEST_FILE, "--skip-auth"])
        .unwrap();

    // Prune the applied resources.
    let mut cmd = Command::cargo_bin("kubit").unwrap();
    let _ = cmd.args(["local", "delete", TEST_FILE]).unwrap();

    let file = File::open(TEST_FILE).unwrap();
    let app_instance: AppInstance = serde_yaml::from_reader(file).unwrap();
    let client = client::Client::try_default()
        .await
        .expect("Unable to create default kubernetes client");

    let namespace = &app_instance.namespace_any();

    let sts_api: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let svc_api: Api<Service> = Api::namespaced(client.clone(), namespace);
    let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    let cleanup_cm_name = cleanup_hack_resource_name(&app_instance);
    let list_params = ListParams::default();

    let sts = sts_api
        .list(&list_params)
        .await
        .expect("Unable to list StatefulSets");
    let svc = svc_api
        .list(&list_params)
        .await
        .expect("Unable to list Services");
    let cleanup_cm = cm_api
        .get_opt(&cleanup_cm_name)
        .await
        .expect("Unable to get {cleanup_cm_name} ConfigMap");

    assert_eq!(
        sts.items.len(),
        0,
        "StatefulSets should have been pruned, expected 0 but got {}",
        sts.items.len()
    );
    assert_eq!(
        svc.items.len(),
        0,
        "Services should have been pruned, expected 0 but got {}",
        svc.items.len()
    );
    assert_eq!(
        cleanup_cm, None,
        "ConfigMap for cleanup should not exist but was found!"
    );
}
