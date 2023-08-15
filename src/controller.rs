use futures::StreamExt;
use itertools::Itertools;
use k8s_openapi::{
    api::{
        batch::v1::{Job, JobSpec},
        core::v1::{
            Container, EnvVar, KeyToPath, Pod, PodSpec, PodTemplateSpec, Secret,
            SecretVolumeSource, ServiceAccount, Volume, VolumeMount,
        },
        rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject},
    },
    apimachinery::pkg::apis::meta::v1::OwnerReference,
};
use std::{collections::HashMap, sync::Arc, time::Duration};

use kube::{
    api::{DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams, PropagationPolicy},
    core::ObjectMeta,
    runtime::{
        conditions::{is_job_completed, Condition},
        controller::{Action, Controller},
        watcher,
    },
    Api, Client, Resource, ResourceExt,
};
use oci_distribution::{
    manifest::OciManifest, secrets::RegistryAuth, Client as OCIClient, Reference,
};
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::{
    apply,
    docker_config::DockerConfig,
    render,
    resources::{AppInstance, AppInstanceStatus},
    Error, Result,
};

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

    info!("running kubit manager");
    let jobs = Api::<Job>::all(client.clone());
    Controller::new(docs, watcher::Config::default().any_semantic())
        .shutdown_on_signal()
        .owns(jobs, watcher::Config::default().any_semantic())
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

impl PackageConfig {
    fn kubecfg_package_metadata(&self) -> Result<KubecfgPackageMetadata> {
        serde_json::from_value(self.metadata.get(PACK_KEY).unwrap().clone())
            .map_err(Error::DecodeKubecfgPackageMetadata)
    }

    fn versioned_kubecfg_image(&self, ctx: &Context) -> Result<String> {
        let kubecfg_version = &self.kubecfg_package_metadata()?.version;
        Ok(format!("{}:{kubecfg_version}", ctx.kubecfg_image))
    }
}

#[derive(Debug, Clone)]
enum ReconciliationState {
    Idle,
    Executing,
    JobTerminated(String, JobOutcome),
}

#[derive(Debug, Clone, Copy)]
enum JobOutcome {
    Success,
    Failure,
}

/// This is the main logic of the controller. This function gets called every time some resource related to the appInstance
/// changes. This function should be idempotent.
async fn reconcile(app_instance: Arc<AppInstance>, ctx: Arc<Context>) -> Result<Action> {
    info!(
        name = app_instance.name_any(),
        namespace = app_instance.namespace(),
        "--------------- Running reconciler ---------------"
    );
    // slow down things a little bit
    tokio::time::sleep(Duration::from_secs(1)).await;

    let state = reconciliation_state(&app_instance, &ctx).await?;
    info!(?state);

    let action = match state {
        ReconciliationState::Idle => {
            launch_job(&app_instance, &ctx).await?;
            Action::await_change()
        }
        ReconciliationState::Executing => {
            info!(
                job_name = job_name_for(&app_instance),
                "waiting for applier job execution"
            );
            Action::await_change()
        }
        ReconciliationState::JobTerminated(job_uid, outcome) => {
            let action = match outcome {
                JobOutcome::Success => {
                    info!("job completed successfully");
                    Action::await_change()
                }
                JobOutcome::Failure => {
                    info!("job failed");
                    Action::requeue(Duration::from_secs(60))
                }
            };
            capture_logs(&app_instance, &ctx, job_uid).await?;
            delete_job(&app_instance, &ctx).await?;
            action
        }
    };

    Ok(action)
}

