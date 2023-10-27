use assert_cmd::prelude::*;
use kubit::apply::{KUBECTL_APPLYSET_ENABLED, KUBECTL_IMAGE, KUBIT_APPLIER_FIELD_MANAGER};
use kubit::render::KUBECFG_IMAGE;
use std::path::PathBuf;
use std::process::Command;
use std::str::from_utf8;

const DEMO_PACKAGE: &str = "oci://ghcr.io/kubecfg/kubit/package-demo:v1";
const TEST_FILE: &str = "tests/fixtures/fake-package.yml";

#[tokio::test]
async fn local_apply_dry_run_script() {
    let mut cmd = Command::cargo_bin("kubit").unwrap();
    let output = cmd
        .args([
            "local",
            "apply",
            TEST_FILE,
            "--dry-run",
            "script",
            "--skip-auth",
        ])
        .unwrap();

    let vectorised_output = &output.stdout.to_vec();
    let output = from_utf8(vectorised_output).expect("unable to read output script");
    let overlay_file = PathBuf::from(
        std::fs::canonicalize(TEST_FILE)
            .expect("unable to find realpath for test")
            .file_name()
            .unwrap(),
    );

    // Assert some known required items in the output command.
    assert!(output.contains("docker"));
    assert!(output.contains(DEMO_PACKAGE));
    assert!(output.contains(KUBECTL_IMAGE));
    assert!(output.contains(KUBECFG_IMAGE));
    assert!(output.contains(KUBECTL_APPLYSET_ENABLED));
    assert!(output.contains(KUBIT_APPLIER_FIELD_MANAGER));
    assert!(output.contains("--server-side"));
    assert!(output.contains(&format!("appInstance_=/overlay/{}", overlay_file.display())));
}
