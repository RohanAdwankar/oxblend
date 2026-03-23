use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::json;

use crate::scene::{OutputFormat, Scene};

const BLENDER_DRIVER: &str = include_str!("../scripts/blender_driver.py");

pub fn run_blender_export(
    scene: &mut Scene,
    output: &Path,
    output_format: OutputFormat,
    blender_bin: Option<&Path>,
) -> Result<()> {
    let blender = resolve_blender_bin(blender_bin)?;
    let temp_dir = make_temp_dir()?;
    let scene_path = temp_dir.join("scene.json");
    let driver_path = temp_dir.join("driver.py");

    fs::write(&scene_path, serde_json::to_vec_pretty(scene)?)
        .context("failed to write intermediate scene json")?;
    fs::write(&driver_path, BLENDER_DRIVER).context("failed to write blender driver")?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    let status = Command::new(&blender)
        .arg("--background")
        .arg("--factory-startup")
        .arg("--python")
        .arg(&driver_path)
        .arg("--")
        .arg(&scene_path)
        .arg(output)
        .arg(output_format.extension())
        .status()
        .with_context(|| format!("failed to launch blender at {}", blender.display()))?;

    if !status.success() {
        bail!("blender export failed with status {}", status);
    }

    Ok(())
}

fn resolve_blender_bin(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        bail!("blender executable not found at {}", path.display());
    }

    if let Ok(path) = env::var("OXBLEND_BLENDER_BIN") {
        let candidate = PathBuf::from(path);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    if let Some(path) = find_in_path("blender") {
        return Ok(path);
    }

    Err(anyhow!(
        "could not find Blender; pass --blender-bin or set OXBLEND_BLENDER_BIN"
    ))
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn make_temp_dir() -> Result<PathBuf> {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = env::temp_dir().join(format!("oxblend-{}", suffix));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[allow(dead_code)]
fn _bridge_contract_example(scene: &Scene) -> serde_json::Value {
    json!({
        "scene": scene,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::parser::parse_scene;

    #[test]
    fn resolves_missing_blender() {
        let error = resolve_blender_bin(Some(Path::new("/definitely/missing/blender")))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not found"));
    }

    #[test]
    fn runs_bridge_with_fake_blender() {
        let temp_dir = make_temp_dir().unwrap();
        let blender_path = temp_dir.join("fake-blender");
        let output_path = temp_dir.join("mesh.stl");

        let mut script = fs::File::create(&blender_path).unwrap();
        writeln!(
            script,
            "#!/bin/sh\nout=''\nprev=''\nfor arg in \"$@\"; do\n  out=\"$prev\"\n  prev=\"$arg\"\ndone\nprintf ok > \"$out\"\n"
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&blender_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&blender_path, perms).unwrap();
        }

        let mut scene = parse_scene("sphere radius=1\n").unwrap();
        run_blender_export(
            &mut scene,
            &output_path,
            OutputFormat::Stl,
            Some(&blender_path),
        )
        .unwrap();

        assert_eq!(fs::read_to_string(output_path).unwrap(), "ok");
    }
}
