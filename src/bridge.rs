use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::json;

use crate::scene::{OutputFormat, Scene};

const BLENDER_DRIVER: &str = include_str!("../scripts/blender_driver.py");
const BLENDER_IMPORT_DRIVER: &str = r#"import sys
from pathlib import Path

import bpy


def parse_args():
    if "--" not in sys.argv:
        raise SystemExit("expected model path after --")
    args = sys.argv[sys.argv.index("--") + 1 :]
    if len(args) != 1:
        raise SystemExit("usage: blender --python import.py -- <model.glb>")
    return Path(args[0])


def main():
    model_path = parse_args()
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=str(model_path))


if __name__ == "__main__":
    main()
"#;

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

pub fn run_blender_snapshot(
    scene: &mut Scene,
    output: &Path,
    blender_bin: Option<&Path>,
) -> Result<()> {
    let blender = resolve_snapshot_blender_bin(blender_bin)?;
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
    let outputs = snapshot_output_paths(output);
    for path in &outputs {
        if path.exists() {
            fs::remove_file(path)
                .with_context(|| format!("failed to clear previous snapshot {}", path.display()))?;
        }
    }

    let mut child = Command::new(&blender)
        .arg("--factory-startup")
        .arg("--python")
        .arg(&driver_path)
        .arg("--")
        .arg(&scene_path)
        .arg(output)
        .arg(".png")
        .spawn()
        .with_context(|| format!("failed to launch blender at {}", blender.display()))?;

    wait_for_snapshots(&mut child, &outputs)?;

    Ok(())
}

pub fn launch_blender_preview(model_path: &Path, blender_bin: Option<&Path>) -> Result<()> {
    let blender = resolve_blender_bin(blender_bin)?;
    let temp_dir = make_temp_dir()?;
    let driver_path = temp_dir.join("import_preview.py");

    fs::write(&driver_path, BLENDER_IMPORT_DRIVER)
        .context("failed to write blender import driver")?;

    Command::new(&blender)
        .arg("--factory-startup")
        .arg("--python")
        .arg(&driver_path)
        .arg("--")
        .arg(model_path)
        .spawn()
        .with_context(|| format!("failed to launch blender at {}", blender.display()))?;

    Ok(())
}

fn resolve_blender_bin(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if let Some(candidate) = normalize_blender_path(path) {
            return Ok(candidate);
        }
        bail!("blender executable not found at {}", path.display());
    }

    if let Ok(path) = env::var("OXBLEND_BLENDER_BIN") {
        let candidate = PathBuf::from(path);
        if let Some(candidate) = normalize_blender_path(&candidate) {
            return Ok(candidate);
        }
    }

    if let Some(path) = find_in_path("blender") {
        return Ok(path);
    }

    if let Some(path) = find_default_blender_locations() {
        return Ok(path);
    }

    Err(anyhow!(
        "could not find Blender; pass --blender-bin or set OXBLEND_BLENDER_BIN"
    ))
}

fn resolve_snapshot_blender_bin(explicit: Option<&Path>) -> Result<PathBuf> {
    if explicit.is_some() {
        return resolve_blender_bin(explicit);
    }

    #[cfg(target_os = "macos")]
    {
        for candidate in [
            Path::new("/Applications/Blender.app"),
            Path::new("/Applications/Blender.app/Contents/MacOS/Blender"),
            Path::new("/Applications/Blender 5.2.app"),
            Path::new("/Applications/Blender 5.2.app/Contents/MacOS/Blender"),
            Path::new("/Applications/Blender 5.1.app"),
            Path::new("/Applications/Blender 5.1.app/Contents/MacOS/Blender"),
        ] {
            if let Some(path) = normalize_blender_path(candidate) {
                return Ok(path);
            }
        }
    }

    resolve_blender_bin(None)
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if let Some(candidate) = normalize_blender_path(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn normalize_blender_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }

    #[cfg(target_os = "macos")]
    {
        if path.extension().and_then(|ext| ext.to_str()) == Some("app") {
            let candidate = path.join("Contents/MacOS/Blender");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn find_default_blender_locations() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        for candidate in [
            Path::new("/Applications/Blender.app"),
            Path::new("/Applications/Blender.app/Contents/MacOS/Blender"),
            Path::new("/Applications/Blender 5.2.app"),
            Path::new("/Applications/Blender 5.2.app/Contents/MacOS/Blender"),
        ] {
            if let Some(path) = normalize_blender_path(candidate) {
                return Some(path);
            }
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

fn snapshot_output_paths(output: &Path) -> Vec<PathBuf> {
    let stem = output
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("snapshot");
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let extension = output
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png");

    ["isometric", "front", "left", "right", "back", "top"]
        .into_iter()
        .map(|suffix| parent.join(format!("{stem}_{suffix}.{extension}")))
        .collect()
}

fn wait_for_snapshots(child: &mut Child, outputs: &[PathBuf]) -> Result<()> {
    let timeout = Duration::from_secs(30);
    let poll = Duration::from_millis(150);
    let start = Instant::now();
    let mut stable_polls = vec![0usize; outputs.len()];
    let mut last_sizes = vec![None; outputs.len()];

    loop {
        if let Some(status) = child.try_wait()? {
            if outputs.iter().all(|path| path.is_file()) {
                return Ok(());
            }
            bail!("blender snapshot failed with status {}", status);
        }

        let mut all_stable = true;
        for (index, output) in outputs.iter().enumerate() {
            match fs::metadata(output) {
                Ok(metadata) => {
                    let size = metadata.len();
                    if last_sizes[index] == Some(size) && size > 0 {
                        stable_polls[index] += 1;
                    } else {
                        stable_polls[index] = 0;
                        last_sizes[index] = Some(size);
                    }
                    if stable_polls[index] < 3 {
                        all_stable = false;
                    }
                }
                Err(_) => {
                    all_stable = false;
                }
            }
        }

        if all_stable {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }

        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "timed out waiting for Blender to finish writing snapshots rooted at {}",
                outputs
                    .first()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<none>".to_string())
            );
        }

        thread::sleep(poll);
    }
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

    #[test]
    fn normalizes_macos_app_bundle_path() {
        let temp_dir = make_temp_dir().unwrap();
        let app_path = temp_dir.join("Blender.app");
        let binary_path = app_path.join("Contents/MacOS/Blender");
        fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
        fs::write(&binary_path, b"").unwrap();

        let resolved = normalize_blender_path(&app_path).unwrap();
        assert_eq!(resolved, binary_path);
    }
}
