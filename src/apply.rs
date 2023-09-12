use crate::{resources::AppInstance, scripting::Script, Result};
use kube::ResourceExt;

pub const KUBIT_APPLIER_FIELD_MANAGER: &str = "kubit-applier";

/// Generates shell script that will apply the manifests and writes it to w
pub fn emit_script<W>(app_instance: &AppInstance, is_local: bool, w: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    let script = script(app_instance, "/tmp/manifests", &None, is_local)?;
    write!(w, "{script}")?;
    Ok(())
}

/// Generates shell script that will apply the manifests
pub fn script(
    app_instance: &AppInstance,
    manifests_dir: &str,
    impersonate_user: &Option<String>,
    is_local: bool,
) -> Result<Script> {
    let tokens = emit_commandline(app_instance, manifests_dir, impersonate_user, is_local);
    Ok(Script::from_vec(tokens))
}

pub fn emit_commandline(
    app_instance: &AppInstance,
    manifests_dir: &str,
    impersonate_user: &Option<String>,
    is_local: bool,
) -> Vec<String> {
    let mut cli: Vec<String> = vec![];

    if is_local {
        cli.extend(
            [
                "docker",
                "run",
                "--rm",
                "-v",
                "$HOME/.kube/config:/.kube/config",
                &format!("{manifests_dir}:{manifests_dir}"),
                "--env",
                "KUBECTL_APPLYSET=true", // TODO: this can be a const
                "bitnami/kubectl:v1.27",
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
            "-f",
            manifests_dir,
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
