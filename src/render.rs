use yash_quote::quoted;

use crate::{resources::AppInstance, Error, Result};

/// Generates shell script that will render the manifest
pub fn emit_script<W>(app_instance: &AppInstance, w: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    let tmp = tempfile::Builder::new().suffix(".json").tempfile()?;
    let (mut file, path) = tmp.keep()?;
    serde_json::to_writer(&mut file, &app_instance).map_err(Error::RenderOverlay)?;

    writeln!(w, "#!/bin/bash")?;
    writeln!(w, "set -euo pipefail")?;

    for i in emit_commandline(app_instance, &path.to_string_lossy(), "/tmp/manifests") {
        write!(w, "{} ", quoted(&i))?;
    }
    writeln!(w)?;
    Ok(())
}

pub fn emit_commandline(
    app_instance: &AppInstance,
    overlay_file: &str,
    output_dir: &str,
) -> Vec<String> {
    let image = &app_instance.spec.package.image;

    [
        "kubecfg",
        "show",
        "--alpha",
        "--reorder=server",
        &format!("oci://{image}"),
        "--overlay-code-file",
        &format!("appInstance_={overlay_file}"),
        "--export-dir",
        output_dir,
        "--export-filename-format",
        "{{printf \"%03d\" (resourceIndex .)}}-{{.apiVersion}}.{{.kind}}-{{default \"default\" .metadata.namespace}}.{{.metadata.name}}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn emit_fetch_app_instance_script(ns: &str, name: &str, output_file: &str) -> String {
    let ns = quoted(ns);
    let name = quoted(name);
    let output_file = quoted(output_file);
    format!("kubectl get appinstances.kubecfg.dev --namespace {ns} {name} -o json >{output_file}")
}
