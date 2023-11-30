use std::fs::File;

use anyhow::{anyhow, Result};
use clap::Subcommand;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{Api, Client};

use crate::resources::AppInstance;

/// Commands used by the kubit controller
#[derive(Clone, Subcommand)]
#[clap(hide = true)]
pub enum Helper {
    /// Fetch an AppInstance resource and output to a file.
    ///
    /// It removes the status field.
    FetchAppInstance {
        #[arg(long)]
        namespace: String,

        #[arg(long, help = "output file")]
        output: String,

        app_instance: String,
    },

    FetchConfigMap {
        #[arg(long)]
        namespace: String,

        #[arg(long, help = "output file")]
        output: String,

        config_map: String,
    },
}

pub async fn run(helper: &Helper) -> Result<()> {
    match helper {
        Helper::FetchAppInstance {
            namespace,
            app_instance,
            output,
        } => {
            let client = Client::try_default().await?;
            let api: Api<AppInstance> = Api::namespaced(client, namespace);
            let mut app_instance = api.get(app_instance).await?;
            app_instance.status = None;

            let file = File::create(output)?;
            serde_json::to_writer_pretty(file, &app_instance)?;
        }

        Helper::FetchConfigMap {
            namespace,
            config_map,
            output,
        } => {
            let client = Client::try_default().await?;
            let api: Api<ConfigMap> = Api::namespaced(client, namespace);
            let config_map = api.get(config_map).await?;

            let data = config_map.data.ok_or(anyhow!("config map did not have a data field"))?;
            let app_instance = data.get("app-instance").ok_or(anyhow!("config map is missing app-instance key"))?;

            let app_instance: AppInstance = serde_yaml::from_str(app_instance)?;

            let file = File::create(output)?;
            serde_json::to_writer_pretty(file, &app_instance)?;
        }
    }
    Ok(())
}
