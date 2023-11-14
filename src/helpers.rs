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

    // Use the applyset to initiate the cleanup operation.
    // This will remove all resources created by the AppInstance.
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
            let file = File::create(output)?;

            // As we use the kubectl applyset functionality, we can pass the namespace
            // that the resources reside it to cleanup everything.
            serde_json::to_writer_pretty(
                file,
                &serde_json::json!({
                "apiVersion": "v1",
                "kind": "Namespace",
                "metadata": {
                    "name": namespace
                }
                }
                ),
            )?;
        }
    }
    Ok(())
}
