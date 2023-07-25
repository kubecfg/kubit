#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use std::{fs::File, path::PathBuf};

use clap::{Parser, Subcommand};
use kube::CustomResourceExt;

use kubit::controller;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[derive(Clone, Parser)]
    #[clap(version)]
    struct Args {
        /// The tracing filter used for logs
        #[clap(long, env = "KUBIT_LOG", default_value = "kubit=info,warn")]
        log_level: kubert::LogFilter,

        /// The logging format
        #[clap(long, default_value = "plain")]
        log_format: kubert::LogFormat,

        #[clap(flatten)]
        client: kubert::ClientArgs,

        #[clap(flatten)]
        admin: kubert::AdminArgs,

        #[command(subcommand)]
        command: Option<Commands>,
    }

    #[derive(Clone, Subcommand)]
    enum Commands {
        /// Generates k8s manifests
        Manifests {
            // Optional directory to write CRDs into
            #[clap(
                long,
                help = "Optional directory to write CRDs into, otherwise write to stdout"
            )]
            crd_dir: Option<PathBuf>,
        },
    }

    let Args {
        log_level,
        log_format,
        client,
        admin,
        command,
    } = Args::parse();

    match &command {
        Some(Commands::Manifests { crd_dir }) => match crd_dir {
            Some(crd_dir) => {
                // Expand vector as more CRDs are created.
                for crd in vec![&kubit::resources::AppInstance::crd()] {
                    let crd_file = format!("{}_{}.yaml", crd.spec.group, crd.spec.names.plural);
                    let crd_path = PathBuf::from(crd_dir).join(crd_file);

                    let file =
                        File::create(crd_path).expect("Could not open AppInstances CRD file");

                    serde_yaml::to_writer(&file, &kubit::resources::AppInstance::crd())?;
                }
            }
            None => println!(
                "{}",
                serde_yaml::to_string(&kubit::resources::AppInstance::crd()).unwrap(),
            ),
        },
        None => {
            let mut admin = kubert::admin::Builder::from(admin);
            admin.with_default_prometheus();

            let rt = kubert::Runtime::builder()
                .with_log(log_level, log_format)
                .with_admin(admin)
                .with_client(client)
                .build()
                .await?;

            let controller = controller::run(rt.client());

            // Both runtimes implements graceful shutdown, so poll until both are done
            tokio::join!(controller, rt.run()).1?;
        }
    }

    Ok(())
}
