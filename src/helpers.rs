use std::fs::File;

use anyhow::Result;
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

    FetchAppInstanceFromConfigMap {
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
            app_instance.metadata.managed_fields = None;

            let file = File::create(output)?;
            serde_json::to_writer_pretty(file, &app_instance)?;
        }

        Helper::FetchAppInstanceFromConfigMap {
            namespace,
            config_map,
            output,
        } => {
            let client = Client::try_default().await?;
            let api: Api<ConfigMap> = Api::namespaced(client, namespace);
            let cm = api.get(config_map).await?;

            let data = cm.data.ok_or(anyhow::anyhow!(
                "ConfigMap {} did not have a data field",
                &config_map
            ))?;

            let app_instance = data.get("app-instance").ok_or(anyhow::anyhow!(
                "ConfigMap {} data did not have an app-instance field",
                &config_map
            ))?;

            let ai: AppInstance = serde_yaml::from_str(app_instance)?;

            let file = File::create(output)?;
            serde_yaml::to_writer(file, &ai)?;
        }
    }
    Ok(())
}
