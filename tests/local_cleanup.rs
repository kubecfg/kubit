use assert_cmd::prelude::*;
use k8s_openapi::api::{apps::v1::StatefulSet, core::v1::Service};
use kube::{api::ListParams, client, Api};
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
    let _ = cmd.args(["local", "cleanup", TEST_FILE]).unwrap();

    let file = File::open(TEST_FILE).unwrap();
    let app_instance: AppInstance = serde_yaml::from_reader(file).unwrap();
    let client = client::Client::try_default()
        .await
        .expect("Unable to create default kubernetes client");

    let sts_api: Api<StatefulSet> = Api::namespaced(client.clone(), &app_instance.namespace_any());
    let svc_api: Api<Service> = Api::namespaced(client.clone(), &app_instance.namespace_any());
    let list_params = ListParams::default();

    let sts = sts_api
        .list(&list_params)
        .await
        .expect("Unable to list StatefulSets");
    let svc = svc_api
        .list(&list_params)
        .await
        .expect("Unable to list Services");

    // Items were pruned using a blank applyset, there should be 0 returned from
    // the Kubernetes API server.
    assert_eq!(sts.items.len(), 0);
    assert_eq!(svc.items.len(), 0);
}
