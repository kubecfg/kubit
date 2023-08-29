use crate::{resources::AppInstance, scripting::Script, Error, Result};

/// Generates shell script that will render the manifest and writes it to writer.
pub fn emit_script<W>(app_instance: &AppInstance, w: &mut W) -> Result<()>
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
    )?;
    writeln!(w, "{script}")?;
    Ok(())
}

/// Generates shell script that will render the manifest
pub fn script(
    app_instance: &AppInstance,
    overlay_file_name: &str,
    output_dir: Option<&str>,
) -> Result<Script> {
    let tokens = emit_commandline(app_instance, overlay_file_name, output_dir);
    Ok(Script::from_vec(tokens))
}

pub fn emit_commandline(
    app_instance: &AppInstance,
    overlay_file: &str,
    output_dir: Option<&str>,
) -> Vec<String> {
    let image = &app_instance.spec.package.image;

    let entrypoint = if image.starts_with("file://") {
        image.clone()
    } else {
        format!("oci://{image}")
    };

    let mut cli = [
        "kubecfg",
        "show",
        "--alpha",
        "--reorder=server",
        &entrypoint,
        "--overlay-code-file",
        &format!("appInstance_={overlay_file}"),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect::<Vec<String>>();

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