async fn reconciliation_state(
    app_instance: &AppInstance,
    ctx: &Context,
) -> Result<ReconciliationState> {
    let ns = app_instance.namespace().unwrap();
    let api: Api<Job> = Api::namespaced(ctx.client.clone(), &ns);
    let job_name = job_name_for(app_instance);
    let job = api.get_opt(&job_name).await?;

    Ok(match job {
        Some(job) => {
            let uid = job
                .labels()
                .get("controller-uid")
                .expect("Jobs must have controller-uid label")
                .clone();

            fn condition(job: &Job, cond: impl Condition<Job>) -> bool {
                cond.matches_object(Some(job))
            }
            if condition(&job, is_job_completed()) {
                ReconciliationState::JobTerminated(uid, JobOutcome::Success)
            } else if condition(&job, is_job_failed()) {
                ReconciliationState::JobTerminated(uid, JobOutcome::Failure)
            } else {
                ReconciliationState::Executing
            }
        }
        None => ReconciliationState::Idle,
    })
}

async fn get_image_pull_secrets(app_instance: &AppInstance, ctx: &Context) -> Result<RegistryAuth> {
    info!("getting image pull credentials");

    let secret_name = {
        let Some(ref refs) = app_instance.spec.image_pull_secrets else {
            return Ok(RegistryAuth::Anonymous)
        };
        if refs.is_empty() {
            return Ok(RegistryAuth::Anonymous);
        }
        refs.iter()
            .exactly_one()
            .map_err(|_| Error::UnsupportedMultipleImagePullSecrets)?
            .name
            .as_ref()
            .expect("schema validation would have enforced this")
    };

    let ns = &app_instance.namespace().ok_or(Error::NamespaceRequired)?;
    let secrets: Api<Secret> = Api::namespaced(ctx.client.clone(), ns);
    let secret = secrets.get(secret_name).await?;

    if secret.type_ != Some("kubernetes.io/dockerconfigjson".to_string()) {
        return Err(Error::BadImagePullSecretType(secret.type_));
    }

    let docker_config = secret
        .data
        .as_ref()
        .and_then(|data| data.get(".dockerconfigjson"))
        .ok_or(Error::NoDockerConfigJsonInImagePullSecret)?;

    let docker_config = DockerConfig::from_slice(&docker_config.0)?;

    let reference: Reference = app_instance.spec.package.image.parse()?;
    Ok(docker_config.get_auth(reference.registry())?)
}

