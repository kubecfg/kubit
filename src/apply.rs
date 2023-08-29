use crate::{resources::AppInstance, scripting::Script, Result};
use kube::ResourceExt;

pub const KUBIT_APPLIER_FIELD_MANAGER: &str = "kubit-applier";

/// Generates shell script that will apply the manifests and writes it to w
pub fn emit_script<W>(app_instance: &AppInstance, w: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    let script = script(app_instance, "/tmp/manifests", &None)?;
    write!(w, "{script}")?;
    Ok(())
}

/// Generates shell script that will apply the manifests
pub fn script(
    app_instance: &AppInstance,
    manifests_dir: &str,
    as_user: &Option<String>,
) -> Result<Script> {
    let tokens = emit_commandline(app_instance, manifests_dir, as_user);
    Ok(Script::from_vec(tokens))
}

pub fn emit_commandline(
    app_instance: &AppInstance,
    manifests_dir: &str,
    as_user: &Option<String>,
) -> Vec<String> {
    let mut cli = vec![
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
        "--field-manager",
        KUBIT_APPLIER_FIELD_MANAGER,
        "--force-conflicts",
        "-v=2",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>();

    if let Some(as_user) = as_user {
        cli.push(format!("--as={as_user}"));
    }

    cli
}
