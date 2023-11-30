use std::collections::HashMap;
use std::sync::Arc;

use k8s_openapi::{api::core::v1::LocalObjectReference, apimachinery::pkg::apis::meta::v1::Time};
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{CustomResource, ResourceExt};
use schemars::{
    JsonSchema,
    schema::{Schema, SchemaObject},
};
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[kube(
group = "kubecfg.dev",
version = "v1alpha1",
kind = "AppInstance",
namespaced
)]
#[kube(status = "AppInstanceStatus")]
#[serde(rename_all = "camelCase")]
pub struct AppInstanceSpec {
    pub package: Package,
    pub image_pull_secrets: Option<Vec<LocalObjectReference>>,

    /// If true, the controller will not reconcile this application.
    /// You can use this if you need to do some manual changes (either with kubectl directly or with kubit CLI)
    #[serde(default)]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pause: bool,
}

impl AppInstance {
    pub fn namespace_any(&self) -> String {
        self.namespace().unwrap_or_default()
    }
}

pub enum AppInstanceLikeResources {
    AppInstance(Arc<AppInstance>),
    ConfigMap(Arc<ConfigMap>),
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub image: String,
    pub api_version: String,
    pub spec: PackageSpec,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackageSpec {
    #[serde(flatten)]
    #[schemars(schema_with = "preserve_arbitrary")]
    arbitrary: HashMap<String, serde_json::Value>,
}

fn preserve_arbitrary(_gen: &mut schemars::gen::SchemaGenerator) -> Schema {
    let mut obj = SchemaObject::default();
    obj.extensions
        .insert("x-kubernetes-preserve-unknown-fields".into(), true.into());
    Schema::Object(obj)
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppInstanceStatus {
    pub last_logs: Option<HashMap<String, String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub conditions: Vec<AppInstanceCondition>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppInstanceCondition {
    pub last_transition_time: Time,
    pub message: String,
    #[serde(default)]
    pub observed_generation: Option<i64>,
    pub reason: String,
    pub status: String,
    pub type_: String,
}
