use crate::{resources::AppInstance, scripting::Script, Result};
use home::home_dir;
use kube::ResourceExt;
use std::env;

pub const KUBIT_APPLIER_FIELD_MANAGER: &str = "kubit-applier";
/// Image used within the "apply" step of kubit
pub const DEFAULT_APPLY_KUBECTL_IMAGE: &str = "bitnami/kubectl:1.27.5";
pub const KUBECTL_APPLYSET_ENABLED: &str = "KUBECTL_APPLYSET=true";

/// Generates shell script that will apply the manifests and writes it to w
pub fn emit_script<W>(
    app_instance: &AppInstance,
    docker: bool,
    kubectl_image: &str,
    w: &mut W,
) -> Result<()>
where
    W: std::io::Write,
{
    let script = script(app_instance, "/tmp/manifests", &None, docker, kubectl_image)?;
    write!(w, "{script}")?;
    Ok(())
}

/// Generates shell script that will apply the manifests
pub fn script(
    app_instance: &AppInstance,
    manifests_dir: &str,
    impersonate_user: &Option<String>,
    docker: bool,
    kubectl_image: &str,
) -> Result<Script> {
    let tokens = emit_commandline(
        app_instance,
        manifests_dir,
        impersonate_user,
        docker,
        kubectl_image,
    );
    Ok(Script::from_vec(tokens))
}

pub fn emit_commandline(
    app_instance: &AppInstance,
    manifests_dir: &str,
    impersonate_user: &Option<String>,
    docker: bool,
    kubectl_image: &str,
) -> Vec<String> {
    let mut cli: Vec<String> = vec![];

    // TODO: shared with `render.rs`, refactor when functionality is correct.
    let user_home = home_dir().expect("unable to retrieve home directory");
    let kube_config =
        env::var("KUBECONFIG").unwrap_or(format!("{}/.kube/config", user_home.display()));

    if docker {
        cli.extend(
            [
                "docker",
                "run",
                "--interactive",
                "--rm",
                "--network",
                "host",
                "-v",
                &format!("{}:/.kube/config", kube_config),
                "--env",
                KUBECTL_APPLYSET_ENABLED,
                "--env",
                "KUBECONFIG=/.kube/config",
                kubectl_image,
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        );
    } else {
        cli.extend(
            ["kubectl"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
    }

    cli.extend(
        [
            "apply",
            "-n",
            &app_instance.namespace_any(),
            "--server-side",
            "--prune",
            "--applyset",
            &app_instance.name_any(),
            "--field-manager",
            KUBIT_APPLIER_FIELD_MANAGER,
            "--force-conflicts",
            "-v=2",
            "-f",
            manifests_dir,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
    );

    if let Some(as_user) = impersonate_user {
        cli.push(format!("--as={as_user}"));
    }

    cli
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;

    const TEST_PACKAGE_FILE: &str = "tests/fixtures/fake-package.yml";

    fn arrange_app_instance() -> AppInstance {
        let example_file = std::fs::File::open(TEST_PACKAGE_FILE)
            .unwrap_or_else(|_| panic!("unable to open {}", TEST_PACKAGE_FILE));
        let app_instance: AppInstance = serde_yaml::from_reader(example_file)
            .unwrap_or_else(|_| panic!("unable to serialize {} to AppInstance", TEST_PACKAGE_FILE));
        app_instance
    }

    #[test]
    fn apply_emit_commandline() {
        let app_instance = arrange_app_instance();
        let docker = false;
        let fake_manifest_dir = "/tmp/test";

        let expected = vec![
            "kubectl",
            "apply",
            "-n",
            "test",
            "--server-side",
            "--prune",
            "--applyset",
            "test",
            "--field-manager",
            KUBIT_APPLIER_FIELD_MANAGER,
            "--force-conflicts",
            "-v=2",
            "-f",
            fake_manifest_dir,
        ];

        let output = emit_commandline(
            &app_instance,
            fake_manifest_dir,
            &None,
            docker,
            DEFAULT_APPLY_KUBECTL_IMAGE,
        );

        assert_eq!(output, expected);
    }
}
