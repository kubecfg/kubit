use crate::{metadata, resources::AppInstance, scripting::Script, Error, Result};
use home::home_dir;
use std::env;

/// GitHub Registry which contains the `kubecfg` image.
pub const DEFAULT_KUBECFG_IMAGE: &str = "ghcr.io/kubecfg/kubecfg/kubecfg";

/// Generates shell script that will render the manifest and writes it to writer.
pub async fn emit_script<W>(
    app_instance: &AppInstance,
    docker: bool,
    skip_auth: bool,
    w: &mut W,
) -> Result<()>
where
    W: std::io::Write,
{
    let tmp = tempfile::Builder::new().suffix(".json").tempfile()?;
    let (mut file, path) = tmp.keep()?;
    serde_json::to_writer(&mut file, &app_instance).map_err(Error::RenderOverlay)?;

    let script = script(
        app_instance,
        &path.to_string_lossy(),
        Some("/tmp/manifests"),
        docker,
        skip_auth,
    )
    .await?;
    writeln!(w, "{script}")?;
    Ok(())
}

/// Generates shell script that will render the manifest
pub async fn script(
    app_instance: &AppInstance,
    overlay_file_name: &str,
    output_dir: Option<&str>,
    docker: bool,
    skip_auth: bool,
) -> Result<Script> {
    let tokens = emit_commandline(
        app_instance,
        overlay_file_name,
        output_dir,
        docker,
        skip_auth,
    )
    .await;
    Ok(Script::from_vec(tokens))
}

pub async fn emit_commandline(
    app_instance: &AppInstance,
    overlay_file: &str,
    output_dir: Option<&str>,
    docker: bool,
    skip_auth: bool,
) -> Vec<String> {
    let image = &app_instance.spec.package.image;

    let entrypoint = if image.starts_with("file://") {
        image.clone()
    } else {
        format!("oci://{image}")
    };

    let mut cli: Vec<String> = vec![];

    if docker {
        let overlay_path = std::fs::canonicalize(overlay_file).unwrap();
        let overlay_file_name = std::path::PathBuf::from(overlay_path.file_name().unwrap());
        let user_home = home_dir().expect("unable to retrieve home directory");
        let docker_config =
            env::var("DOCKER_CONFIG").unwrap_or(format!("{}/.docker", user_home.display()));
        let kube_config =
            env::var("KUBECONFIG").unwrap_or(format!("{}/.kube/config", user_home.display()));
        let package_config = metadata::fetch_package_config_local_auth(app_instance, skip_auth)
            .await
            .unwrap();
        let kubecfg_image = package_config
            .versioned_kubecfg_image(DEFAULT_KUBECFG_IMAGE)
            .expect("unable to parse kubecfg image");

        cli.extend(
            [
                "docker",
                "run",
                "--rm",
                "--network",
                "host",
                "-v",
                &format!("{}:/.kube/config", kube_config),
                "-v",
                &format!(
                    "{}:/overlay/{}",
                    overlay_path.display(),
                    overlay_file_name.display()
                ),
                "--env",
                "KUBECONFIG=/.kube/config",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        );

        // Whenever we are not skipping authentication, we should always mount
        // docker credentials in order to pull image manifests.
        if !skip_auth {
            cli.extend(
                [
                    "-v",
                    &format!("{}:/.docker", docker_config),
                    // DOCKER_CONFIG within the container
                    "--env",
                    "DOCKER_CONFIG=/.docker",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            );
        }

        // The image should always be the final item in the "docker run" section
        // in order for the proceeding arguments to be parsed correctly.
        cli.extend(
            [&kubecfg_image]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
    } else {
        cli = ["kubecfg"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
    }

    cli.extend(
        ["show", &entrypoint, "--alpha", "--reorder=server"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    );

    // Running as `kubit local apply` requires a different overlay path,
    // as the file is mounted to the container.
    if docker {
        let overlay_path = std::fs::canonicalize(overlay_file).unwrap();
        let overlay_file_name = std::path::PathBuf::from(overlay_path.file_name().unwrap());
        cli.extend(
            [
                "--overlay-code-file",
                &format!("appInstance_=/overlay/{}", overlay_file_name.display()),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        );
    } else {
        cli.extend(
            [
                "--overlay-code-file",
                &format!("appInstance_={}", overlay_file),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        );
    }

    if let Some(output_dir) = output_dir {
        const FORMAT: &str = "{{printf \"%03d\" (resourceIndex .)}}-{{.apiVersion}}.{{.kind}}-{{default \"default\" .metadata.namespace}}.{{.metadata.name}}";
        let out = [
            "--export-dir",
            output_dir,
            "--export-filename-format",
            FORMAT,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<String>>();
        cli.extend(out);
    }
    cli
}

pub fn emit_fetch_app_instance_commandline(ns: &str, name: &str, output_file: &str) -> Vec<String> {
    [
        "kubit",
        "helper",
        "fetch-app-instance",
        "--namespace",
        ns,
        "--output",
        output_file,
        name,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>()
}

pub fn emit_fetch_appinstance_from_config_map_commandline(
    ns: &str,
    name: &str,
    output_file: &str,
) -> Vec<String> {
    [
        "kubit",
        "helper",
        "fetch-app-instance-from-config-map",
        "--namespace",
        ns,
        "--output",
        output_file,
        name,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {

    use super::*;
    const TEST_PACKAGE_FILE: &str = "tests/fixtures/fake-package.yml";

    fn arrange_app_instance() -> AppInstance {
        let example_file = std::fs::File::open(TEST_PACKAGE_FILE)
            .unwrap_or_else(|_| panic!("unable to open {}", TEST_PACKAGE_FILE));
        let app_instance: AppInstance = serde_yaml::from_reader(example_file)
            .unwrap_or_else(|_| panic!("unable to serialize {} to AppInstance", TEST_PACKAGE_FILE));
        app_instance
    }

    #[tokio::test]
    async fn render_emit_commandline() {
        let app_instance = arrange_app_instance();
        let docker = false;
        let skip_auth = false;

        let test_overlay_file = &format!("appInstance_={}", TEST_PACKAGE_FILE);
        let expected = vec![
            "kubecfg",
            "show",
            "oci://ghcr.io/kubecfg/kubit/package-demo:v1",
            "--alpha",
            "--reorder=server",
            "--overlay-code-file",
            test_overlay_file,
        ];

        let output =
            emit_commandline(&app_instance, TEST_PACKAGE_FILE, None, docker, skip_auth).await;

        assert_eq!(output, expected);
    }
}
