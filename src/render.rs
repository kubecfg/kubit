use crate::{resources::AppInstance, Result};

/// Generates shell script that will render the manifest
pub fn emit_script<W>(_app_instance: &AppInstance, w: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    writeln!(w, "echo TODO")?;
    Ok(())
}
