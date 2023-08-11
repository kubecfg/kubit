use yash_quote::quoted;

use crate::{resources::AppInstance, Result};
use kube::ResourceExt;

/// Generates shell script that will apply the manifests
pub fn emit_script<W>(app_instance: &AppInstance, w: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    writeln!(w, "#!/bin/bash")?;
    writeln!(w, "set -euo pipefail")?;

    for i in emit_commandline(app_instance, "/tmp/manifests") {
        write!(w, "{} ", quoted(&i))?;
    }
    writeln!(w)?;
    Ok(())
}

pub fn emit_commandline(app_instance: &AppInstance, manifests_dir: &str) -> Vec<String> {
    vec![
        "kubectl",
        "apply",
        "-f",
        manifests_dir,
        "-n",
        &app_instance.namespace().unwrap(),
        "--server-side",
        "--prune",
        "--applyset",
        &app_instance.name_any(),
        "--force-conflicts",
        "-v",
        "2",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
