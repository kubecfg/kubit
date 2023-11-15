use crate::{
    apply::KUBECTL_APPLYSET_ENABLED,
    apply::{KUBECTL_IMAGE, KUBIT_APPLIER_FIELD_MANAGER},
    resources::AppInstance,
    scripting::Script,
    Result,
};
use home::home_dir;
use kube::ResourceExt;
use std::env;

pub fn emit_commandline(
    app_instance: &AppInstance,
    deletion_dir: &str,
    is_local: bool,
) -> Vec<String> {
    let mut cli: Vec<String> = vec![];

    if is_local {
        let user_home = home_dir().expect("unable to retrieve home directory");
        let kube_config =
            env::var("KUBECONFIG").unwrap_or(format!("{}/.kube/config", user_home.display()));
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
                // The empty applyset must be mounted to be seen by the container.
                "-v",
                &format!("{}:{}", deletion_dir, deletion_dir),
                "--env",
                KUBECTL_APPLYSET_ENABLED,
                "--env",
                "KUBECONFIG=/.kube/config",
                KUBECTL_IMAGE,
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
            deletion_dir,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
    );

    cli
}

pub fn emit_post_deletion_commandline(app_instance: &AppInstance, is_local: bool) -> Vec<String> {
    let mut cli: Vec<String> = vec![];

    if is_local {
        let user_home = home_dir().expect("unable to retrieve home directory");
        let kube_config =
            env::var("KUBECONFIG").unwrap_or(format!("{}/.kube/config", user_home.display()));
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
                "KUBECONFIG=/.kube/config",
                KUBECTL_IMAGE,
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
            "delete",
            "configmap",
            &configmap_name_for(app_instance.name_any()),
            "--namespace",
            &app_instance.namespace_any(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
    );

    cli
}

pub fn emit_deletion_setup(ns: &str, output_file: &str) -> Vec<String> {
    [
        "kubit",
        "helper",
        "cleanup",
        "--namespace",
        ns,
        "--output",
        output_file,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>()
}

/// Utility to generate the cleanup configmap name based on a given name.
pub fn configmap_name_for(name: String) -> String {
    format!("{name}-cleanup")
}

/// Generates a shell script that will cleanup the created AppInstance resources.
pub fn script(app_instance: &AppInstance, deletion_dir: &str, is_local: bool) -> Result<Script> {
    let tokens = emit_commandline(app_instance, deletion_dir, is_local);
    Ok(Script::from_vec(tokens))
}

/// Generates a shell script that is used post prune operation of the AppInstance
/// resources. In other words, it is used to delete the blank ConfigMap that was
/// used as the blank applyset.
pub fn post_pruning_script(app_instance: &AppInstance, is_local: bool) -> Result<Script> {
    let configmap_deletion = emit_post_deletion_commandline(app_instance, is_local);
    Ok(Script::from_vec(configmap_deletion))
}

/// Generates a shell script that is used as a helper during the cleanup process
/// of the associated AppInstance.
pub fn setup_script(app_instance: &AppInstance, deletion_dir: &str) -> Result<Script> {
    let cleanup_helper = emit_deletion_setup(&app_instance.namespace_any(), deletion_dir);
    Ok(Script::from_vec(cleanup_helper))
}
