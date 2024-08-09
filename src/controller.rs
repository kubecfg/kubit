use futures::StreamExt;
use itertools::Itertools;
use k8s_openapi::{
    api::{
        batch::v1::{Job, JobSpec},
        core::v1::{
            ConfigMap, Container, EnvVar, KeyToPath, Pod, PodSpec, PodTemplateSpec, Secret,
            SecretVolumeSource, ServiceAccount, Volume, VolumeMount,
        },
        rbac::v1::{
            ClusterRole, ClusterRoleBinding, PolicyRule, Role, RoleBinding, RoleRef, Subject,
        },
    },
    apimachinery::pkg::apis::meta::v1::{OwnerReference, Time},
    chrono::Utc,
};
use std::{collections::HashMap, sync::Arc, time::Duration};

use kube::{
    api::{DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams, PropagationPolicy},
    core::ObjectMeta,
    error::ErrorResponse,
    runtime::{
        conditions::{is_deleted, is_job_completed, Condition},
        controller::{Action, Controller},
        finalizer::{finalizer, Event as Finalizer},
        wait::await_condition,
        watcher,
    },
    Api, Client, Resource, ResourceExt,
};
use oci_distribution::{secrets::RegistryAuth, Reference};

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::{
    apply::{self},
    delete,
    docker_config::DockerConfig,
    oci::{self, PackageConfig},
    render,
    resources::{AppInstance, AppInstanceCondition, AppInstanceLikeResources, AppInstanceStatus},
    Error, Result,
};

pub const KUBECTL_IMAGE: &str = "registry.k8s.io/kubectl:v1.28.0";

const APPLIER_SERVICE_ACCOUNT: &str = "kubit-applier";

const KUBIT_FINALIZER: &str = "kubecfg.dev/appinstance-cleanup";

struct Context {
    client: Client,
    kubecfg_image: String,
    kubit_image: String,
    kubectl_image_apply: String,
    kubectl_image_render: String,
    config_map_name: Option<String>,
    only_paused: bool,
}

impl Context {
    pub fn apply_step_image(&self) -> String {
        self.kubectl_image_apply.clone()
    }

    pub fn render_step_image(&self) -> String {
        self.kubectl_image_render.clone()
    }
}

fn error_policy_app_instance(
    app_instance: Arc<AppInstance>,
    error: &Error,
    _ctx: Arc<Context>,
) -> Action {
    let name = app_instance.name_any();
    warn!(?name, %error, "reconcile failed");
    // TODO(mkm): make error requeue duration configurable
    Action::requeue(Duration::from_secs(5))
}

fn error_policy_config_map(
    config_map: Arc<ConfigMap>,
    error: &Error,
    _ctx: Arc<Context>,
) -> Action {
    let config = &config_map.as_ref().data.as_ref().unwrap()["app-instance"];
    let app_instance: Result<AppInstance, _> = serde_yaml::from_str(config);
    match app_instance {
        Ok(ai) => error_policy_app_instance(Arc::new(ai), error, _ctx),
        Err(serr) => {
            warn!(%serr, "failed to convert config map to AppInstance while handling {}", error);
            Action::requeue(Duration::from_secs(5))
        }
    }
}

async fn reconcile_app_instance(
    app_instance: Arc<AppInstance>,
    ctx: Arc<Context>,
) -> Result<Action> {
    reconcile(AppInstanceLike::from(app_instance), ctx).await
}

async fn reconcile_config_map(config_map: Arc<ConfigMap>, ctx: Arc<Context>) -> Result<Action> {
    let cm_name = config_map.name_any();
    if ctx
        .as_ref()
        .config_map_name
        .as_ref()
        .is_some_and(|x| x.as_str() == cm_name)
    {
        let app_instance = AppInstanceLike::from_config_map(config_map, "app-instance");
        match app_instance {
            Ok(ai) => reconcile(ai, ctx).await,
            Err(error) => {
                error!(%error, "failed to convert configmap to appinstance");
                Err(Error::InvalidConfigMap(error.to_string()))
            }
        }
    } else {
        Ok(Action::requeue(Duration::from_secs(5)))
    }
}

