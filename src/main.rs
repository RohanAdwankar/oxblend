mod bridge;
mod parser;
mod scene;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser as ClapParser;

use crate::bridge::run_blender_export;
use crate::parser::parse_scene;
use crate::scene::OutputFormat;

#[derive(Debug, ClapParser)]
#[command(author, version, about = "Declarative Blender scene DSL to mesh exporter")]
struct Cli {
    /// Input .oxb scene file
    input: PathBuf,

    /// Output mesh path; format is inferred from file extension
    #[arg(short, long)]
    output: PathBuf,

    /// Path to a Blender executable
    #[arg(long)]
    blender_bin: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read input file {}", cli.input.display()))?;

    let mut scene = parse_scene(&source)
        .with_context(|| format!("failed to parse {}", cli.input.display()))?;
    scene.validate()?;

    let output_format = OutputFormat::from_path(&cli.output)?;
    if scene.uses_color() && !output_format.supports_color() {
        bail!(
            "scene uses color but {} does not preserve color; choose .obj, .ply, or .glb",
            output_format.extension()
        );
    }

    run_blender_export(&mut scene, &cli.output, output_format, cli.blender_bin.as_deref())
}
