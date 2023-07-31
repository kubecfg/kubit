use yash_quote::quoted;

use crate::{resources::AppInstance, Error, Result};

/// Generates shell script that will render the manifest
pub fn emit_script<W>(app_instance: &AppInstance, w: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    let image = &app_instance.spec.package.image;

    let tmp = tempfile::Builder::new().suffix(".json").tempfile()?;
    let (mut file, path) = tmp.keep()?;
    serde_json::to_writer(&mut file, &app_instance).map_err(Error::RenderOverlay)?;

    writeln!(w, "#!/bin/bash")?;
    writeln!(w, "set -euo pipefail")?;
    writeln!(
        w,
        r#"kubecfg show oci://{} --alpha --overlay-code-file appInstance_={}"#,
        quoted(image),
        quoted(&path.to_string_lossy()),
    )?;
    Ok(())
}

pub fn emit_commandline(app_instance: &AppInstance, overlay_file: &str) -> Vec<String> {
    let image = &app_instance.spec.package.image;

    [
        "kubecfg",
        "show",
        "--alpha",
        "--reorder=server",
        &format!("oci://{}", image),
        "--overlay-code-file",
        &format!("appInstance_={overlay_file}"),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
