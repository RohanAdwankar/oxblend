mod bridge;
mod parser;
mod scene;
mod summary;
mod view;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser as ClapParser, Subcommand};

use crate::bridge::{run_blender_export, run_blender_snapshot};
use crate::parser::parse_scene;
use crate::scene::OutputFormat;
use crate::summary::summarize_scene;
use crate::view::run_viewer;

#[derive(Debug, ClapParser)]
#[command(author, version, about = "Declarative Blender scene DSL to mesh exporter")]
struct Cli {
    /// Input .oxb scene file for one-shot export mode
    input: Option<PathBuf>,

    /// Output mesh path; format is inferred from file extension
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Path to a Blender executable
    #[arg(long)]
    blender_bin: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Export a .oxb file to a mesh format
    Export {
        /// Input .oxb scene file
        input: PathBuf,

        /// Output mesh path; format is inferred from file extension
        #[arg(short, long)]
        output: PathBuf,

        /// Path to a Blender executable
        #[arg(long)]
        blender_bin: Option<PathBuf>,
    },
    /// Launch a live local web viewer for a .oxb file
    View {
        /// Input .oxb scene file
        input: PathBuf,

        /// Path to a Blender executable
        #[arg(long)]
        blender_bin: Option<PathBuf>,
    },
    /// Print a deterministic textual summary of a .oxb scene
    Summarize {
        /// Input .oxb scene file
        input: PathBuf,
    },
    /// Render a PNG snapshot of a .oxb scene
    Snapshot {
        /// Input .oxb scene file
        input: PathBuf,

        /// Output PNG path
        #[arg(short, long)]
        output: PathBuf,

        /// Path to a Blender executable
        #[arg(long)]
        blender_bin: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::View { input, blender_bin }) => {
            run_viewer(input, blender_bin.or(cli.blender_bin)).await?;
        }
        Some(Command::Summarize { input }) => {
            let source = fs::read_to_string(&input)
                .with_context(|| format!("failed to read input file {}", input.display()))?;
            let scene = parse_scene(&source)
                .with_context(|| format!("failed to parse {}", input.display()))?;
            scene.validate()?;
            print!("{}", summarize_scene(&scene)?);
        }
        Some(Command::Snapshot {
            input,
            output,
            blender_bin,
        }) => {
            let source = fs::read_to_string(&input)
                .with_context(|| format!("failed to read input file {}", input.display()))?;
            let mut scene = parse_scene(&source)
                .with_context(|| format!("failed to parse {}", input.display()))?;
            scene.validate()?;
            run_blender_snapshot(&mut scene, &output, blender_bin.as_deref())?;
        }
        Some(Command::Export {
            input,
            output,
            blender_bin,
        }) => {
            let source = fs::read_to_string(&input)
                .with_context(|| format!("failed to read input file {}", input.display()))?;

            let mut scene = parse_scene(&source)
                .with_context(|| format!("failed to parse {}", input.display()))?;
            scene.validate()?;

            let output_format = OutputFormat::from_path(&output)?;
            if scene.uses_color() && !output_format.supports_color() {
                bail!(
                    "scene uses color but {} does not preserve color; choose .obj, .ply, or .glb",
                    output_format.extension()
                );
            }

            run_blender_export(&mut scene, &output, output_format, blender_bin.as_deref())?;
        }
        None => {
            let input = cli
                .input
                .context("input .oxb file is required unless using `oxblend view <file>`")?;
            let output = cli
                .output
                .context("`-o, --output <OUTPUT>` is required for one-shot export mode")?;

            let source = fs::read_to_string(&input)
                .with_context(|| format!("failed to read input file {}", input.display()))?;

            let mut scene = parse_scene(&source)
                .with_context(|| format!("failed to parse {}", input.display()))?;
            scene.validate()?;

            let output_format = OutputFormat::from_path(&output)?;
            if scene.uses_color() && !output_format.supports_color() {
                bail!(
                    "scene uses color but {} does not preserve color; choose .obj, .ply, or .glb",
                    output_format.extension()
                );
            }

            run_blender_export(&mut scene, &output, output_format, cli.blender_bin.as_deref())?;
        }
    }

    Ok(())
}
