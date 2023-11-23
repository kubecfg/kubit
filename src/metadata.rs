use anyhow::{bail, Result};
use clap::Subcommand;
use docker_credential::DockerCredential;
use oci_distribution::{secrets::RegistryAuth, Reference};
use std::fs::File;

use crate::{
    oci::{self, PackageConfig},
    resources::AppInstance,
};

#[derive(Clone, Subcommand)]
pub enum Metadata {
    /// Retrieve the JSON schema for the package `spec`.
    Schema {
        app_instance: String,
        #[clap(long)]
        skip_auth: bool,
    },

    /// Retrieve the list of OCI images referenced by the package.
    /// It can be useful when using private mirror for air-gapped environments.
    Images {
        app_instance: String,
        #[clap(long)]
        skip_auth: bool,
    },
}

pub async fn run(schema: &Metadata) -> Result<()> {
    match schema {
        Metadata::Schema {
            app_instance,
            skip_auth,
        } => {
            let config = fetch_package_config_from_file(app_instance, *skip_auth).await?;
            let schema = config.schema()?;
            println!("{schema}");
        }
        Metadata::Images {
            app_instance,
            skip_auth,
        } => {
            let config = fetch_package_config_from_file(app_instance, *skip_auth).await?;
            let images = config.images();
            for image in images? {
                println!("{image}");
            }
        }
    };
    Ok(())
}

async fn fetch_package_config_from_file(
    app_instance: &str,
    skip_auth: bool,
) -> Result<PackageConfig> {
    let file = File::open(app_instance)?;
    let app_instance: AppInstance = serde_yaml::from_reader(file)?;
    fetch_package_config_local_auth(&app_instance, skip_auth).await
}

pub async fn fetch_package_config_local_auth(
    app_instance: &AppInstance,
    skip_auth: bool,
) -> Result<PackageConfig> {
    let reference: Reference = app_instance.spec.package.image.parse()?;
    let auth = if skip_auth {
        RegistryAuth::Anonymous
    } else {
        let credentials = docker_credential::get_credential(reference.registry())?;
        let DockerCredential::UsernamePassword(username, password) = credentials else {
            bail!("unsupported docker credentials")
        };
        RegistryAuth::Basic(username, password)
    };
    let config = oci::fetch_package_config(app_instance, &auth).await?;
    Ok(config)
}
