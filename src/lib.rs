#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Kube Error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("{0}")]
    OCI(#[from] oci::Error),

    #[error("OCI error: {0}")]
    OCIParseError(#[from] oci_distribution::ParseError),

    #[error("Unsupported manifest type: Index")]
    UnsupportedManifestIndex,

    #[error("Error decoding package config JSON: {0}")]
    DecodePackageConfig(serde_json::Error),

    #[error("Error decoding kubecfg pack metadata JSON: {0}")]
    DecodeKubecfgPackageMetadata(serde_json::Error),

    #[error("Error rendering spec back as JSON: {0}")]
    RenderOverlay(serde_json::Error),

    #[error("IO Error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Internal error: {0}")]
    TempfilePersistError(#[from] tempfile::PersistError),

    #[error("Namespace is required")]
    NamespaceRequired,

    #[error(".spec.imagePullSecret currently requires to have exactly one pull secret")]
    UnsupportedMultipleImagePullSecrets,

    #[error("Image pull secret doesn't contain .dockerconfigjson")]
    NoDockerConfigJsonInImagePullSecret,

    #[error("Error decoding docker config JSON: {0}")]
    DecodeDockerConfig(#[from] docker_config::Error),

    #[error("Unsupported image pull secret type: {0:?}, should be kubernetes.io/dockerconfigjson")]
    BadImagePullSecretType(Option<String>),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// GitHub Registry which contains the `kubecfg` image.
pub const KUBECFG_REGISTRY: &str = "ghcr.io/kubecfg/kubecfg/kubecfg";

/// Expose all controller components used by main.
pub mod controller;

/// Resource type definitions.
pub mod resources;

pub mod apply;
pub mod helpers;
pub mod local;
pub mod metadata;
pub mod render;
mod scripting;

mod docker_config;
mod oci;