async fn fetch_package_config(app_instance: &AppInstance, ctx: &Context) -> Result<PackageConfig> {
    let image = &app_instance.spec.package.image;
    info!(image, "fetching image");

    let reference: Reference = image.parse()?;
    info!(?reference, "reference");
    let auth = get_image_pull_secrets(app_instance, ctx).await?;

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

    let config = serde_json::from_slice(&buf).map_err(Error::DecodePackageConfig)?;

    Ok(config)
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

fn patch_params() -> PatchParams {
    PatchParams::apply("kubit").force()
}

async fn setup_job_rbac(app_instance: &AppInstance, ctx: &Context) -> Result<()> {
    let ns = app_instance.clone().namespace().unwrap();
    let pp = patch_params();

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

async fn launch_job(app_instance: &AppInstance, ctx: &Context) -> Result<()> {
    setup_job_rbac(app_instance, ctx).await?;

    let package_config: PackageConfig = fetch_package_config(app_instance, ctx).await?;
    info!(?package_config, "got package config");

    let kubecfg_image = package_config.versioned_kubecfg_image(ctx)?;
    info!("Using: {}", kubecfg_image);

    create_job(app_instance, kubecfg_image, ctx).await
}

fn job_name_for(app_instance: &AppInstance) -> String {
    format!("kubit-apply-{}", app_instance.name_any())
}

async fn delete_job(app_instance: &AppInstance, ctx: &Context) -> Result<()> {
    let ns = &app_instance.namespace().ok_or(Error::NamespaceRequired)?;
    let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
    let name = job_name_for(app_instance);
    jobs.delete(
        &name,
        &DeleteParams {
            propagation_policy: Some(PropagationPolicy::Foreground),
            ..Default::default()
        },
    )
    .await?;
    info!(name, "job deleted");
    Ok(())
}

async fn create_job(
    app_instance: &AppInstance,
    kubecfg_image: String,
    ctx: &Context,
) -> Result<()> {
    let ns = &app_instance.namespace().ok_or(Error::NamespaceRequired)?;
    let job_name = job_name_for(app_instance);

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
            backoff_limit: Some(0),
            template: PodTemplateSpec {
                spec: Some(PodSpec {
                    service_account: Some(APPLIER_SERVICE_ACCOUNT.to_string()),
                    restart_policy: Some("Never".to_string()),
                    active_deadline_seconds: Some(60),
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
                                Some("/manifests"),
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

    Ok(())
}

// kube crate comes with is_job_completed but that condition is true only if it completes successfully.
fn is_job_failed() -> impl Condition<Job> {
    |obj: Option<&Job>| {
        if let Some(job) = &obj {
            if let Some(s) = &job.status {
                if let Some(conds) = &s.conditions {
                    if let Some(pcond) = conds.iter().find(|c| c.type_ == "Failed") {
                        return pcond.status == "True";
                    }
                }
            }
        }
        false
    }
}

async fn capture_logs(app_instance: &AppInstance, ctx: &Context, job_uid: String) -> Result<()> {
    let ns = &app_instance.namespace().ok_or(Error::NamespaceRequired)?;
    info!(?ns, "reporting errors");

    let pods_api: Api<Pod> = Api::namespaced(ctx.client.clone(), ns);
    let job_name = job_name_for(app_instance);

    let pods = pods_api
        .list(&ListParams {
            label_selector: Some(format!("job-name={job_name},controller-uid={job_uid}")),
            ..Default::default()
        })
        .await?;

    let mut per_container_logs = HashMap::new();

    // There should be exactly one pod per job. In the unlikely even
    // something is broken with k8s and we end up getting two pods matching the same job uid
    // let's just get the logs of all these pods and concatenate them together. Chances are
    // that this is easier to debug than just getting logs for a random pod.
    //
    // NOTE(mkm): I don't know how likely this is to happen so I'm not sure if it's worth doing
    // something more complicated like capturing the pod names and grouping the logs by pod name.
    for pod in pods.items {
        let mut container_names = vec![];

        let pod_status = pod.status.as_ref().unwrap();
        let container_statuses = [
            pod_status.init_container_statuses.as_ref(),
            pod_status.container_statuses.as_ref(),
        ]
        .into_iter()
        .flatten()
        .flat_map(|vec| vec.iter());

        for status in container_statuses {
            // we cannot get logs from a container that hasn't started yet.
            // We know a container hasn't started yet when:
            // 1. the container is explicitly in the "waiting" state
            // 2. the state field is empty
            let is_waiting = status
                .state
                .as_ref()
                .map(|x| x.waiting.is_some())
                .unwrap_or(true);
            info!(name = status.name, ?is_waiting, "Container status");
            if !is_waiting {
                container_names.push(&status.name);
            }
        }

        for container_name in container_names {
            let logs = pods_api
                .logs(
                    &pod.name_any(),
                    &LogParams {
                        container: Some(container_name.clone()),
                        ..Default::default()
                    },
                )
                .await?;
            per_container_logs
                .entry(container_name.clone())
                .and_modify(|e: &mut String| e.push_str(&logs))
                .or_insert(logs);
        }
        let logs_json =
            serde_json::to_string(&per_container_logs).expect("cannot render basic json");
        info!(logs_json);
    }

    let app_instance_api: Api<AppInstance> = Api::namespaced(ctx.client.clone(), ns);

    let app_instance_patch = AppInstance {
        metadata: Default::default(),
        spec: Default::default(),
        status: Some(AppInstanceStatus {
            last_logs: Some(per_container_logs),
        }),
    };
    app_instance_api
        .patch_status(
            &app_instance.name_any(),
            &patch_params(),
            &Patch::Apply(&app_instance_patch),
        )
        .await?;
    info!("status patched");
    Ok(())
}
