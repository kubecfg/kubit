use docker_credential::DockerCredential;
use futures::StreamExt;
use k8s_openapi::{
    api::{
        batch::v1::{Job, JobSpec},
        core::v1::{
            Container, EnvVar, KeyToPath, PodSpec, PodTemplateSpec, SecretVolumeSource,
            ServiceAccount, Volume, VolumeMount,
        },
        rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject},
    },
    apimachinery::pkg::apis::meta::v1::OwnerReference,
};

use std::{collections::HashMap, sync::Arc, time::Duration};

use kube::{
    api::{ListParams, Patch, PatchParams, PostParams},
    core::ObjectMeta,
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
    Api, Client, Resource, ResourceExt,
};
use oci_distribution::{
    manifest::OciManifest, secrets::RegistryAuth, Client as OCIClient, Reference,
};
use serde::{Deserialize, Serialize};
use serde_json;

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::{apply, render, resources::AppInstance, Error, Result};

const PACK_KEY: &str = "pack.kubecfg.dev/v1alpha1";

const KUBECTL_IMAGE: &str =
    "bitnami/kubectl@sha256:d5229eb7ad4fa8e8cb9004e63b6b257fe5c925de4bde9c6fcbee5e758c08cc13";

const APPLIER_SERVICE_ACCOUNT: &str = "kubit-applier";

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

    setup_rbac(&app_instance, Arc::clone(&ctx)).await?;

    launch_job(&app_instance, &kubecfg_pack_metadata, Arc::clone(&ctx)).await?;

    Ok(Action::await_change())
}

fn handle_resource_exists<R>(res: kube::Result<R>) -> Result<()>
where
    R: kube::Resource,
{
    match res {
        Err(kube::Error::Api(ae)) => match ae.code {
            409 => {
                info!(
                    "{} resource already exist, doing nothing",
                    tynm::type_name::<R>()
                );
                Ok(())
            }
            _ => Err(kube::Error::Api(ae).into()),
        },
        Err(e) => Err(e.into()),
        Ok(_) => Ok(()),
    }
}

fn owned_by(app_instance: &AppInstance) -> Option<Vec<OwnerReference>> {
    app_instance.controller_owner_ref(&()).map(|o| vec![o])
}

async fn setup_rbac(app_instance: &AppInstance, ctx: Arc<Context>) -> Result<()> {
    let ns = app_instance.clone().namespace().unwrap();
    let pp = PatchParams::apply("kubit").force();

    let metadata = ObjectMeta {
        name: Some(APPLIER_SERVICE_ACCOUNT.to_string()),
        namespace: app_instance.namespace().clone(),
        owner_references: owned_by(app_instance),
        ..Default::default()
    };

    let service_account: Api<ServiceAccount> = Api::namespaced(ctx.client.clone(), &ns);
    let res = ServiceAccount {
        metadata: metadata.clone(),
        ..Default::default()
    };
    service_account
        .patch(&res.name_any(), &pp, &Patch::Apply(&res))
        .await?;

    let role: Api<Role> = Api::namespaced(ctx.client.clone(), &ns);
    let res = Role {
        metadata: metadata.clone(),
        rules: Some(vec![PolicyRule {
            api_groups: Some(["*"].iter().map(|s| s.to_string()).collect()),
            resources: Some(["*"].iter().map(|s| s.to_string()).collect()),
            verbs: ["create", "update", "get", "list", "patch", "watch"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        }]),
    };
    role.patch(&res.name_any(), &pp, &Patch::Apply(&res))
        .await?;

    let api: Api<RoleBinding> = Api::namespaced(ctx.client.clone(), &ns);
    let role_binding = RoleBinding {
        metadata: metadata.clone(),
        role_ref: RoleRef {
            api_group: "rbac.authorization.k8s.io".to_string(),
            kind: "Role".to_string(),
            name: APPLIER_SERVICE_ACCOUNT.to_string(),
        },
        subjects: Some(vec![Subject {
            kind: "ServiceAccount".to_string(),
            name: APPLIER_SERVICE_ACCOUNT.to_string(),
            ..Default::default()
        }]),
    };
    api.patch(&role_binding.name_any(), &pp, &Patch::Apply(&role_binding))
        .await?;

    Ok(())
}

async fn launch_job(
    app_instance: &AppInstance,
    kubecfg_pack_metadata: &KubecfgPackageMetadata,
    ctx: Arc<Context>,
) -> Result<()> {
    let kubecfg_version = &kubecfg_pack_metadata.version;
    let kubecfg_image = format!("{}:{kubecfg_version}", ctx.kubecfg_image);

    info!("Using: {}", kubecfg_image);

    let ns = &app_instance.namespace().ok_or(Error::NamespaceRequired)?;
    let job_name = format!("kubit-apply-{}", app_instance.name_any());

    let volumes = vec![
        Volume {
            name: "docker".to_string(),
            secret: Some(SecretVolumeSource {
                secret_name: Some("gar-docker-secret".to_string()),
                items: Some(vec![KeyToPath {
                    key: ".dockerconfigjson".to_string(),
                    path: "config.json".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        Volume {
            name: "overlay".to_string(),
            empty_dir: Some(Default::default()),
            ..Default::default()
        },
        Volume {
            name: "manifests".to_string(),
            empty_dir: Some(Default::default()),
            ..Default::default()
        },
    ];

    let mk_mount = |name: &str| VolumeMount {
        name: name.to_string(),
        mount_path: format!("/{name}"),
        ..Default::default()
    };

    let volume_mounts = Some(volumes.iter().map(|v| mk_mount(&v.name)).collect());
    let container_defaults = Container {
        volume_mounts: volume_mounts.clone(),
        env: Some(vec![
            EnvVar {
                name: "DOCKER_CONFIG".to_string(),
                value: Some("/docker".to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "KUBECTL_APPLYSET".to_string(),
                value: Some("true".to_string()),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };

    let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name),
            namespace: app_instance.namespace().clone(),
            owner_references: owned_by(app_instance),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(1),
            template: PodTemplateSpec {
                spec: Some(PodSpec {
                    service_account: Some(APPLIER_SERVICE_ACCOUNT.to_string()),
                    restart_policy: Some("Never".to_string()),
                    volumes: Some(volumes),
                    init_containers: Some(vec![
                        Container {
                            name: "fetch-app-instance".to_string(),
                            image: Some(KUBECTL_IMAGE.to_string()),
                            command: Some(
                                ["/bin/bash", "-c"].iter().map(|s| s.to_string()).collect(),
                            ),
                            args: Some(vec![render::emit_fetch_app_instance_script(
                                ns,
                                &app_instance.name_any(),
                                "/overlay/appinstance.json",
                            )]),
                            ..container_defaults.clone()
                        },
                        Container {
                            name: "render-manifests".to_string(),
                            image: Some(kubecfg_image.clone()),
                            command: Some(render::emit_commandline(
                                app_instance,
                                "/overlay/appinstance.json",
                                "/manifests",
                            )),
                            ..container_defaults.clone()
                        },
                    ]),
                    containers: vec![Container {
                        name: "apply-manifests".to_string(),
                        image: Some(KUBECTL_IMAGE.to_string()),
                        command: Some(apply::emit_commandline(app_instance, "/manifests")),
                        ..container_defaults.clone()
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

    handle_resource_exists(jobs.create(&pp, &job).await)?;

    // TODO:
    //
    // 1. if job exists check if it's has terminated and take action
    // 2. make sure we watch the job as it changes status

    Ok(())
}
