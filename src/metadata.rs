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
    Schema { app_instance: String },
    Images { app_instance: String },
}

pub async fn run(schema: &Metadata) -> Result<()> {
    match schema {
        Metadata::Schema { app_instance } => {
            let config = fetch_package_config(app_instance).await?;
            let schema = config.schema();
            println!("{schema}");
        }
        Metadata::Images { app_instance } => {
            let config = fetch_package_config(app_instance).await?;
            let images = config.images();
            for image in images {
                println!("{image}");
            }
        }
    };
    Ok(())
}

async fn fetch_package_config(app_instance: &str) -> Result<PackageConfig> {
    let file = File::open(app_instance)?;
    let app_instance: AppInstance = serde_yaml::from_reader(file)?;
    let reference: Reference = app_instance.spec.package.image.parse()?;
    let credentials = docker_credential::get_credential(reference.registry())?;
    let DockerCredential::UsernamePassword(username, password ) = credentials else {bail!("unsupported docker credentials")};
    let auth = RegistryAuth::Basic(username, password);

    let config = oci::fetch_package_config(&app_instance, &auth).await?;
    Ok(config)
}
