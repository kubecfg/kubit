use std::collections::HashMap;

use kube::CustomResource;
use schemars::{
    schema::{Schema, SchemaObject},
    JsonSchema,
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

// Like k8s_openapi::api::core::v1::LocalObjectReference but derives JsonSchema
// so we can use it here.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct LocalObjectReference {
    pub name: Option<String>,
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
}
