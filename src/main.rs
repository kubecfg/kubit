#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use std::{
    fs::File,
    io::{stdout, Write},
    path::PathBuf,
};

use clap::{Parser, Subcommand};
use kube::CustomResourceExt;

use kubit::{apply, controller, helpers, local, metadata, render, resources::AppInstance};

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

        /// Kubectl image to use for the apply step of kubit.
        ///
        /// This MUST be greater than v1.27 of kubectl as we utilise the applyset
        /// functionality
        #[clap(
            name = "apply-step-image",
            long,
            env = "KUBIT_APPLY_STEP_KUBECTL_IMAGE",
            default_value = apply::DEFAULT_APPLY_KUBECTL_IMAGE
        )]
        apply_image_kubectl: String,

        /// Kubectl image to use for the render step of kubit.
        ///
        /// This MUST be greater than v1.27 of kubectl as we utilise the applyset
        /// functionality
        #[clap(
            name = "render-step-image",
            long,
            env = "KUBIT_RENDER_STEP_KUBECTL_IMAGE",
            default_value = crate::controller::KUBECTL_IMAGE
        )]
        render_image_kubectl: String,

        /// Kubecfg image to use within the render step
        #[clap(
            long,
            env = "KUBIT_KUBECFG_IMAGE",
            default_value = render::DEFAULT_KUBECFG_IMAGE
        )]
        kubecfg_image: String,

        #[clap(
            long,
            env = "KUBIT_CONTROLLER_IMAGE",
           default_value = concat!("ghcr.io/kubecfg/kubit:v", env!("CARGO_PKG_VERSION"))
        )]
        kubit_image: String,

        /// If true, processes only paused instances
        #[clap(long, default_value = "false")]
        only_paused: bool,

        #[command(subcommand)]
        command: Option<Commands>,

        #[clap(long, env = "KUBIT_WATCHED_NAMESPACE", default_value = None)]
        watched_namespace: Option<String>,

        #[clap(long, default_value = "app-instance")]
        config_map_name: Option<String>,
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
            /// Path to the file containing a (YAML) AppInstance manifest.
            #[clap(long)]
            app_instance: String,

            #[clap(long)]
            skip_auth: bool,

            #[command(subcommand)]
            script: Scripts,
        },

        /// Run operator logic locally from the CLI
        Local {
            #[command(subcommand)]
            local: local::Local,
        },

        /// Retreive metadata from an AppInstance's package
        Metadata {
            #[command(subcommand)]
            metadata: metadata::Metadata,
        },

        Helper {
            #[command(subcommand)]
            helper: helpers::Helper,
        },
    }

    #[derive(Clone, Subcommand)]
    enum Scripts {
        /// Render manifests
        Render,
        /// Apply manifests
        Apply,
    }

    let Args {
        log_level,
        log_format,
        client,
        admin,
        kubecfg_image,
        kubit_image,
        apply_image_kubectl,
        render_image_kubectl,
        command,
        only_paused,
        watched_namespace,
        config_map_name,
    } = Args::parse();

    // Expand vector as more CRDs are created.
    let crds = vec![kubit::resources::AppInstance::crd()];
    match &command {
        Some(Commands::Manifests { crd_dir }) => {
            for crd in crds {
                let mut out_writer: Box<dyn Write> = match crd_dir {
                    Some(dir) => {
                        let crd_file = format!("{}_{}.yaml", crd.spec.group, crd.spec.names.plural);
                        let file = File::create(dir.join(crd_file)).map_err(|e| {
                            anyhow::anyhow!("Could not open AppInstances CRD file: {e}")
                        })?;

                        Box::new(file)
                    }
                    None => Box::new(stdout()),
                };
                // The YAML delimiter is added in the event we have multiple documents.
                writeln!(out_writer, "---")?;
                serde_yaml::to_writer(out_writer, &crd)?;
            }
        }
        Some(Commands::Metadata { metadata }) => metadata::run(metadata).await?,
        Some(Commands::Local { local }) => local::run(local, &client.impersonate_user).await?,
        Some(Commands::Helper { helper }) => helpers::run(helper).await?,
        Some(Commands::Scripts {
            app_instance,
            script,
            skip_auth,
        }) => {
            let file = File::open(app_instance)?;
            let app_instance: AppInstance = serde_yaml::from_reader(file)?;
            let mut output = stdout().lock();
            match script {
                Scripts::Render => {
                    render::emit_script(
                        &app_instance,
                        false,
                        *skip_auth,
                        kubecfg_image,
                        &mut output,
                    )
                    .await?
                }
                Scripts::Apply => {
                    apply::emit_script(&app_instance, false, &apply_image_kubectl, &mut output)?
                }
            }
        }
        None => {
            let prom = prometheus_client::registry::Registry::default();

            let admin = kubert::admin::Builder::from(admin).with_prometheus(prom);

            let rt = kubert::Runtime::builder()
                .with_log(log_level, log_format)
                .with_admin(admin)
                .with_client(client)
                .build()
                .await?;

            let controller = controller::run(
                rt.client(),
                kubecfg_image,
                kubit_image,
                apply_image_kubectl,
                render_image_kubectl,
                only_paused,
                config_map_name,
                watched_namespace,
            );

            // Both runtimes implements graceful shutdown, so poll until both are done
            tokio::join!(controller, rt.run()).1?;
        }
    }

    Ok(())
}
