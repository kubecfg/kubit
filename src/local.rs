use anyhow::{bail, Result};
use clap::Subcommand;
use kube::ResourceExt;
use std::fs::{self, File};
use std::io;
use std::io::{stdout, IsTerminal, Read, Write};
use std::os::unix::prelude::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use tempfile::{NamedTempFile, TempDir};

use crate::delete::cleanup_hack_resource_name;
use crate::Error;
use crate::{
    apply::{self, KUBIT_APPLIER_FIELD_MANAGER},
    delete, render,
    resources::AppInstance,
    scripting::Script,
};

#[derive(Clone, Subcommand)]
pub enum Local {
    /// Applies the template locally
    Apply {
        /// Path to the file containing a (YAML) AppInstance manifest.
        app_instance: String,

        /// Dry run
        #[clap(long)]
        dry_run: Option<DryRun>,

        /// Show diff before applying. If in tty, interactively ask if you want to continue.
        #[clap(long("diff"), default_value = "false")]
        pre_diff: bool,

        /// Allow anonymous authentication to an OCI registry, e.g. to public registries.
        #[clap(long, default_value = "false")]
        skip_auth: bool,

        /// Use Docker containers for dependencies, rather than relying on locally installed
        /// versions.
        #[clap(long, default_value = "false")]
        docker: bool,

        /// Override the package image field in the spec
        #[clap(long)]
        package_image: Option<String>,

        /// Override the kubectl image
        ///
        /// This MUST be greater than 1.27.0
        #[clap(long, default_value = apply::DEFAULT_APPLY_KUBECTL_IMAGE)]
        apply_step_image: String,

        /// Override the image for kubecfg
        #[clap(long, default_value = render::DEFAULT_KUBECFG_IMAGE)]
        kubecfg_image: String,
    },

    /// Delete the resources created by a packaged AppInstance.
    ///
    /// This removes all created resource, except the containing Namespace as it
    /// is created outside of an applyset.
    Delete {
        /// Path to the file containing a (YAML) AppInstance manifest.
        app_instance: String,

        /// Dry run
        #[clap(long)]
        dry_run: Option<DryRun>,

        /// Use Docker containers for dependencies, rather than relying on locally installed
        /// versions.
        #[clap(long, default_value = "false")]
        docker: bool,
    },
}

#[derive(Clone, clap::ValueEnum, Debug)]
pub enum DryRun {
    Render,
    Diff,
    Script,
}

impl std::fmt::Display for DryRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dry_run = match self {
            DryRun::Diff => "diff".to_string(),
            DryRun::Script => "script".to_string(),
            DryRun::Render => "render".to_string(),
        };
        write!(f, "{}", dry_run)
    }
}

pub async fn run(local: &Local, impersonate_user: &Option<String>) -> Result<()> {
    match local {
        Local::Apply {
            app_instance,
            dry_run,
            package_image,
            pre_diff,
            skip_auth,
            docker,
            apply_step_image,
            kubecfg_image,
        } => {
            apply(
                app_instance,
                dry_run,
                package_image,
                impersonate_user,
                *pre_diff,
                *docker,
                *skip_auth,
                apply_step_image.to_string(),
                kubecfg_image.to_string(),
            )
            .await?;
        }
        Local::Delete {
            app_instance,
            docker,
            dry_run,
        } => delete(app_instance, *docker, dry_run).await?,
    };
    Ok(())
}

/// This trait allows us to close the temporary file but not delete it yet
trait DeferredDelete {
    fn close(self: Box<Self>) -> std::io::Result<Option<DeferredDeleteHandle>>;
}

struct DeferredDeleteHandle {
    path: PathBuf,
}

impl Drop for DeferredDeleteHandle {
    fn drop(&mut self) {
        fs::remove_file(self.path.clone()).unwrap()
    }
}

impl DeferredDelete for NamedTempFile {
    fn close(self: Box<Self>) -> std::io::Result<Option<DeferredDeleteHandle>> {
        let path = self.path().to_path_buf();
        self.persist(path.clone())?;
        Ok(Some(DeferredDeleteHandle { path }))
    }
}

struct NopDeferredDelete<W>(W);

