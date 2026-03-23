# oxblend

https://github.com/user-attachments/assets/e48a348d-5c58-428c-a217-1cf3c001aa06

`oxblend` is a small declarative scene DSL for Blender-backed mesh export.
You write a `.oxb` file with objects like spheres, cubes, booleans, groups, and a few generated shapes, then run:

```bash
oxblend scene.oxb -o thing.stl
```

## Why?

Similar to how Mermaid as a diagram language has made it easier for LLMs to generate diagrams, the aim of this project is to make it easier for LLMs to generate 3D assets.

## Quick Start

Build the CLI:

```bash
cargo build
```

Run an example:

```bash
cargo run -- examples/datacenter.oxb -o examples/datacenter.stl
```

Launch the live viewer:

```bash
cargo run -- view examples/datacenter.oxb
```

Generate a deterministic scene summary:

```bash
cargo run -- summarize examples/datacenter2.oxb
```

Oxblend should find your Blender for you if it is installed normally.

Otherwise point `oxblend` at it explicitly:

```bash
cargo run -- examples/datacenter.oxb -o examples/datacenter.stl --blender-bin /Applications/Blender.app
```

You can also use:

```bash
export OXBLEND_BLENDER_BIN=/Applications/Blender.app
```

On macOS, `oxblend` accepts either the app bundle path like `/Applications/Blender.app` or the inner executable path `.../Contents/MacOS/Blender`.

## DSL Example

The repo includes a datacenter example made from rectangular solids:

[`examples/datacenter.oxb`](/Users/rohanadwankar/oxblend/examples/datacenter.oxb)

The shorthand style is intentionally compact. For example:

```oxb
sphere 1 0,0,0 red
cube rack size=2 at=4,0,1 scale=1,0.6,2
union combined left=rack right=other_rack
```

Supported v1 commands:

- `sphere`
- `cube`
- `cylinder`
- `capsule`
- `skin`
- `cone`
- `torus`
- `extrude`
- `loft`
- `revolve`
- `sweep`
- `group`
- `transform`
- `apply`
- `expect_attach`
- `expect_intersect`
- `expect_ground`
- `union`
- `difference`
- `intersection`
- `repeat`
- `mirror`

## How It Works

The code is split into a small Rust front-end and a Blender Python bridge:

- [`src/parser.rs`](/Users/rohanadwankar/oxblend/src/parser.rs) parses `.oxb` into a typed scene model.
- [`src/scene.rs`](/Users/rohanadwankar/oxblend/src/scene.rs) defines objects, booleans, transforms, groups, and output formats.
- [`src/bridge.rs`](/Users/rohanadwankar/oxblend/src/bridge.rs) locates Blender, writes a temporary JSON scene payload, writes the Python driver, and launches Blender in headless mode.
- [`scripts/blender_driver.py`](/Users/rohanadwankar/oxblend/scripts/blender_driver.py) reconstructs the scene in Blender using `bpy`, applies transforms/material colors, runs booleans, and exports to STL/OBJ/PLY/GLB.

So the cooperation model is:

1. Rust parses and validates the declarative scene.
2. Rust serializes the scene to JSON.
3. Rust starts Blender with `--background --python ...`.
4. Blender Python creates the actual geometry and writes the final mesh file.

For `oxblend view`, there is one extra layer:

1. `oxblend` starts a localhost web server.
2. The browser shows a split view with the interactive preview on the left and the source editor on the right.
3. Edits from the browser are written back to the `.oxb` file.
4. A file watcher triggers a fresh temporary `.glb` export through Blender.
5. The browser reloads the preview model when the new render succeeds.

The current browser preview uses `model-viewer` from a CDN, so the webview expects ordinary internet access for that frontend dependency.

## Deterministic Summary

`oxblend summarize foo.oxb` prints a deterministic text description of the parsed scene.

It includes:

- expanded object counts after repeats
- object, boolean, and group names
- lint warnings for disconnected components and unattached parts
- lint warnings for failed declared constraints
- inferred mirror-pair notes for repeated bilateral parts
- approximate bounds, centers, and sizes
- scene extents and diagonal
- pairwise intersection and distance information

This is intended for agent workflows that need a stable textual way to verify whether a generated scene is coherent.
