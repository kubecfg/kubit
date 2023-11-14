use std::fs::File;

use anyhow::Result;
use clap::Subcommand;
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

    /// Initiate the cleanup process by leveraging an empty applyset.
    ///
    /// A single resource, a blank ConfigMap from the Namespace that the AppInstance resides within, is
    /// written into a file that will ensure that all resources are automatically pruned.
    Cleanup {
        #[arg(long)]
        namespace: String,

        #[arg(long, help = "output file")]
        output: String,
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
        Helper::Cleanup { namespace, output } => {
            create_cleanup_cm(namespace, &format!("{namespace}-cleanup"), output)?;
        }
    }
    Ok(())
}

/// Write a blank ConfigMap to a file. This is used as a utility to help cleanup
/// resources by leveraging the applyset functionality.
///
/// Unfortunately, we cannot use a blank object of kind `List` as the applyset
/// requires that _some_ objects are passed to it.
pub fn create_cleanup_cm(
    namespace: &String,
    configmap_name: &String,
    output: &String,
) -> Result<()> {
    let file = File::create(output)?;

    serde_json::to_writer_pretty(
        file,
        &serde_json::json!(
        {
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {
                "name": configmap_name,
                "namespace": namespace,
            }
        }),
    )?;

    Ok(())
}