impl<W> DeferredDelete for NopDeferredDelete<W> {
    fn close(self: Box<Self>) -> std::io::Result<Option<DeferredDeleteHandle>> {
        Ok(None)
    }
}

impl<W> Write for NopDeferredDelete<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

trait WriteClose: Write + DeferredDelete {}
impl<W: Write> WriteClose for NopDeferredDelete<W> {}
impl WriteClose for NamedTempFile {}

/// Generate a script that runs kubecfg show and kubectl apply and runs it.
#[allow(clippy::too_many_arguments)]
pub async fn apply(
    app_instance: &str,
    dry_run: &Option<DryRun>,
    package_image: &Option<String>,
    impersonate_user: &Option<String>,
    pre_diff: bool,
    docker: bool,
    skip_auth: bool,
    kubectl_image: String,
    kubecfg_image: String,
) -> Result<()> {
    let (output, path) = get_script(dry_run)?;

    let overlay_file_name = app_instance;
    let file = File::open(overlay_file_name)?;
    let mut app_instance: AppInstance = serde_yaml::from_reader(file)?;

    if let Some(package_image) = package_image {
        app_instance.spec.package.image.clone_from(package_image);
    }

    if pre_diff {
        if dry_run.is_some() {
            bail!("--diff and --dry-run are mutually exclusive");
        }
        prediff(
            overlay_file_name,
            dry_run,
            package_image,
            impersonate_user,
            docker,
            skip_auth,
            kubectl_image.clone(),
            kubecfg_image.clone(),
        )
        .await?;
        if !confirm_continue() {
            return Ok(());
        }
    }

    write_apply_script(
        app_instance,
        overlay_file_name,
        output,
        dry_run,
        impersonate_user,
        docker,
        skip_auth,
        path,
        kubectl_image,
        kubecfg_image,
    )
    .await
}

fn diff(app_instance: &AppInstance) -> Result<Script> {
    let applyset_id = get_applyset_id(app_instance)?;
    let remove_labels = Script::from_str(&format!(
        "apply_label applyset.kubernetes.io/part-of={applyset_id}"
    ));
    let diff = format!("kubectl diff -f - --server-side --force-conflicts --field-manager={KUBIT_APPLIER_FIELD_MANAGER}");
    let diff = Script::from_str(&diff);
    let script = (apply_label_workaround() + (remove_labels | diff)).subshell();
    Ok(script)
}

// Workaround for issue: https://github.com/kubernetes/kubectl/issues/1265
fn apply_label_workaround() -> Script {
    Script::from_str(
        r#"apply_label() {
        kubectl label --local -f - -o json "$1" \
        | jq -c . \
        | while read -r line; do echo '---'; echo "$line" | yq eval -P; done
      }"#,
    )
}

fn get_applyset_id(app_instance: &AppInstance) -> Result<String> {
    // kubectl -n influxdb get secret influxdb -o jsonpath="{.metadata.labels.applyset\.kubernetes\.io/id}"
    let out = Command::new("kubectl")
        .arg("get")
        .arg("secret")
        .arg("-n")
        .arg(app_instance.namespace_any())
        .arg(app_instance.name_any())
        .arg("-o")
        .arg("jsonpath={.metadata.labels.applyset\\.kubernetes\\.io/id}")
        .output()?
        .stdout;
    Ok(String::from_utf8(out)?)
}

fn get_script(dry_run: &Option<DryRun>) -> io::Result<(Box<dyn WriteClose>, Option<PathBuf>)> {
    Ok(if matches!(dry_run, Some(DryRun::Script)) {
        (Box::new(NopDeferredDelete(stdout())), None)
    } else {
        let tmp = tempfile::Builder::new().suffix(".sh").tempfile()?;
        let path = tmp.path().to_path_buf();
        (Box::new(tmp), Some(path))
    })
}

