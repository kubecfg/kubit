#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use std::{
    fs::File,
    io::{stdout, Write},
    path::PathBuf,
};

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

        #[clap(long, default_value = "ghcr.io/kubecfg/kubecfg/kubecfg")]
        kubecfg_image: String,

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
        kubecfg_image,
        command,
    } = Args::parse();

    // Expand vector as more CRDs are created.
    let crds = vec![kubit::resources::AppInstance::crd()];
    match &command {
        Some(Commands::Manifests { crd_dir }) => {
            for crd in crds {
                let mut out_writer = match crd_dir {
                    Some(dir) => {
                        let crd_file = format!("{}_{}.yaml", crd.spec.group, crd.spec.names.plural);
                        let crd_path = PathBuf::from(dir).join(crd_file);
                        let file =
                            File::create(crd_path).expect("Could not open AppInstances CRD file");
                        Box::new(file) as Box<dyn Write>
                    }
                    None => Box::new(stdout()) as Box<dyn Write>,
                };
                // The YAML delimiter is added in the event we have multiple documents.
                out_writer.write(b"---\n").unwrap();
                serde_yaml::to_writer(out_writer, &crd).unwrap();
            }
        }
        None => {
            let mut admin = kubert::admin::Builder::from(admin);
            admin.with_default_prometheus();

            let rt = kubert::Runtime::builder()
                .with_log(log_level, log_format)
                .with_admin(admin)
                .with_client(client)
                .build()
                .await?;

            let controller = controller::run(rt.client(), kubecfg_image);

            // Both runtimes implements graceful shutdown, so poll until both are done
            tokio::join!(controller, rt.run()).1?;
        }
    }

    Ok(())
}
