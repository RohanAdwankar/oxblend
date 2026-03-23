# oxblend Authoring Skill

Use this skill when you need to create, revise, inspect, or validate `.oxb` scene files for `oxblend`.

## Goal

Write concise declarative 3D scenes that `oxblend` can turn into meshes through Blender.

The main priorities are:

1. Keep the `.oxb` source compact and regular.
2. Prefer repeated structure over copy-paste.
3. Keep geometry coherent in scale, spacing, and naming.
4. Use `oxblend summarize` to verify the scene deterministically.

## Core Model

`oxblend` is not a free-form Blender script language.
It is a small scene DSL that gets parsed in Rust and then executed in Blender through a Python bridge.

The current supported object commands are:

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

The current supported scene commands are:

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

## Basic Syntax

Compact positional syntax works for simple cases:

```oxb
sphere 1 0,0,0 red
cube 2 4,0,1
```

Named attribute syntax is preferred for anything nontrivial:

```oxb
sphere ball radius=1 at=0,0,0 color=red
cube rack size=2 at=4,0,1 scale=1,0.6,2
```

Common transform attributes:

- `at=x,y,z`
- `rotate=x,y,z`
- `scale=s` or `scale=x,y,z`
- `color=name`
- `color=#RRGGBB`
- `color=r,g,b`
- `color=r,g,b,a`

## Groups, Transforms, and Booleans

Inline group:

```oxb
group cluster children=a,b,c at=0,0,0
```

Block group:

```oxb
group cluster {
  children=a,b,c
  at=0,0,0
}
```

Named transform:

```oxb
transform lift at=0,0,5
apply lift to=cluster
```

Constraints:

```oxb
expect_attach muzzle snout_bridge
expect_intersect shoulder upper
expect_ground front_pos__paw
```

Inside `repeat` or `mirror` blocks, use `@name` to refer to a global node outside the local prefix:

```oxb
mirror ear axis=x {
  loft outer sections=...
  expect_attach outer @head
}
```

Boolean operations:

```oxb
union merged left=a right=b
difference carved left=base right=cutter
intersection overlap left=a right=b
```

## Repeat Syntax

Use `repeat` whenever there is symmetry, regular spacing, or repeated modules.
This is the main mechanism for making `.oxb` short and maintainable.

Count + step form:

```oxb
repeat rack count=8 step=6,0,0 start=-24,0,0 {
  cube unit size=2 at=0,0,2 scale=1,0.6,2
}
```

Explicit positions form:

```oxb
repeat aisle positions=-18,0,0.05;0,0,0.05;18,0,0.05 {
  cube strip size=10 scale=0.8,4.0,0.01
}
```

Nested repeats are supported and are preferred for 2D grids:

```oxb
repeat row count=4 step=0,8,0 start=0,-16,0 {
  repeat rack count=8 step=6,0,0 start=-24,0,0 {
    cube unit size=2 at=0,0,2 scale=1,0.6,2
  }
}
```

Best practice:

- Use `repeat` for rows, columns, rings, wall segments, seats, beams, pillars, shelves, and any modular structure.
- Prefer nested `repeat` blocks over long flat lists of repeated objects.
- Use explicit `positions=` only when spacing is irregular.

## Mirror Syntax

Use `mirror` for bilateral anatomy and any left/right duplication that should stay symmetric.

```oxb
mirror side axis=x {
  capsule leg radius=0.22 depth=2.4 at=0.9,0,1.2 rotate=0,8,0
  sphere paw radius=0.18 at=0.9,0.2,0.0 scale=1.2,1.5,0.7
}
```

This expands to two copies:

- a positive-side copy with names prefixed like `side_pos__...`
- a mirrored negative-side copy with names prefixed like `side_neg__...`

Best practice:

- Define only one anatomical side inside the block.
- Use `axis=x` for left/right symmetry in the default coordinate system.
- Keep centerline masses like torso, spine, neck, and head outside the `mirror` block.

## Generated Shapes

Extrude:

```oxb
extrude wall profile=0,0;1,0;1,1;0,1 depth=2
```

Loft:

```oxb
loft muzzle sections=0:-0.3,-0.1;0.3,-0.1;0,0.2|0.8:-0.1,-0.05;0.1,-0.05;0,0.08
```

Skin:

```oxb
skin bridge path=0,0,0;0,0.4,0.1;0,0.8,0.1 radii=0.2;0.14;0.06
```

Revolve:

```oxb
revolve vase profile=1,0;0.5,2 axis=z angle=180
```

Sweep:

```oxb
sweep rail profile=0,0;0.25,0;0.25,0.25;0,0.25 path=0,0,0;0,0,2
```

## Naming Guidance

Use short semantic names:

- Good: `floor`, `wall`, `rack`, `aisle`, `beam`, `tower`
- Less good: `thing1`, `obj_a`, `mesh42`

Inside `repeat`, names should describe the repeated unit, not the final expanded names.
The parser qualifies repeated names automatically.

## Modeling Best Practices

- Keep one consistent unit scale across the file.
- Establish anchors first: floor, outer shell, major walls, central masses.
- Add repetition next: rows, columns, bays, aisles, towers, seats.
- Use booleans for subtractive design only when needed.
- Prefer `cube` plus transforms for architectural layouts and low-detail blockouts.
- Prefer `capsule`, `sphere`, and `mirror` for animals, limbs, tails, and other organic silhouettes.
- Use `skin` to bridge body parts that should read as physically attached.
- For readability, separate large structural regions with blank lines and comments.

For complex scenes:

1. Build the coarse layout first.
2. Add one repeated module.
3. Expand to repeated rows or grids.
4. Add booleans or generator shapes last.

## Validation Workflow

Use these commands repeatedly while authoring:

One-shot export:

```bash
cargo run -- scene.oxb -o scene.stl
```

Live viewer:

```bash
cargo run -- view scene.oxb
```

Deterministic scene summary:

```bash
cargo run -- summarize scene.oxb
```

`summarize` is especially useful for LLM workflows because it expands repeats and prints deterministic scene facts:

- total object counts
- object and group names
- approximate bounds
- lint warnings for disconnected or unattached parts
- scene size and diagonal
- pairwise distances
- approximate intersections via bounds overlap

Use it to catch:

- accidental overlaps
- mis-scaled repeated modules
- objects placed outside the intended envelope
- inconsistent naming
- spacing that breaks symmetry
- failed expected attachments or ground contacts

## Export Guidance

- Use `.stl` for geometry-only output.
- Use `.obj`, `.ply`, or `.glb` if color matters.
- The live viewer uses `.glb` internally for preview.

## Agent Behavior

When writing or revising `.oxb`:

1. Prefer the shortest scene that preserves structure and readability.
2. Replace copy-pasted repeated geometry with `repeat`.
3. Keep names semantic.
4. After changes, run `oxblend summarize` and inspect counts, bounds, and pairwise relationships.
5. If the scene is architectural or symmetric, assume repetition is expected unless there is evidence otherwise.