// TODO(jdockerty): refactor args to avoid a huge number of inputs.
#[allow(clippy::too_many_arguments)]
async fn write_apply_script(
    app_instance: AppInstance,
    overlay_file_name: &str,
    mut output: Box<dyn WriteClose>,
    dry_run: &Option<DryRun>,
    impersonate_user: &Option<String>,
    docker: bool,
    skip_auth: bool,
    path: Option<PathBuf>,
    kubectl_image: String,
    kubecfg_image: String,
) -> Result<()> {
    let mut steps: Vec<Script> = vec![];

    if !docker {
        steps.extend([Script::from_str("export KUBECTL_APPLYSET=true")]);
    }

    steps.extend([render::script(
        &app_instance,
        overlay_file_name,
        None,
        docker,
        skip_auth,
        kubecfg_image,
    )
    .await?
        | match dry_run {
            Some(DryRun::Render) => Script::from_str("cat"),
            Some(DryRun::Diff) => diff(&app_instance)?,
            Some(DryRun::Script) | None => {
                apply::script(&app_instance, "-", impersonate_user, docker, &kubectl_image)?
            }
        }]);

    let script: Script = steps.into_iter().sum();

    writeln!(output, "{script}")?;

    // close the file but don't delete it until _deferred_delete_handle local var is in scope.
    let _deferred_delete_handle = output.close()?;

    if let Some(path) = path {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Command::new(path).status()?;
    }
    Ok(())
}

async fn write_delete_script(
    app_instance: AppInstance,
    mut output: Box<dyn WriteClose>,
    docker: bool,
    path: Option<PathBuf>,
) -> Result<()> {
    let mut steps: Vec<Script> = vec![];
    let tmp_dir = TempDir::new().unwrap();
    let output_path = &format!(
        "{}/{}",
        tmp_dir.path().display(),
        cleanup_hack_resource_name(&app_instance.name_any())
    );

    if !docker {
        steps.extend([Script::from_str("export KUBECTL_APPLYSET=true")]);
    }

    steps.extend([
        delete::setup_script(&app_instance, &app_instance.name_any(), output_path, docker)?,
        delete::script(&app_instance, output_path, docker)?,
        delete::post_pruning_script(&app_instance, &app_instance.name_any(), docker)?,
    ]);

    let script: Script = steps.into_iter().sum();

    writeln!(output, "{script}")?;

    // close the file but don't delete it until _deferred_delete_handle local var is in scope.
    let _deferred_delete_handle = output.close()?;

    if let Some(path) = path {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Command::new(path).status()?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn prediff(
    overlay_file_name: &str,
    dry_run: &Option<DryRun>,
    package_image: &Option<String>,
    impersonate_user: &Option<String>,
    docker: bool,
    skip_auth: bool,
    kubectl_image: String,
    kubecfg_image: String,
) -> Result<()> {
    let (output, path) = get_script(dry_run)?;

    let file = File::open(overlay_file_name)?;
    let mut app_instance: AppInstance = serde_yaml::from_reader(file)?;

    if let Some(package_image) = package_image {
        app_instance.spec.package.image.clone_from(package_image);
    }

    write_apply_script(
        app_instance,
        overlay_file_name,
        output,
        &Some(DryRun::Diff),
        impersonate_user,
        docker,
        skip_auth,
        path,
        kubectl_image,
        kubecfg_image,
    )
    .await
}

pub fn confirm_continue() -> bool {
    if !std::io::stdout().is_terminal() {
        return true;
    }

    print!("Apply? [y/N] ");
    std::io::stdout().flush().unwrap();

    /*
    let is_tty = unsafe { libc::isatty(libc::STDIN_FILENO as i32) } != 0;
    if !is_tty {
        return true;
    }
    */

    let mut buffer = [0; 1];
    std::io::stdin().read_exact(&mut buffer).unwrap();
    matches!(buffer[0], b'y' | b'Y')
}

pub async fn delete(app_instance: &str, docker: bool, dry_run: &Option<DryRun>) -> Result<()> {
    match dry_run {
        Some(DryRun::Render | DryRun::Diff) => {
            Err(Error::UnsupportedDryRunOption(dry_run.clone().unwrap()).into())
        }
        Some(DryRun::Script) | None => {
            let (output, path) = get_script(dry_run)?;

            let file = File::open(app_instance)?;
            let app_instance: AppInstance = serde_yaml::from_reader(file)?;

            write_delete_script(app_instance, output, docker, path).await?;

            Ok(())
        }
    }
}
