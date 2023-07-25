use kube::CustomResource;
use schemars::JsonSchema;
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
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub image: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppInstanceStatus {}