async fn reconcile(app_instance: AppInstanceLike, ctx: Arc<Context>) -> Result<Action> {
    info!(
        name = app_instance.name_any(),
        namespace = app_instance.instance.namespace(),
        "--------------- Running reconciler ---------------"
    );
    // slow down things a little bit
    tokio::time::sleep(Duration::from_secs(1)).await;

    if app_instance.instance.spec.pause != ctx.only_paused {
        info!(
            name = app_instance.name_any(),
            ns = app_instance.instance.namespace(),
            app_instance.instance.spec.pause,
            ctx.only_paused,
            "paused"
        );
        return Ok(Action::await_change());
    }

    match app_instance.original {
        AppInstanceLikeResources::AppInstance(app_instance) => {
            let app_instance_api: Api<AppInstance> =
                Api::namespaced(ctx.client.clone(), &app_instance.namespace_any());

            finalizer(
                &app_instance_api,
                KUBIT_FINALIZER,
                app_instance,
                |event| async {
                    match event {
                        Finalizer::Apply(ai) => {
                            AppInstanceLike::from(ai).reconcile_apply(&ctx).await
                        }
                        Finalizer::Cleanup(ai) => {
                            AppInstanceLike::from(ai).reconcile_delete(&ctx).await
                        }
                    }
                },
            )
            .await
            .map_err(|e| Error::FinalizerError(Box::new(e)))
        }
        AppInstanceLikeResources::ConfigMap(config_map) => {
            let config_map_api: Api<ConfigMap> = Api::namespaced(
                ctx.client.clone(),
                &config_map.namespace().unwrap_or_default(),
            );

            finalizer(
                &config_map_api,
                KUBIT_FINALIZER,
                config_map,
                |event| async {
                    match event {
                        Finalizer::Apply(cm) => {
                            AppInstanceLike::from_config_map(cm, "app-instance")?
                                .reconcile_apply(&ctx)
                                .await
                        }
                        Finalizer::Cleanup(cm) => {
                            AppInstanceLike::from_config_map(cm, "app-instance")?
                                .reconcile_delete(&ctx)
                                .await
                        }
                    }
                },
            )
            .await
            .map_err(|e| Error::FinalizerError(Box::new(e)))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: Client,
    kubecfg_image: String,
    kubit_image: String,
    apply_step_image: String,
    render_step_image: String,
    only_paused: bool,
    config_map_name: Option<String>,
    watched_namespace: Option<String>,
) -> Result<()> {
    let namespace = watched_namespace.as_deref();

    let jobs = if let Some(ns) = namespace {
        Api::<Job>::namespaced(client.clone(), ns)
    } else {
        Api::<Job>::all(client.clone())
    };

    info!("apply/delete image: {apply_step_image}");
    info!("render image: {render_step_image}");

    if watched_namespace.is_none() {
        info!("running kubit manager in AppInstance (CRD) mode");
        let docs = if let Some(ns) = namespace {
            Api::<AppInstance>::namespaced(client.clone(), ns)
        } else {
            Api::<AppInstance>::all(client.clone())
        };
        if let Err(e) = docs.list(&ListParams::default().limit(1)).await {
            error!("CRD is not queryable; {e:?}. Is the CRD installed?");
            std::process::exit(1);
        }

        Controller::new(docs, watcher::Config::default().any_semantic())
            .shutdown_on_signal()
            .owns(jobs, watcher::Config::default().any_semantic())
            .run(
                reconcile_app_instance,
                error_policy_app_instance,
                Arc::new(Context {
                    client,
                    kubecfg_image,
                    kubit_image,
                    config_map_name: None,
                    only_paused,
                    kubectl_image_apply: apply_step_image,
                    kubectl_image_render: render_step_image,
                }),
            )
            .filter_map(|x| async move { std::result::Result::ok(x) })
            .for_each(|_| futures::future::ready(()))
            .await;
    } else {
        info!("running kubit manager in ConfigMap, single-namespace mode");
        let docs = if let Some(ns) = namespace {
            Api::<ConfigMap>::namespaced(client.clone(), ns)
        } else {
            error!("ConfigMap configuration requires a namespace.");
            std::process::exit(1);
        };

        Controller::new(docs, watcher::Config::default().any_semantic())
            .shutdown_on_signal()
            .owns(jobs, watcher::Config::default().any_semantic())
            .run(
                reconcile_config_map,
                error_policy_config_map,
                Arc::new(Context {
                    client,
                    kubecfg_image,
                    kubit_image,
                    config_map_name,
                    only_paused,
                    kubectl_image_apply: apply_step_image,
                    kubectl_image_render: render_step_image,
                }),
            )
            .filter_map(|x| async move { std::result::Result::ok(x) })
            .for_each(|_| futures::future::ready(()))
            .await;
    }

    Ok(())
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

#[derive(Debug, Clone)]
pub struct AppInstanceLike {
    instance: Arc<AppInstance>,
    original: AppInstanceLikeResources,
}

impl From<Arc<AppInstance>> for AppInstanceLike {
    fn from(value: Arc<AppInstance>) -> Self {
        Self {
            original: AppInstanceLikeResources::AppInstance(value.clone()),
            instance: value,
        }
    }
}

impl AppInstanceLike {
    pub fn name_any(&self) -> String {
        match &self.original {
            AppInstanceLikeResources::AppInstance(ai) => ai.name_any(),
            // The reconciler needs to watch the name of the ConfigMap,
            // not the name of the AppInstance object inside of it.
            AppInstanceLikeResources::ConfigMap(cm) => cm.name_any(),
        }
    }
    pub fn from_config_map(config_map: Arc<ConfigMap>, key: &str) -> Result<Self> {
        let config = &config_map.as_ref().data.as_ref();
        if let Some(config) = config {
            let config = &config[key];
            let mut ai: AppInstance =
                serde_yaml::from_str(config).map_err(|e| Error::InvalidConfigMap(e.to_string()))?;
            ai.metadata.uid.clone_from(&config_map.metadata.uid);
            Ok(Self {
                original: AppInstanceLikeResources::ConfigMap(config_map),
                instance: Arc::new(ai),
            })
        } else {
            Err(Error::InvalidConfigMap(
                "configmap did not have the `data` property set".to_string(),
            ))
        }
    }

    async fn reconcile_apply(&self, ctx: &Context) -> Result<Action> {
        let state = self.reconciliation_state(ctx).await?;

        // We have two status conditions
        //
        // Reconcilier: It will report the status of each iteration of the reconcilier.
        //              When the reconcilier retries previous failed runs it will report a new fresh run and thus you may
        //              not see the errors of the previous run.
        // Ready: It will report the overall Readiness of the instance installation process. If it fails, the error message will stick
        //        for longer even if there is another ongoing run of the reconcilier that is retrying.

        let action = match state {
            ReconciliationState::Idle => {
                match self.launch_job(ctx).await {
                    Ok(()) => {
                        self.update_condition(
                            ctx,
                            "Reconcilier",
                            "False",
                            "ExpandingTemplate",
                            None,
                        )
                        .await?;
                    }
                    Err(err) => {
                        self.update_condition(ctx, "Reconcilier", "False", "Failed", None)
                            .await?;

                        self.update_condition(
                            ctx,
                            "Ready",
                            "False",
                            "Failed",
                            Some(format!("Cannot launch installation job: {err}")),
                        )
                        .await?;
                        return Err(err);
                    }
                };
                Action::await_change()
            }
            ReconciliationState::Executing => {
                info!(
                    job_name = self.job_name_for("apply"),
                    "waiting for applier job execution"
                );
                Action::await_change()
            }
            ReconciliationState::JobTerminated(job_uid, outcome) => {
                let log_summary = self.capture_logs(ctx, job_uid).await?;

                let action = match outcome {
                    JobOutcome::Success => {
                        info!("job completed successfully");
                        self.update_condition(ctx, "Reconcilier", "True", "Succeeded", None)
                            .await?;
                        self.update_condition(
                            ctx,
                            "Ready",
                            "True",
                            "JobCompletedSuccessfully",
                            None,
                        )
                        .await?;
                        Action::await_change()
                    }
                    JobOutcome::Failure => {
                        info!("job failed");
                        self.update_condition(ctx, "Reconcilier", "True", "Failed", None)
                            .await?;
                        self.update_condition(
                            ctx,
                            "Ready",
                            "False",
                            "JobFailed",
                            Some(log_summary),
                        )
                        .await?;
                        Action::requeue(Duration::from_secs(60))
                    }
                };
                self.delete_job(ctx).await?;
                action
            }
        };

        Ok(action)
    }

    async fn reconcile_delete(&self, ctx: &Context) -> Result<Action> {
        info!(
            name = self.name_any(),
            namespace = self.instance.namespace(),
            "Cleaning up!"
        );
        let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), &self.instance.namespace_any());
        let apply_job_name = self.job_name_for("apply");
        let cleanup_job_name = self.job_name_for("cleanup");

        if let Some(apply_job) = jobs.get_opt(&apply_job_name).await? {
            info!("Deleting the running job");
            self.delete_job(ctx).await?;

            info!("Awaiting termination of {apply_job_name}");
            let job_uid = apply_job.uid().unwrap();
            let cond = await_condition(jobs.clone(), &apply_job_name, is_deleted(&job_uid));

            // Cleaning up the job can take some time and is an idempotent action, so we
            // can requeue if upon failure when an Err is returned.
            if tokio::time::timeout(Duration::from_secs(120), cond)
                .await
                .is_err()
            {
                return Err(Error::ResourceDeletionTimeout);
            } else {
                self.create_cleanup(jobs, &cleanup_job_name, ctx).await?;
                return self.delete_cleanup_hack_configmap(ctx).await;
            }
        }

        info!("No Job found for {apply_job_name}, proceeding to cleanup phase");
        match jobs.get_opt(&cleanup_job_name).await? {
            Some(_) => {
                self.create_cleanup(jobs, &cleanup_job_name, ctx).await?;
                self.delete_cleanup_hack_configmap(ctx).await
            }
            None => self.delete_cleanup_hack_configmap(ctx).await,
        }
    }

    /// Delete the ConfigMap that was used to prune the applyset.
    ///
    /// This diverges slightly from the spawned Job with emit_<command> style
    /// that is used throughout the codebase, as the Job is marked as `Completed`.
    /// This is problematic because we cannot use a `PreStop` hook in order to
    /// run a `kubectl delete configmap` operation.
    ///
    /// From the Kubernetes documentation:
    /// A call to the PreStop hook fails if the container is already in a terminated or completed
    /// state
    ///
    /// For further details see
    /// <https://kubernetes.io/docs/concepts/containers/container-lifecycle-hooks/#container-hooks>
    async fn delete_cleanup_hack_configmap(&self, ctx: &Context) -> Result<Action> {
        let cm_api: Api<ConfigMap> =
            Api::namespaced(ctx.client.clone(), &self.instance.namespace_any());
        let delete_params = DeleteParams::default();
        let cm_name = &delete::cleanup_hack_resource_name(&self.name_any());
        info!("Performing ConfigMap deletion on {cm_name} to finalise cleanup process.");
        cm_api
            .delete(cm_name, &delete_params)
            .await
            .map(|_| ())
            .or_else(|err| match err {
                // ConfigMap has already been deleted or does not exist, so there
                // is nothing to delete.
                kube::Error::Api(ErrorResponse { code: 404, .. }) => Ok(()),
                _ => Err(err),
            })
            .map_err(Error::KubeError)?;
        info!("{cm_name} deleted");
        Ok(Action::await_change())
    }

    async fn create_cleanup(
        &self,
        jobs: Api<Job>,
        job_name: &str,
        ctx: &Context,
    ) -> Result<Action> {
        info!("Setting up RBAC");
        self.setup_namespaced_roles(ctx).await?;
        info!("Creating cleanup job");
        self.launch_cleanup_job(ctx).await?;

        let cond = await_condition(jobs, job_name, is_job_completed());
        info!("Awaiting completion of {job_name}");
        if tokio::time::timeout(Duration::from_secs(120), cond)
            .await
            .is_err()
        {
            Err(Error::ResourceDeletionTimeout)
        } else {
            info!("{job_name} deleted");
            Ok(Action::await_change())
        }
    }

    async fn launch_cleanup_job(&self, ctx: &Context) -> Result<()> {
        let ns = &self.instance.namespace().ok_or(Error::NamespaceRequired)?;
        let cleanup_job_name = format!("kubit-cleanup-{}", &self.name_any());

        let mut volumes = vec![Volume {
            name: "manifests".to_string(),
            empty_dir: Some(Default::default()),
            ..Default::default()
        }];

        if let Some(ref refs) = self.instance.spec.image_pull_secrets {
            let secret_ref = refs
                .iter()
                .exactly_one()
                .map_err(|_| Error::UnsupportedMultipleImagePullSecrets)?;

            let docker_creds = Volume {
                name: "docker".to_string(),
                secret: Some(SecretVolumeSource {
                    secret_name: secret_ref.name.clone(),
                    items: Some(vec![KeyToPath {
                        key: ".dockerconfigjson".to_string(),
                        path: "config.json".to_string(),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            };
            volumes.push(docker_creds);
        }

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
                name: Some(cleanup_job_name),
                namespace: self.instance.namespace().clone(),
                owner_references: self.owned_by(),
                ..Default::default()
            },
            spec: Some(JobSpec {
                backoff_limit: Some(0),
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        service_account: Some(APPLIER_SERVICE_ACCOUNT.to_string()),
                        restart_policy: Some("Never".to_string()),
                        active_deadline_seconds: Some(180),
                        volumes: Some(volumes),
                        init_containers: Some(vec![Container {
                            name: "setup-delete".to_string(),
                            // We need to use the bitnami image to make use of the in built
                            // shell to use the stdout redirection into a file.
                            image: Some(ctx.apply_step_image()),
                            command: Some(vec!["/bin/sh".to_string()]),
                            args: Some(vec![
                                "-c".to_string(),
                                delete::emit_deletion_setup(
                                    &self.instance,
                                    &self.name_any(),
                                    &format!(
                                        "/manifests/cm-{}",
                                        delete::cleanup_hack_resource_name(&self.name_any())
                                    ),
                                    false,
                                )
                                .join(" "),
                            ]),
                            ..container_defaults.clone()
                        }]),
                        containers: vec![Container {
                            name: "cleanup-manifests".to_string(),
                            image: Some(ctx.render_step_image()),
                            command: Some(delete::emit_commandline(
                                &self.instance,
                                &format!(
                                    "/manifests/cm-{}",
                                    delete::cleanup_hack_resource_name(&self.name_any())
                                ),
                                false,
                            )),
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
    async fn reconciliation_state(&self, ctx: &Context) -> Result<ReconciliationState> {
        let ns = self.instance.namespace_any();
        let api: Api<Job> = Api::namespaced(ctx.client.clone(), &ns);
        let job_name = self.job_name_for("apply");
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

    async fn get_image_pull_secrets(&self, ctx: &Context) -> Result<RegistryAuth> {
        info!("getting image pull credentials");

        let secret_name = {
            let Some(ref refs) = self.instance.spec.image_pull_secrets else {
                return Ok(RegistryAuth::Anonymous);
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

        let ns = &self.instance.namespace().ok_or(Error::NamespaceRequired)?;
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

        let reference: Reference = self.instance.spec.package.image.parse()?;
        Ok(docker_config.get_auth(reference.registry())?)
    }

    async fn fetch_package_config(&self, ctx: &Context) -> Result<PackageConfig> {
        let auth = self.get_image_pull_secrets(ctx).await?;
        let res = oci::fetch_package_config(&self.instance, &auth).await?;
        Ok(res)
    }

    fn owned_by(&self) -> Option<Vec<OwnerReference>> {
        // These are effectively duplicated lines of code because
        // controller_owner_ref cares which type it is called on.
        match &self.original {
            AppInstanceLikeResources::AppInstance(ai) => {
                ai.controller_owner_ref(&()).map(|o| vec![o])
            }
            AppInstanceLikeResources::ConfigMap(cm) => {
                cm.controller_owner_ref(&()).map(|o| vec![o])
            }
        }
    }

    async fn setup_cluster_roles(&self, ctx: &Context) -> Result<()> {
        let ns = self.instance.namespace_any();
        let pp = patch_params();
        let crd_name = format!("{APPLIER_SERVICE_ACCOUNT}-crd");

        let metadata = ObjectMeta {
            name: Some(crd_name.clone()),
            namespace: None,
            owner_references: self.owned_by(),
            ..Default::default()
        };

        let role: Api<ClusterRole> = Api::all(ctx.client.clone());
        let res = ClusterRole {
            metadata: metadata.clone(),
            rules: Some(vec![PolicyRule {
                api_groups: Some(
                    ["apiextensions.k8s.io"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
                resources: Some(
                    ["customresourcedefinitions"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
                verbs: ["delete", "create", "patch", "list", "get"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        role.patch(&res.name_any(), &pp, &Patch::Apply(&res))
            .await?;

        let api: Api<ClusterRoleBinding> = Api::all(ctx.client.clone());
        let role_binding = ClusterRoleBinding {
            metadata: metadata.clone(),
            role_ref: RoleRef {
                api_group: "rbac.authorization.k8s.io".to_string(),
                kind: "ClusterRole".to_string(),
                name: crd_name.clone(),
            },
            subjects: Some(vec![Subject {
                kind: "ServiceAccount".to_string(),
                name: APPLIER_SERVICE_ACCOUNT.to_string(),
                namespace: Some(ns),
                ..Default::default()
            }]),
        };
        api.patch(&role_binding.name_any(), &pp, &Patch::Apply(&role_binding))
            .await?;

        Ok(())
    }

    async fn setup_namespaced_roles(&self, ctx: &Context) -> Result<()> {
        let ns = self.instance.namespace_any();
        let pp = patch_params();

        let metadata = ObjectMeta {
            name: Some(APPLIER_SERVICE_ACCOUNT.to_string()),
            namespace: self.instance.namespace().clone(),
            owner_references: self.owned_by(),
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
                verbs: [
                    "create", "update", "get", "list", "patch", "watch", "delete",
                ]
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

    async fn launch_job(&self, ctx: &Context) -> Result<()> {
        self.setup_namespaced_roles(ctx).await?;
        self.setup_cluster_roles(ctx).await?;

        let package_config: PackageConfig = self.fetch_package_config(ctx).await?;
        info!("got package config");

        let kubecfg_image = package_config.versioned_kubecfg_image(&ctx.kubecfg_image)?;
        info!("Using: {}", kubecfg_image);

        self.create_job(kubecfg_image, ctx).await
    }

    fn job_name_for(&self, job_type: &str) -> String {
        format!("kubit-{job_type}-{}", self.name_any())
    }

    async fn delete_job(&self, ctx: &Context) -> Result<()> {
        let ns = &self.instance.namespace().ok_or(Error::NamespaceRequired)?;
        let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
        let name = self.job_name_for("apply");
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

    async fn create_job(&self, kubecfg_image: String, ctx: &Context) -> Result<()> {
        let ns = &self.instance.namespace().ok_or(Error::NamespaceRequired)?;
        let job_name = self.job_name_for("apply");

        let mut volumes = vec![
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

        if let Some(ref refs) = self.instance.spec.image_pull_secrets {
            let secret_ref = refs
                .iter()
                .exactly_one()
                .map_err(|_| Error::UnsupportedMultipleImagePullSecrets)?;

            let volume = Volume {
                name: "docker".to_string(),
                secret: Some(SecretVolumeSource {
                    secret_name: secret_ref.name.clone(),
                    items: Some(vec![KeyToPath {
                        key: ".dockerconfigjson".to_string(),
                        path: "config.json".to_string(),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            };
            volumes.push(volume);
        }

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
                namespace: self.instance.namespace().clone(),
                owner_references: self.owned_by(),
                ..Default::default()
            },
            spec: Some(JobSpec {
                backoff_limit: Some(0),
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        service_account: Some(APPLIER_SERVICE_ACCOUNT.to_string()),
                        restart_policy: Some("Never".to_string()),
                        active_deadline_seconds: Some(180),
                        volumes: Some(volumes),
                        init_containers: Some(
                            self.init_containers(
                                ns,
                                &kubecfg_image,
                                &ctx.kubit_image,
                                &container_defaults,
                            )
                            .await,
                        ),
                        containers: vec![Container {
                            name: "apply-manifests".to_string(),
                            image: Some(ctx.apply_step_image()),
                            command: Some(apply::emit_commandline(
                                &self.instance,
                                "/manifests",
                                &None,
                                false,
                                &ctx.apply_step_image(),
                            )),
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

    async fn capture_logs(&self, ctx: &Context, job_uid: String) -> Result<String> {
        let ns = &self.instance.namespace().ok_or(Error::NamespaceRequired)?;
        info!(?ns, "reporting errors");

        let pods_api: Api<Pod> = Api::namespaced(ctx.client.clone(), ns);
        let job_name = self.job_name_for("apply");

        let pods = pods_api
            .list(&ListParams {
                label_selector: Some(format!("job-name={job_name},controller-uid={job_uid}")),
                ..Default::default()
            })
            .await?;

        let mut per_container_logs = HashMap::new();
        let mut log_summary = String::new();

        // There should be exactly one pod per job. In the unlikely even
        // something is broken with k8s and we end up getting two pods matching the same job uid
        // let's just get the logs of all these pods and concatenate them together. Chances are
        // that this is easier to debug than just getting logs for a random pod.
        //
        // NOTE(mkm): I don't know how likely this is to happen so I'm not sure if it's worth doing
        // something more complicated like capturing the pod names and grouping the logs by pod name.
        for pod in pods.items {
            let mut container_names = vec![];
            let mut failed_container_name = None;

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

                let has_failed = status
                    .state
                    .as_ref()
                    .and_then(|x| x.terminated.as_ref())
                    .map(|x| x.exit_code > 0)
                    .unwrap_or(false);
                if has_failed {
                    failed_container_name = Some(status.name.clone());
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

                if Some(container_name) == failed_container_name.as_ref() {
                    if let Some(last_line) = logs.lines().next() {
                        log_summary.push_str(last_line);
                    }
                    log_summary.push_str("\n...\n");
                    if let Some(last_line) = logs.lines().last() {
                        log_summary.push_str(last_line);
                    }
                }

                per_container_logs
                    .entry(container_name.clone())
                    .and_modify(|e: &mut String| e.push_str(&logs))
                    .or_insert(logs);
            }
            let logs_json =
                serde_json::to_string(&per_container_logs).expect("cannot render basic json");
            info!(logs_json);
        }

        let old_status = self.old_status(ns, ctx).await?;

        self.update_status(
            ctx,
            AppInstanceStatus {
                last_logs: Some(per_container_logs),
                ..old_status
            },
        )
        .await?;
        Ok(log_summary)
    }

    async fn update_condition(
        &self,
        ctx: &Context,
        type_: &str,
        status: &str,
        reason: &str,
        message: Option<String>,
    ) -> Result<()> {
        let ns = &self.instance.namespace().ok_or(Error::NamespaceRequired)?;
        let old_status = self.old_status(ns, ctx).await?;

        let mut conditions = old_status.conditions;
        update_condition_vec(&mut conditions, type_, status, reason, message)?;

        let status = AppInstanceStatus {
            conditions,
            ..old_status
        };

        self.update_status(ctx, status).await
    }

    async fn old_status(&self, ns: &str, ctx: &Context) -> Result<AppInstanceStatus> {
        match self.original {
            AppInstanceLikeResources::AppInstance(_) => {
                let api: Api<AppInstance> = Api::namespaced(ctx.client.clone(), ns);
                Ok(api
                    .get_status(&self.name_any())
                    .await?
                    .status
                    .clone()
                    .unwrap_or_default())
            }
            AppInstanceLikeResources::ConfigMap(_) => {
                let api: Api<ConfigMap> = Api::namespaced(ctx.client.clone(), ns);
                let data = api.get(&self.name_any()).await?.data;
                if let Some(data) = data {
                    let status = data.get("status");
                    if let Some(status) = status {
                        serde_json::from_str::<AppInstanceStatus>(status.as_str())
                            .map_err(Error::InvalidConfigMapStatus)
                    } else {
                        Ok(AppInstanceStatus::default())
                    }
                } else {
                    Ok(AppInstanceStatus::default())
                }
            }
        }
    }

    async fn update_status(&self, ctx: &Context, status: AppInstanceStatus) -> Result<()> {
        let ns = &self.instance.namespace().ok_or(Error::NamespaceRequired)?;

        match self.original {
            AppInstanceLikeResources::AppInstance(_) => {
                let app_instance_api: Api<AppInstance> = Api::namespaced(ctx.client.clone(), ns);

                let app_instance_patch = AppInstance {
                    metadata: Default::default(),
                    spec: Default::default(),
                    status: Some(status),
                };
                app_instance_api
                    .patch_status(
                        &self.name_any(),
                        &patch_params(),
                        &Patch::Apply(&app_instance_patch),
                    )
                    .await?;
            }
            AppInstanceLikeResources::ConfigMap(_) => {
                let config_map_api: Api<ConfigMap> = Api::namespaced(ctx.client.clone(), ns);
                let status_string =
                    serde_json::to_string(&status).map_err(Error::InvalidConfigMapStatus)?;
                let patch = serde_json::json!({
                    "apiVersion": "v1",
                    "kind": "ConfigMap",
                    "data": {
                        "status": status_string,
                    }
                });
                config_map_api
                    .patch(&self.name_any(), &patch_params(), &Patch::Apply(&patch))
                    .await?;
            }
        }
        info!("status patched");

        Ok(())
    }

    async fn init_containers(
        &self,
        ns: &str,
        kubecfg_image: &str,
        kubit_image: &str,
        container_defaults: &Container,
    ) -> Vec<Container> {
        let (command, name) = match self.original {
            AppInstanceLikeResources::AppInstance(_) => (
                render::emit_fetch_app_instance_commandline(
                    ns,
                    &self.name_any(),
                    "/overlay/appinstance.json",
                ),
                "fetch-app-instance",
            ),
            AppInstanceLikeResources::ConfigMap(_) => (
                render::emit_fetch_appinstance_from_config_map_commandline(
                    ns,
                    &self.name_any(),
                    "/overlay/appinstance.json",
                ),
                "fetch-config-map",
            ),
        };
        let fetch_container = Container {
            name: name.to_string(),
            image: Some(kubit_image.to_string()),
            command: Some(command),
            ..container_defaults.clone()
        };
        vec![
            fetch_container,
            Container {
                name: "render-manifests".to_string(),
                image: Some(kubecfg_image.to_string()),
                command: Some(
                    render::emit_commandline(
                        &self.instance,
                        "/overlay/appinstance.json",
                        Some("/manifests"),
                        false,
                        false,
                        kubecfg_image.to_string(),
                    )
                    .await,
                ),
                ..container_defaults.clone()
            },
        ]
    }
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

fn patch_params() -> PatchParams {
    PatchParams::apply("kubit").force()
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

fn update_condition_vec(
    vec: &mut Vec<AppInstanceCondition>,
    type_: &str,
    status: &str,
    reason: &str,
    message: Option<String>,
) -> Result<()> {
    let mut new_condition = AppInstanceCondition {
        message: message.unwrap_or_default(),
        reason: reason.to_string(),
        status: status.to_string(),
        type_: type_.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation: None,
    };
    for i in vec.iter_mut() {
        if i.type_ == type_ {
            if i.status == new_condition.status {
                new_condition.last_transition_time = i.last_transition_time.clone();
            }
            *i = new_condition;
            return Ok(());
        }
    }

    vec.push(new_condition);
    Ok(())
}

#[allow(dead_code)]
fn find_condition(app_instance: &AppInstance, type_: &str) -> Option<AppInstanceCondition> {
    app_instance
        .status
        .as_ref()
        .and_then(|s| s.conditions.iter().find(|i| i.type_ == type_).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manipulate_conditions() {
        let mut conditions = vec![];

        update_condition_vec(
            &mut conditions,
            "Ready",
            "False",
            "WakingUpWithoutCoffee",
            None,
        )
        .unwrap();

        assert_eq!(
            conditions.iter().map(|c| &c.type_).collect::<Vec<_>>(),
            &["Ready"]
        );
        assert_eq!(
            conditions.iter().map(|c| &c.status).collect::<Vec<_>>(),
            &["False"]
        );

        update_condition_vec(
            &mut conditions,
            "Healthy",
            "False",
            "NotReady",
            Some("still waking up".to_string()),
        )
        .unwrap();

        assert_eq!(
            conditions.iter().map(|c| &c.type_).collect::<Vec<_>>(),
            &["Ready", "Healthy"]
        );
        assert_eq!(
            conditions.iter().map(|c| &c.status).collect::<Vec<_>>(),
            &["False", "False"]
        );

        update_condition_vec(
            &mut conditions,
            "Ready",
            "True",
            "ReconciliationSucceeded",
            None,
        )
        .unwrap();

        assert_eq!(
            conditions.iter().map(|c| &c.type_).collect::<Vec<_>>(),
            &["Ready", "Healthy"]
        );
        assert_eq!(
            conditions.iter().map(|c| &c.status).collect::<Vec<_>>(),
            &["True", "False"]
        );

        let prev_transition = conditions[0].last_transition_time.clone();
        let prev_message = conditions[0].message.clone();
        update_condition_vec(
            &mut conditions,
            "Ready",
            "True",
            "ReconciliationSucceeded",
            Some("message change doesn't cause transition time change".to_string()),
        )
        .unwrap();
        let next_transition = conditions[0].last_transition_time.clone();
        let next_message = conditions[0].message.clone();

        assert_eq!(prev_transition, next_transition);
        assert_ne!(prev_message, next_message);

        update_condition_vec(
            &mut conditions,
            "Ready",
            "False",
            "EverythingIsBroken",
            None,
        )
        .unwrap();

        let next_transition = conditions[0].last_transition_time.clone();

        assert!(prev_transition < next_transition);
    }
}
