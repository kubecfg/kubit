use futures::StreamExt;
use std::{collections::HashMap, sync::Arc, time::Duration};

use kube::{
    api::ListParams,
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
    kubecfg_image: String,
}

fn error_policy(sinker: Arc<AppInstance>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = sinker.name_any();
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
        .run(reconcile, error_policy, Arc::new(Context { kubecfg_image }))
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

    let client_config = oci_distribution::client::ClientConfig {
        protocol: oci_distribution::client::ClientProtocol::Https,
        ..Default::default()
    };
    let mut client = OCIClient::new(client_config);
    let (manifest, _) = client
        .pull_manifest(&reference, &RegistryAuth::Anonymous)
        .await?;

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

    let kubecfg_version = &kubecfg_pack_metadata.version;
    let kubecfg_image = format!("{}:{kubecfg_version}", ctx.kubecfg_image);

    let mut buf = vec![];
    render::emit_script(&app_instance, &mut buf)?;
    let script = String::from_utf8(buf).unwrap(); // for debug only

    info!(
        ?kubecfg_image,
        script, "TODO: create a Job that runs kubecfg and renders the jsonnet artifact"
    );

    Ok(Action::await_change())
}
