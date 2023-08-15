use anyhow::Result;
use std::fs::{self, File};
use std::io::{stdout, Write};
use std::os::unix::prelude::PermissionsExt;
use std::process::Command;

use crate::{apply, render, resources::AppInstance, scripting::Script};

#[derive(Clone, clap::ValueEnum)]
pub enum DryRun {
    Render,
    Script,
}

/// Generate a script that runs kubecfg show and kubectl apply and runs it.
pub fn apply(app_instance: &str, dry_run: &Option<DryRun>) -> Result<()> {
    let (mut output, path): (Box<dyn Write>, _) = if matches!(dry_run, Some(DryRun::Script)) {
        (Box::new(stdout()), None)
    } else {
        let tmp = tempfile::Builder::new().suffix(".sh").tempfile()?;
        let path = tmp.path().to_path_buf();
        (Box::new(tmp), Some(path))
    };

    let overlay_file_name = app_instance;
    let file = File::open(overlay_file_name)?;
    let app_instance: AppInstance = serde_yaml::from_reader(file)?;

    let steps = vec![
        Script::from_str("export KUBECTL_APPLYSET=true"),
        render::script(&app_instance, overlay_file_name, None)?
            | match dry_run {
                Some(DryRun::Render) => Script::from_str("cat"),
                _ => apply::script(&app_instance, "-")?,
            },
    ];
    let script: Script = steps.into_iter().sum();

    writeln!(output, "{script}")?;

    if let Some(path) = path {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Command::new(path).status()?;
    }

    Ok(())
}
