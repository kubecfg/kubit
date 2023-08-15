use anyhow::Result;
use kube::ResourceExt;
use std::fs::{self, File};
use std::io::{stdout, Write};
use std::os::unix::prelude::PermissionsExt;
use std::process::Command;

use crate::{
    apply::{self, KUBIT_APPLIER_FIELD_MANAGER},
    render,
    resources::AppInstance,
    scripting::Script,
};

#[derive(Clone, clap::ValueEnum)]
pub enum DryRun {
    Render,
    Diff,
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
                Some(DryRun::Diff) => diff(&app_instance)?,
                Some(DryRun::Script) | None => apply::script(&app_instance, "-")?,
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
        .arg(app_instance.namespace().unwrap())
        .arg(app_instance.name_any())
        .arg("-o")
        .arg("jsonpath={.metadata.labels.applyset\\.kubernetes\\.io/id}")
        .output()?
        .stdout;
    Ok(String::from_utf8(out)?)
}