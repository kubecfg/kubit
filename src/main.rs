#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use std::{
    fs::File,
    io::{stdout, Write},
    path::PathBuf,
};

use clap::{Parser, Subcommand};
use kube::CustomResourceExt;

use kubit::{apply, controller, render, resources::AppInstance};

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

        /// Render scripts for various phases
        Scripts {
            #[command(subcommand)]
            script: Scripts,
        },
    }

    #[derive(Clone, Parser)]
    struct ScriptArgs {
        /// Path to the file containing a (YAML) AppInstance manifest.
        app_instance: String,
    }

    #[derive(Clone, Subcommand)]
    enum Scripts {
        /// Render manifests
        Render(ScriptArgs),
        /// Apply manifests
        Apply(ScriptArgs),
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
                let mut out_writer: Box<dyn Write> = match crd_dir {
                    Some(dir) => {
                        let crd_file = format!("{}_{}.yaml", crd.spec.group, crd.spec.names.plural);
                        let file = File::create(dir.join(crd_file))
                            .expect("Could not open AppInstances CRD file");
                        Box::new(file)
                    }
                    None => Box::new(stdout()),
                };
                // The YAML delimiter is added in the event we have multiple documents.
                writeln!(out_writer, "---").unwrap();
                serde_yaml::to_writer(out_writer, &crd).unwrap();
            }
        }
        Some(Commands::Scripts { script }) => {
            fn app_instance(args: &ScriptArgs) -> anyhow::Result<AppInstance> {
                Ok(serde_yaml::from_reader(File::open(&args.app_instance)?)?)
            }
            let mut output = stdout().lock();
            match script {
                Scripts::Render(args) => render::emit_script(&app_instance(args)?, &mut output)?,
                Scripts::Apply(args) => apply::emit_script(&app_instance(args)?, &mut output)?,
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
