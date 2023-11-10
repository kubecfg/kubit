use crate::{
    apply::KUBECTL_APPLYSET_ENABLED,
    apply::{KUBECTL_IMAGE, KUBIT_APPLIER_FIELD_MANAGER},
    resources::AppInstance,
};
use home::home_dir;
use k8s_openapi::api::core::v1::Namespace;
use kube::core::ObjectMeta;
use kube::ResourceExt;
use std::env;

pub fn emit_commandline(app_instance: &AppInstance, is_local: bool) -> Vec<String> {
    let mut cli: Vec<String> = vec![];

    let user_home = home_dir().expect("unable to retrieve home directory");
    let kube_config =
        env::var("KUBECONFIG").unwrap_or(format!("{}/.kube/config", user_home.display()));

    let namespace = Namespace {
        metadata: ObjectMeta {
            name: Some(app_instance.namespace_any()),
            ..Default::default()
        },
        ..Default::default()
    };

    let ns = serde_yaml::to_string(&namespace).unwrap();

    cli.extend(
        // TODO: CHANGE THIS TO A BASIC VOLUME MOUNT
        ["cat", &ns, "|"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    );

    if is_local {
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
            "-",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
    );

    cli
}
