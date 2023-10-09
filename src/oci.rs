use std::collections::HashMap;

use oci_distribution::{manifest::OciManifest, secrets::RegistryAuth, Client, Reference};
use serde::{Deserialize, Serialize};

use crate::resources::AppInstance;

const PACK_KEY: &str = "pack.kubecfg.dev/v1alpha1";
const KUBIT_KEY: &str = "kubit.kubecfg.dev/v1alpha1";
const IMAGE_LIST_KEY: &str = "oci.image.list";

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

    #[error("Missing metadata key: pack.kubecfg.dev/v1alpha1")]
    MissingMetadataKeyPack,

    #[error("Missing metadata key: kubit.kubecfg.dev/v1alpha1")]
    MissingMetadataKeyKubit,

    #[error("Missing metadata key: schema under kubit.kubecfg.dev/v1alpha1")]
    MissingMetadataKeyKubitSchema,

    #[error("Missing metadata key: oci.image.list")]
    MissingMetadataKeyImageList,

    #[error("Missing metadata key: images under oci.image.list")]
    MissingMetadataKeyImageListImages,

    #[error("Error serializing JSON schema: {0}")]
    SerializeJSONSchema(serde_json::Error),

    #[error("Error serializing image list: {0}")]
    SerializeImageList(serde_json::Error),
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
        serde_json::from_value(
            self.metadata
                .get(PACK_KEY)
                .ok_or(Error::MissingMetadataKeyPack)?
                .clone(),
        )
        .map_err(Error::DecodeKubecfgPackageMetadata)
    }

    pub fn versioned_kubecfg_image(&self, kubecfg_image: &str) -> Result<String> {
        let kubecfg_version = &self.kubecfg_package_metadata()?.version;
        Ok(format!("{}:{kubecfg_version}", kubecfg_image))
    }

    pub fn schema(&self) -> Result<String> {
        serde_json::to_string_pretty(
            self.metadata
                .get(KUBIT_KEY)
                .ok_or(Error::MissingMetadataKeyKubit)?
                .get("schema")
                .ok_or(Error::MissingMetadataKeyKubitSchema)?,
        )
        .map_err(Error::SerializeJSONSchema)
    }

    pub fn images(&self) -> Result<Vec<String>> {
        serde_json::from_value(
            self.metadata
                .get(IMAGE_LIST_KEY)
                .ok_or(Error::MissingMetadataKeyImageList)?
                .get("images")
                .ok_or(Error::MissingMetadataKeyImageListImages)?
                .clone(),
        )
        .map_err(Error::SerializeImageList)
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
