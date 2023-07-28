use docker_credential::DockerCredential;
use futures::StreamExt;
use k8s_openapi::api::{
    batch::v1::{Job, JobSpec},
    core::v1::{Container, PodSpec, PodTemplateSpec},
};
use std::{collections::HashMap, sync::Arc, time::Duration};

use kube::{
    api::{ListParams, PostParams},
    core::ObjectMeta,
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
    Api, Client, ResourceExt,
};
use oci_distribution::{
    manifest::OciManifest, secrets::RegistryAuth, Client as OCIClient, Reference,
};
use serde::{Deserialize, Serialize};
use serde_json;

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::{render, resources::AppInstance, Error, Result};

const PACK_KEY: &str = "pack.kubecfg.dev/v1alpha1";

struct Context {
    client: Client,
    kubecfg_image: String,
}

fn error_policy(app_instance: Arc<AppInstance>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = app_instance.name_any();
    warn!(?name, %error, "reconcile failed");
    // TODO(mkm): make error requeue duration configurable
    Action::requeue(Duration::from_secs(5))
}

pub async fn run(client: Client, kubecfg_image: String) -> Result<()> {
    let docs = Api::<AppInstance>::all(client.clone());
    if let Err(e) = docs.list(&ListParams::default().limit(1)).await {
        error!("CRD is not queryable; {e:?}. Is the CRD installed?");
        std::process::exit(1);
    }
    Controller::new(docs, watcher::Config::default().any_semantic())
        .shutdown_on_signal()
        .run(
            reconcile,
            error_policy,
            Arc::new(Context {
                client,
                kubecfg_image,
            }),
        )
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageConfig {
    entrypoint: String,
    #[serde(default)]
    metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KubecfgPackageMetadata {
    version: String,
}

async fn reconcile(app_instance: Arc<AppInstance>, ctx: Arc<Context>) -> Result<Action> {
    info!(?app_instance, "running reconciler");

    let image = &app_instance.spec.package.image;
    info!(image, "fetching image");

    let reference: Reference = image.parse()?;
    info!(?reference, "reference");
    let credentials = docker_credential::get_credential(reference.registry())?;
    let DockerCredential::UsernamePassword(username, password ) = credentials else {todo!()};
    let auth = RegistryAuth::Basic(username, password);
    // TODO: handle the case of unauthenticated repositories

    let client_config = oci_distribution::client::ClientConfig {
        protocol: oci_distribution::client::ClientProtocol::Https,
        ..Default::default()
    };
    let mut client = OCIClient::new(client_config);
    let (manifest, _) = client.pull_manifest(&reference, &auth).await?;

    let manifest = match manifest {
        OciManifest::Image(manifest) => manifest,
        OciManifest::ImageIndex(_) => return Err(Error::UnsupportedManifestIndex),
    };

    let mut buf = vec![];
    client
        .pull_blob(&reference, &manifest.config.digest, &mut buf)
        .await?;

    let config: PackageConfig = serde_json::from_slice(&buf).map_err(Error::DecodePackageConfig)?;
    info!(?config, "got package config");

    let kubecfg_pack_metadata: KubecfgPackageMetadata =
        serde_json::from_value(config.metadata.get(PACK_KEY).unwrap().clone())
            .map_err(Error::DecodeKubecfgPackageMetadata)?;

    launch_job(&app_instance, &kubecfg_pack_metadata, Arc::clone(&ctx)).await?;

    Ok(Action::await_change())
}

async fn launch_job(
    app_instance: &AppInstance,
    kubecfg_pack_metadata: &KubecfgPackageMetadata,
    ctx: Arc<Context>,
) -> Result<()> {
    let kubecfg_version = &kubecfg_pack_metadata.version;
    let kubecfg_image = format!("{}:{kubecfg_version}", ctx.kubecfg_image);

    let mut buf = vec![];
    render::emit_script(app_instance, &mut buf)?;
    let script = String::from_utf8(buf).unwrap(); // for debug only

    info!(
        ?kubecfg_image,
        script, "TODO: create a Job that runs kubecfg and renders the jsonnet artifact"
    );

    let ns = &app_instance.namespace().ok_or(Error::NamespaceRequired)?;
    let job_name = format!("kubit-apply-{}", app_instance.name_any());

    let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name),
            namespace: app_instance.namespace().clone(),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(1),
            template: PodTemplateSpec {
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    containers: vec![Container {
                        name: "kubecfg".to_string(),
                        image: Some(kubecfg_image.clone()),
                        command: Some(
                            // TODO: use render::emit_commandline
                            ["kubecfg", "version"]
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                        ),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let pp = PostParams::default();

    match jobs.create(&pp, &job).await {
        Ok(o) => info!(?o, "Created job"),
        Err(kube::Error::Api(ae)) => match ae.code {
            409 => info!("job already exist, doing nothing"),
            _ => return Err(kube::Error::Api(ae).into()),
        },
        Err(e) => panic!("API error: {}", e),
    }

    // TODO:
    //
    // 1. if job exists check if it's has terminated and take action
    // 2. make sure we watch the job as it changes status

    Ok(())
}
