use std::collections::HashMap;

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const APPINSTANCE_CRD_FILE: &str = "kubecfg.dev_appinstances.yaml";

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
    pub spec: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppInstanceStatus {}
