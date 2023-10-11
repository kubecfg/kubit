use crate::{metadata, resources::AppInstance, scripting::Script, Error, Result};
use home::home_dir;
use std::env;

/// GitHub Registry which contains the `kubecfg` image.
pub const KUBECFG_IMAGE: &str = "ghcr.io/kubecfg/kubecfg/kubecfg";

/// Generates shell script that will render the manifest and writes it to writer.
pub async fn emit_script<W>(
    app_instance: &AppInstance,
    is_local: bool,
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
        is_local,
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
    is_local: bool,
    skip_auth: bool,
) -> Result<Script> {
    let tokens = emit_commandline(
        app_instance,
        overlay_file_name,
        output_dir,
        is_local,
        skip_auth,
    )
    .await;
    Ok(Script::from_vec(tokens))
}

pub async fn emit_commandline(
    app_instance: &AppInstance,
    overlay_file: &str,
    output_dir: Option<&str>,
    is_local: bool,
    skip_auth: bool,
) -> Vec<String> {
    let image = &app_instance.spec.package.image;

    let entrypoint = if image.starts_with("file://") {
        image.clone()
    } else {
        format!("oci://{image}")
    };

    let mut cli: Vec<String> = vec![];

    if is_local {
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
            .versioned_kubecfg_image(KUBECFG_IMAGE)
            .expect("unable to parse kubecfg image");

        cli.extend(
            [
                "docker",
                "run",
                "--rm",
                "-v",
                &format!("{}:/.kube/config", kube_config),
                "-v",
                &format!(
                    "{}:/overlay/{}",
                    overlay_path.display(),
                    overlay_file_name.display()
                ),
                "-v",
                &format!("{}:/.docker", docker_config),
                // DOCKER_CONFIG within the container
                "--env",
                "DOCKER_CONFIG=/.docker",
                "--env",
                "KUBECONFIG=/.kube/config",
                &kubecfg_image,
            ]
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
    if is_local {
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

#[cfg(test)]
mod tests {

    use super::*;
    use serde_json::json;
    use std::fs::File;
    use tempdir::TempDir;

    const TEST_PACKAGE_FILE: &str = "tests/fixtures/fake-package.yml";
    const TEST_HOME_ENV: &str = "/fake/home/test";

    fn arrange_app_instance() -> AppInstance {
        let example_file = std::fs::File::open(TEST_PACKAGE_FILE)
            .unwrap_or_else(|_| panic!("unable to open {}", TEST_PACKAGE_FILE));
        let app_instance: AppInstance = serde_yaml::from_reader(example_file)
            .unwrap_or_else(|_| panic!("unable to serialize {} to AppInstance", TEST_PACKAGE_FILE));
        app_instance
    }

    #[tokio::test]
    async fn render_emit_commandline_when_not_local() {
        let app_instance = arrange_app_instance();
        let is_local = false;
        let skip_auth = false;

        let test_overlay_file = &format!("appInstance_={}", TEST_PACKAGE_FILE);
        let expected = vec![
            "kubecfg",
            "show",
            "oci://gcr.io/mkm-cloud/package-demo:v1",
            "--alpha",
            "--reorder=server",
            "--overlay-code-file",
            test_overlay_file,
        ];

        let output =
            emit_commandline(&app_instance, TEST_PACKAGE_FILE, None, is_local, skip_auth).await;

        assert_eq!(output, expected);
    }

    #[tokio::test]
    async fn render_emit_commandline_when_local() {
        let app_instance = arrange_app_instance();
        let is_local = true;
        // Ensure that we don't need to interact with "credHelpers" or registry auth for
        // a fake package.
        let skip_auth = true;
        let local_dir_prefix = "render_emit_local";

        let temp_dir =
            TempDir::new(local_dir_prefix).expect("unable to create temporary dir for test");
        let fake_config_path = temp_dir.path().join("config.json");
        let fake_config =
            File::create(&fake_config_path).expect("unable to create fake_config for test");

        // Arranging fake credentials for our test, 'something' needs to exist for the
        // docker_credentials crate to read; however, they do not have to be valid.
        let fake_credentials = json!({"auths": {"gcr.io": {"auth": "ZmFrZTp0ZXN0Cg=="}}});
        let _ = serde_json::to_writer(&fake_config, &fake_credentials);

        let test_kubeconfig_path = &format!("{}/.kube/config", TEST_HOME_ENV);
        let test_mounted_kubeconfig_path = &format!("{}:/.kube/config", test_kubeconfig_path);
        let test_mounted_overlay_path = &format!(
            "{}/{}:/overlay/fake-package.yml",
            std::env::current_dir()
                .expect("unable to get current dir")
                .display(),
            TEST_PACKAGE_FILE
        );
        let test_mounted_docker_path = &format!("{}:/.docker", temp_dir.path().display());

        // Set reliant environment variables for the currently running process before executing.
        env::set_var("KUBECONFIG", test_kubeconfig_path);
        env::set_var("DOCKER_CONFIG", temp_dir.path());

        // Rendering relies on a specific package version of `kubecfg`, we must
        // retrieve that version here to ensure that our output is correct.
        let package_config = metadata::fetch_package_config_local_auth(&app_instance, skip_auth)
            .await
            .unwrap();
        let kubecfg_image = package_config
            .versioned_kubecfg_image(KUBECFG_IMAGE)
            .expect("unable to parse kubecfg image");

        let expected = vec![
            "docker",
            "run",
            "--rm",
            "-v",
            test_mounted_kubeconfig_path,
            "-v",
            test_mounted_overlay_path,
            "-v",
            test_mounted_docker_path,
            "--env",
            "DOCKER_CONFIG=/.docker",
            "--env",
            "KUBECONFIG=/.kube/config",
            &kubecfg_image,
            "show",
            "oci://gcr.io/mkm-cloud/package-demo:v1",
            "--alpha",
            "--reorder=server",
            "--overlay-code-file",
            "appInstance_=/overlay/fake-package.yml",
        ];

        let output =
            emit_commandline(&app_instance, TEST_PACKAGE_FILE, None, is_local, skip_auth).await;

        assert_eq!(output, expected);
    }
}
