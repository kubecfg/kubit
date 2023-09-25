use crate::{metadata, resources::AppInstance, scripting::Script, Error, Result, KUBECFG_REGISTRY};
use home::home_dir;
use tempfile::NamedTempFile;
use std::env;

/// Generates shell script that will render the manifest and writes it to writer.
pub async fn emit_script<W>(app_instance: &AppInstance, is_local: bool, w: &mut W) -> Result<()>
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
) -> Result<Script> {
    let tokens = emit_commandline(app_instance, overlay_file_name, output_dir, is_local).await;
    Ok(Script::from_vec(tokens))
}

pub async fn emit_commandline(
    app_instance: &AppInstance,
    overlay_file: &str,
    output_dir: Option<&str>,
    is_local: bool,
) -> Vec<String> {
    let image = &app_instance.spec.package.image;

    let entrypoint = if image.starts_with("file://") {
        image.clone()
    } else {
        format!("oci://{image}")
    };

    let mut cli: Vec<String> = vec![];
    let overlay_path = std::fs::canonicalize(overlay_file).unwrap();
    let overlay_file_name = std::path::PathBuf::from(overlay_path.file_name().unwrap());
    let user_home = home_dir().expect("unable to retrieve home directory");
    let docker_config =
        env::var("DOCKER_CONFIG").unwrap_or(format!("{}/.docker", user_home.display()));
    let kube_config =
        env::var("KUBECONFIG").unwrap_or(format!("{}/.kube/config", user_home.display()));

    if is_local {
        let temp_file = NamedTempFile::new().expect("unable to create temporary file");
        serde_yaml::to_writer(&temp_file, app_instance).unwrap();

        let meta = metadata::fetch_package_config(temp_file.path().to_str().unwrap()).await.unwrap();
        let kubecfg_image = meta.versioned_kubecfg_image(KUBECFG_REGISTRY).expect("unable to parse kubecfg image");

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
                &kubecfg_image
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
                &format!("appInstance_={}", overlay_path.display()),
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
