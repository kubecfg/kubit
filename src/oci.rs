use std::collections::HashMap;

use oci_distribution::{manifest::OciManifest, secrets::RegistryAuth, Client, Reference};
use serde::{Deserialize, Serialize};

use crate::resources::AppInstance;

const PACK_KEY: &str = "pack.kubecfg.dev/v1alpha1";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Unsupported manifest type: Index")]
    UnsupportedManifestIndex,

    #[error("Error decoding package config JSON: {0}")]
    DecodePackageConfig(serde_json::Error),

    #[error("Error decoding kubecfg pack metadata JSON: {0}")]
    DecodeKubecfgPackageMetadata(serde_json::Error),

    #[error("OCI error: {0}")]
    OciParse(#[from] oci_distribution::ParseError),

    #[error("OCI error: {0}")]
    Oci(#[from] oci_distribution::errors::OciDistributionError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageConfig {
    entrypoint: String,
    #[serde(default)]
    metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KubecfgPackageMetadata {
    version: String,
}

impl PackageConfig {
    pub fn kubecfg_package_metadata(&self) -> Result<KubecfgPackageMetadata> {
        serde_json::from_value(self.metadata.get(PACK_KEY).unwrap().clone())
            .map_err(Error::DecodeKubecfgPackageMetadata)
    }

    pub fn versioned_kubecfg_image(&self, kubecfg_image: &str) -> Result<String> {
        let kubecfg_version = &self.kubecfg_package_metadata()?.version;
        Ok(format!("{}:{kubecfg_version}", kubecfg_image))
    }

    pub fn schema(&self) -> String {
        serde_json::to_string_pretty(
            self.metadata
                .get("kubit.kubecfg.dev/v1alpha1")
                .unwrap()
                .get("schema")
                .unwrap(),
        )
        .unwrap()
    }

    pub fn images(&self) -> Vec<String> {
        serde_json::from_value(
            self.metadata
                .get("oci.image.list")
                .unwrap()
                .get("images")
                .unwrap()
                .clone(),
        )
        .unwrap()
    }
}

pub async fn fetch_package_config(
    app_instance: &AppInstance,
    auth: &RegistryAuth,
) -> Result<PackageConfig> {
    let image = &app_instance.spec.package.image;

    let client_config = oci_distribution::client::ClientConfig {
        protocol: oci_distribution::client::ClientProtocol::Https,
        ..Default::default()
    };
    let mut client = Client::new(client_config);
    let reference: Reference = image.parse()?;
    let (manifest, _) = client.pull_manifest(&reference, auth).await?;

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
