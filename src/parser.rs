use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use shlex::split as shlex_split;

use crate::scene::{
    ApplySpec, Axis, BooleanOp, BooleanSpec, Color, ConstraintSpec, GroupSpec, LoftSection,
    NamedTransform, ObjectKind, ObjectSpec, Scene, Transform, Vec2, Vec3,
};

pub fn parse_scene(source: &str) -> Result<Scene> {
    let mut parser = SceneParser::new();
    parser.parse(source)
}

struct SceneParser {
    anonymous_counter: usize,
}

#[derive(Clone)]
struct ParserContext {
    prefix: String,
    offset: Vec3,
    mirror: Option<MirrorAxis>,
}

impl ParserContext {
    fn qualify(&self, name: impl AsRef<str>) -> String {
        let name = name.as_ref();
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}{}", self.prefix, name)
        }
    }

    fn child(&self, prefix_fragment: &str, offset: Vec3) -> Self {
        Self {
            prefix: format!("{}{}", self.prefix, prefix_fragment),
            offset: add_vec3(self.offset, offset),
            mirror: self.mirror,
        }
    }

    fn mirrored_child(&self, prefix_fragment: &str, axis: MirrorAxis) -> Self {
        Self {
            prefix: format!("{}{}", self.prefix, prefix_fragment),
            offset: mirror_vec3(self.offset, axis),
            mirror: Some(axis),
        }
    }
}

impl Default for ParserContext {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            offset: Vec3::ZERO,
            mirror: None,
        }
    }
}

struct RepeatSpec {
    name: String,
    offsets: Vec<Vec3>,
}

#[derive(Clone, Copy)]
enum MirrorAxis {
    X,
    Y,
    Z,
}

struct MirrorSpec {
    name: String,
    axis: MirrorAxis,
}

impl SceneParser {
    fn new() -> Self {
        Self {
            anonymous_counter: 0,
        }
    }

    fn parse(&mut self, source: &str) -> Result<Scene> {
        let mut scene = Scene::new();
        let lines: Vec<&str> = source.lines().collect();
        let mut index = 0usize;
        self.parse_block(&lines, &mut index, &mut scene, &ParserContext::default())?;
        Ok(scene)
    }

    fn parse_block(
        &mut self,
        lines: &[&str],
        index: &mut usize,
        scene: &mut Scene,
        ctx: &ParserContext,
    ) -> Result<()> {
        while *index < lines.len() {
            let line_no = *index + 1;
            let line = strip_comments(lines[*index]).trim();
            *index += 1;
            if line.is_empty() {
                continue;
            }

            if line == "}" {
                bail!("line {}: unexpected closing brace", line_no);
            }

            if line.ends_with('{') {
                let header = line.trim_end_matches('{').trim();
                let header_tokens = tokenize(header)
                    .with_context(|| format!("line {}: invalid block header", line_no))?;
                if header_tokens.is_empty() {
                    bail!("line {}: empty block header", line_no);
                }

                let (body_start, body_end) = collect_block_range(lines, index, line_no)?;
                let body = &lines[body_start..body_end];

                match header_tokens[0].as_str() {
                    "group" => {
                        if header_tokens.len() != 2 {
                            bail!("line {}: expected 'group <name> {{'", line_no);
                        }
                        scene.groups.push(parse_group_block(
                            &ctx.qualify(&header_tokens[1]),
                            body,
                            ctx,
                        )?);
                    }
                    "transform" => {
                        if header_tokens.len() != 2 {
                            bail!("line {}: expected 'transform <name> {{'", line_no);
                        }
                        scene.transforms.push(parse_transform_block(
                            &ctx.qualify(&header_tokens[1]),
                            body,
                            ctx,
                        )?);
                    }
                    "repeat" => {
                        let repeat = parse_repeat_header(line_no, &header_tokens)?;
                        for (idx, offset) in repeat.offsets.iter().enumerate() {
                            let child_ctx =
                                ctx.child(&format!("{}_{}__", repeat.name, idx + 1), *offset);
                            let mut nested_index = 0usize;
                            self.parse_block(body, &mut nested_index, scene, &child_ctx)?;
                        }
                    }
                    "mirror" => {
                        let mirror = parse_mirror_header(line_no, &header_tokens)?;
                        let contexts = [
                            ctx.child(&format!("{}_pos__", mirror.name), Vec3::ZERO),
                            ctx.mirrored_child(&format!("{}_neg__", mirror.name), mirror.axis),
                        ];
                        for child_ctx in contexts {
                            let mut nested_index = 0usize;
                            self.parse_block(body, &mut nested_index, scene, &child_ctx)?;
                        }
                    }
                    other => bail!("line {}: unsupported block '{}'", line_no, other),
                }

                *index = body_end + 1;
                continue;
            }

            let tokens = tokenize(line).with_context(|| format!("line {}: invalid syntax", line_no))?;
            if tokens.is_empty() {
                continue;
            }

            match tokens[0].as_str() {
                "sphere" | "cube" | "cylinder" | "capsule" | "skin" | "cone" | "torus"
                | "extrude" | "loft" | "revolve" | "sweep" => {
                    scene.objects.push(self.parse_object(line_no, &tokens, ctx)?)
                }
                "group" => scene.groups.push(parse_group_inline(line_no, &tokens, ctx)?),
                "transform" => scene.transforms.push(parse_transform_inline(line_no, &tokens, ctx)?),
                "apply" => scene.applies.push(parse_apply(line_no, &tokens, ctx)?),
                "expect_attach" => scene.constraints.push(parse_expect_attach(line_no, &tokens, ctx)?),
                "expect_intersect" => {
                    scene.constraints.push(parse_expect_intersect(line_no, &tokens, ctx)?)
                }
                "expect_ground" => scene.constraints.push(parse_expect_ground(line_no, &tokens, ctx)?),
                "union" | "difference" | "intersection" => {
                    scene.booleans.push(parse_boolean(line_no, &tokens, ctx)?)
                }
                other => bail!("line {}: unknown command '{}'", line_no, other),
            }
        }

        Ok(())
    }

    fn next_generated_name(&mut self, prefix: &str, ctx: &ParserContext) -> String {
        self.anonymous_counter += 1;
        ctx.qualify(format!("{}_{}", prefix, self.anonymous_counter))
    }

    fn parse_object(
        &mut self,
        line_no: usize,
        tokens: &[String],
        ctx: &ParserContext,
    ) -> Result<ObjectSpec> {
        let command = tokens[0].as_str();
        let (mut name, positional, attrs) = split_name_and_attrs(command, &tokens[1..], || {
            self.next_generated_name(command, ctx)
        });
        if !ctx.prefix.is_empty() && !name.starts_with(&ctx.prefix) {
            name = ctx.qualify(name);
        }

        let (kind, consumed_positional) = match command {
            "sphere" => {
                let radius = parse_required_scalar(command, "radius", &positional, &attrs, 0)?;
                (ObjectKind::Sphere { radius }, 1usize)
            }
            "cube" => {
                let size = parse_required_scalar(command, "size", &positional, &attrs, 0)?;
                (ObjectKind::Cube { size }, 1usize)
            }
            "cylinder" => {
                let radius = parse_required_scalar(command, "radius", &positional, &attrs, 0)?;
                let depth = parse_required_scalar(command, "depth", &positional, &attrs, 1)?;
                (ObjectKind::Cylinder { radius, depth }, 2usize)
            }
            "capsule" => {
                let radius = parse_required_scalar(command, "radius", &positional, &attrs, 0)?;
                let depth = parse_required_scalar(command, "depth", &positional, &attrs, 1)?;
                (ObjectKind::Capsule { radius, depth }, 2usize)
            }
            "skin" => {
                let path = parse_required_profile3(command, "path", &attrs)?;
                let radii = parse_required_scalar_list(command, "radii", &attrs)?;
                if path.len() != radii.len() {
                    bail!(
                        "line {}: skin requires path and radii to have the same number of points",
                        line_no
                    );
                }
                let sides = attrs
                    .get("sides")
                    .map(|value| parse_usize(value))
                    .transpose()?
                    .unwrap_or(12);
                if sides < 3 {
                    bail!("line {}: skin sides must be at least 3", line_no);
                }
                (ObjectKind::Skin { path, radii, sides }, 0usize)
            }
            "cone" => {
                let radius = parse_required_scalar(command, "radius", &positional, &attrs, 0)?;
                let depth = parse_required_scalar(command, "depth", &positional, &attrs, 1)?;
                (ObjectKind::Cone { radius, depth }, 2usize)
            }
            "torus" => {
                let major_radius =
                    parse_required_scalar(command, "major_radius", &positional, &attrs, 0)?;
                let minor_radius =
                    parse_required_scalar(command, "minor_radius", &positional, &attrs, 1)?;
                (
                    ObjectKind::Torus {
                        major_radius,
                        minor_radius,
                    },
                    2usize,
                )
            }
            "extrude" => {
                let profile = parse_required_profile2(command, "profile", &attrs)?;
                let depth = parse_required_scalar(command, "depth", &positional, &attrs, 0)?;
                (ObjectKind::Extrude { profile, depth }, 1usize)
            }
            "loft" => {
                let sections = parse_required_loft_sections(command, "sections", &attrs)?;
                (ObjectKind::Loft { sections }, 0usize)
            }
            "revolve" => {
                let profile = parse_required_profile2(command, "profile", &attrs)?;
                let axis = attrs
                    .get("axis")
                    .map(|value| parse_axis(value))
                    .transpose()?
                    .unwrap_or(Axis::Z);
                let angle_degrees = attrs
                    .get("angle")
                    .map(|value| parse_f64(value))
                    .transpose()?
                    .unwrap_or(360.0);
                (
                    ObjectKind::Revolve {
                        profile,
                        axis,
                        angle_degrees,
                    },
                    0usize,
                )
            }
            "sweep" => {
                let profile = parse_required_profile2(command, "profile", &attrs)?;
                let path = parse_required_profile3(command, "path", &attrs)?;
                (ObjectKind::Sweep { profile, path }, 0usize)
            }
            _ => bail!("line {}: unsupported object '{}'", line_no, command),
        };

        let remaining = if consumed_positional >= positional.len() {
            &[][..]
        } else {
            &positional[consumed_positional..]
        };
        let mut transform = parse_transform_attrs(&attrs, remaining)?;
        transform.translation = add_vec3(transform.translation, ctx.offset);
        if let Some(axis) = ctx.mirror {
            transform = mirror_transform(transform, axis);
        }

        Ok(ObjectSpec {
            name,
            kind,
            transform,
        })
    }
}

fn collect_block_range(lines: &[&str], index: &usize, start_line: usize) -> Result<(usize, usize)> {
    let body_start = *index;
    let mut depth = 1usize;
    let mut cursor = *index;

    while cursor < lines.len() {
        let line = strip_comments(lines[cursor]).trim();
        if line.ends_with('{') {
            depth += 1;
        } else if line == "}" {
            depth -= 1;
            if depth == 0 {
                return Ok((body_start, cursor));
            }
        }
        cursor += 1;
    }

    bail!("line {}: unterminated block", start_line)
}

fn tokenize(line: &str) -> Result<Vec<String>> {
    shlex_split(line).ok_or_else(|| anyhow!("unable to tokenize line"))
}

fn strip_comments(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return "";
    }

    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'#' && i > 0 && bytes[i - 1].is_ascii_whitespace() {
            return &line[..i];
        }
    }
    line
}

fn split_name_and_attrs<F>(
    command: &str,
    tokens: &[String],
    default_name: F,
) -> (String, Vec<String>, HashMap<String, String>)
where
    F: FnOnce() -> String,
{
    let mut name = None;
    let mut positional = Vec::new();
    let mut attrs = HashMap::new();

    for token in tokens {
        if let Some((key, value)) = token.split_once('=') {
            attrs.insert(key.to_ascii_lowercase(), value.to_string());
        } else {
            positional.push(token.clone());
        }
    }

    if let Some(attr_name) = attrs.remove("name") {
        name = Some(attr_name);
    } else if let Some(first) = positional.first() {
        let treat_as_name = match command {
            "sphere" | "cube" | "cylinder" | "capsule" | "skin" | "cone" | "torus" => {
                parse_f64(first).is_err()
            }
            "extrude" | "loft" | "revolve" | "sweep" => {
                !first.contains(';') && !first.contains('|') && parse_f64(first).is_err()
            }
            _ => false,
        };
        if treat_as_name {
            name = Some(positional.remove(0));
        }
    }

    (name.unwrap_or_else(default_name), positional, attrs)
}

fn parse_repeat_header(line_no: usize, tokens: &[String]) -> Result<RepeatSpec> {
    if tokens.len() < 2 {
        bail!(
            "line {}: expected 'repeat <name> count=<n> step=<x,y,z> {{' or positions=<...>'",
            line_no
        );
    }
    let name = tokens[1].clone();
    let attrs = parse_attrs(&tokens[2..]);

    if let Some(positions) = attrs.get("positions") {
        return Ok(RepeatSpec {
            name,
            offsets: parse_positions(positions)?,
        });
    }

    let count = attrs
        .get("count")
        .ok_or_else(|| anyhow!("line {}: repeat requires count=<n> or positions=<...>", line_no))?
        .parse::<usize>()
        .with_context(|| format!("line {}: invalid repeat count", line_no))?;
    if count == 0 {
        bail!("line {}: repeat count must be at least 1", line_no);
    }
    let step = attrs
        .get("step")
        .map(|value| parse_vec3(value))
        .transpose()?
        .ok_or_else(|| anyhow!("line {}: repeat count form requires step=<x,y,z>", line_no))?;
    let start = attrs
        .get("start")
        .map(|value| parse_vec3(value))
        .transpose()?
        .unwrap_or(Vec3::ZERO);

    let mut offsets = Vec::with_capacity(count);
    for i in 0..count {
        offsets.push(add_vec3(start, mul_vec3(step, i as f64)));
    }

    Ok(RepeatSpec { name, offsets })
}

fn parse_mirror_header(line_no: usize, tokens: &[String]) -> Result<MirrorSpec> {
    if tokens.len() < 2 {
        bail!("line {}: expected 'mirror <name> axis=<x|y|z> {{'", line_no);
    }
    let attrs = parse_attrs(&tokens[2..]);
    let axis = attrs
        .get("axis")
        .ok_or_else(|| anyhow!("line {}: mirror requires axis=<x|y|z>", line_no))
        .and_then(|value| parse_mirror_axis(value))?;
    Ok(MirrorSpec {
        name: tokens[1].clone(),
        axis,
    })
}

fn parse_group_inline(line_no: usize, tokens: &[String], ctx: &ParserContext) -> Result<GroupSpec> {
    if tokens.len() < 2 {
        bail!("line {}: expected 'group <name> ...'", line_no);
    }
    let mut attrs = parse_attrs(&tokens[2..]);
    let children = parse_children_attr(&mut attrs, line_no, "group")?
        .into_iter()
        .map(|child| ctx.qualify(child))
        .collect();
    let mut transform = parse_transform_attrs(&attrs, &[])?;
    transform.translation = add_vec3(transform.translation, ctx.offset);
    if let Some(axis) = ctx.mirror {
        transform = mirror_transform(transform, axis);
    }
    Ok(GroupSpec {
        name: ctx.qualify(&tokens[1]),
        children,
        transform,
    })
}

fn parse_group_block(name: &str, lines: &[&str], ctx: &ParserContext) -> Result<GroupSpec> {
    let mut attrs = collect_block_attrs(lines)?;
    let children = parse_children_attr(&mut attrs, 0, "group")?
        .into_iter()
        .map(|child| ctx.qualify(child))
        .collect();
    let mut transform = parse_transform_attrs(&attrs, &[])?;
    transform.translation = add_vec3(transform.translation, ctx.offset);
    if let Some(axis) = ctx.mirror {
        transform = mirror_transform(transform, axis);
    }
    Ok(GroupSpec {
        name: name.to_string(),
        children,
        transform,
    })
}

fn parse_transform_inline(
    line_no: usize,
    tokens: &[String],
    ctx: &ParserContext,
) -> Result<NamedTransform> {
    if tokens.len() < 2 {
        bail!("line {}: expected 'transform <name> ...'", line_no);
    }
    let attrs = parse_attrs(&tokens[2..]);
    let mut transform = parse_transform_attrs(&attrs, &[])?;
    transform.translation = add_vec3(transform.translation, ctx.offset);
    if let Some(axis) = ctx.mirror {
        transform = mirror_transform(transform, axis);
    }
    Ok(NamedTransform {
        name: ctx.qualify(&tokens[1]),
        transform,
    })
}

fn parse_transform_block(name: &str, lines: &[&str], ctx: &ParserContext) -> Result<NamedTransform> {
    let attrs = collect_block_attrs(lines)?;
    let mut transform = parse_transform_attrs(&attrs, &[])?;
    transform.translation = add_vec3(transform.translation, ctx.offset);
    if let Some(axis) = ctx.mirror {
        transform = mirror_transform(transform, axis);
    }
    Ok(NamedTransform {
        name: name.to_string(),
        transform,
    })
}

fn parse_apply(line_no: usize, tokens: &[String], ctx: &ParserContext) -> Result<ApplySpec> {
    if tokens.len() < 2 {
        bail!("line {}: expected 'apply <transform> to=a,b'", line_no);
    }
    let attrs = parse_attrs(&tokens[2..]);
    let targets = attrs
        .get("to")
        .map(|value| parse_csv(value))
        .unwrap_or_default();
    if targets.is_empty() {
        bail!("line {}: apply requires to=<targets>", line_no);
    }
    Ok(ApplySpec {
        transform: ctx.qualify(&tokens[1]),
        targets: targets.into_iter().map(|target| ctx.qualify(target)).collect(),
    })
}

fn parse_expect_attach(
    line_no: usize,
    tokens: &[String],
    ctx: &ParserContext,
) -> Result<ConstraintSpec> {
    if tokens.len() < 3 {
        bail!("line {}: expected 'expect_attach <left> <right>'", line_no);
    }
    Ok(ConstraintSpec::Attach {
        left: resolve_constraint_target(ctx, &tokens[1]),
        right: resolve_constraint_target(ctx, &tokens[2]),
    })
}

fn parse_expect_ground(
    line_no: usize,
    tokens: &[String],
    ctx: &ParserContext,
) -> Result<ConstraintSpec> {
    if tokens.len() < 2 {
        bail!("line {}: expected 'expect_ground <target>'", line_no);
    }
    Ok(ConstraintSpec::Ground {
        target: resolve_constraint_target(ctx, &tokens[1]),
    })
}

fn parse_expect_intersect(
    line_no: usize,
    tokens: &[String],
    ctx: &ParserContext,
) -> Result<ConstraintSpec> {
    if tokens.len() < 3 {
        bail!("line {}: expected 'expect_intersect <left> <right>'", line_no);
    }
    Ok(ConstraintSpec::Intersect {
        left: resolve_constraint_target(ctx, &tokens[1]),
        right: resolve_constraint_target(ctx, &tokens[2]),
    })
}

fn resolve_constraint_target(ctx: &ParserContext, value: &str) -> String {
    if let Some(global) = value.strip_prefix('@') {
        global.to_string()
    } else {
        ctx.qualify(value)
    }
}

fn parse_boolean(line_no: usize, tokens: &[String], ctx: &ParserContext) -> Result<BooleanSpec> {
    if tokens.len() < 4 {
        bail!(
            "line {}: expected '{} <name> <left> <right>'",
            line_no,
            tokens[0]
        );
    }
    let mut attrs = parse_attrs(&tokens[2..]);
    let name = ctx.qualify(&tokens[1]);

    let left = ctx.qualify(
        attrs
            .remove("left")
            .or_else(|| attrs.remove("base"))
            .unwrap_or_else(|| tokens[2].clone()),
    );
    let right = ctx.qualify(
        attrs
            .remove("right")
            .or_else(|| attrs.remove("tool"))
            .unwrap_or_else(|| tokens[3].clone()),
    );

    let op = match tokens[0].as_str() {
        "union" => BooleanOp::Union,
        "difference" => BooleanOp::Difference,
        "intersection" => BooleanOp::Intersection,
        other => bail!("line {}: unsupported boolean op '{}'", line_no, other),
    };

    let mut transform = parse_transform_attrs(&attrs, &[])?;
    transform.translation = add_vec3(transform.translation, ctx.offset);
    if let Some(axis) = ctx.mirror {
        transform = mirror_transform(transform, axis);
    }

    Ok(BooleanSpec {
        name,
        op,
        left,
        right,
        transform,
    })
}

fn collect_block_attrs(lines: &[&str]) -> Result<HashMap<String, String>> {
    let mut attrs = HashMap::new();
    for (idx, line) in lines.iter().enumerate() {
        for token in tokenize(strip_comments(line).trim())? {
            if let Some((key, value)) = token.split_once('=') {
                attrs.insert(key.to_ascii_lowercase(), value.to_string());
            } else {
                bail!("line {}: block entries must be key=value", idx + 1);
            }
        }
    }
    Ok(attrs)
}

fn parse_attrs(tokens: &[String]) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    for token in tokens {
        if let Some((key, value)) = token.split_once('=') {
            attrs.insert(key.to_ascii_lowercase(), value.to_string());
        }
    }
    attrs
}

fn parse_children_attr(
    attrs: &mut HashMap<String, String>,
    line_no: usize,
    owner: &str,
) -> Result<Vec<String>> {
    let Some(children) = attrs.remove("children") else {
        bail!("line {}: {} requires children=<a,b,...>", line_no, owner);
    };
    let values = parse_csv(&children);
    if values.is_empty() {
        bail!("line {}: {} children cannot be empty", line_no, owner);
    }
    Ok(values)
}

fn parse_transform_attrs(attrs: &HashMap<String, String>, positional: &[String]) -> Result<Transform> {
    let translation = attrs
        .get("at")
        .or_else(|| attrs.get("translate"))
        .or_else(|| attrs.get("position"))
        .map(|value| parse_vec3(value))
        .transpose()?
        .or_else(|| positional.first().map(|value| parse_vec3(value)).transpose().ok().flatten())
        .unwrap_or(Vec3::ZERO);

    let rotation_degrees = attrs
        .get("rotate")
        .or_else(|| attrs.get("rotation"))
        .map(|value| parse_vec3(value))
        .transpose()?
        .unwrap_or(Vec3::ZERO);

    let scale = attrs
        .get("scale")
        .map(|value| parse_scale(value))
        .transpose()?
        .unwrap_or(Vec3::ONE);

    let color = attrs.get("color").map(|value| parse_color(value)).transpose()?;
    let color = match color {
        Some(value) => Some(value),
        None => positional.get(1).map(|value| parse_color(value)).transpose()?,
    };

    Ok(Transform {
        translation,
        rotation_degrees,
        scale,
        color,
    })
}

fn parse_required_scalar(
    command: &str,
    attr_name: &str,
    positional: &[String],
    attrs: &HashMap<String, String>,
    index: usize,
) -> Result<f64> {
    if let Some(value) = attrs.get(attr_name) {
        return parse_f64(value);
    }
    if attr_name == "major_radius" {
        if let Some(value) = attrs.get("major") {
            return parse_f64(value);
        }
    }
    if attr_name == "minor_radius" {
        if let Some(value) = attrs.get("minor") {
            return parse_f64(value);
        }
    }
    if let Some(value) = positional.get(index) {
        return parse_f64(value);
    }
    Err(anyhow!("{} requires {}", command, attr_name))
}

fn parse_required_profile2(
    command: &str,
    attr_name: &str,
    attrs: &HashMap<String, String>,
) -> Result<Vec<Vec2>> {
    attrs
        .get(attr_name)
        .map(|value| parse_profile2(value))
        .transpose()?
        .ok_or_else(|| anyhow!("{} requires {}=<x,y;...>", command, attr_name))
}

fn parse_required_profile3(
    command: &str,
    attr_name: &str,
    attrs: &HashMap<String, String>,
) -> Result<Vec<Vec3>> {
    attrs
        .get(attr_name)
        .map(|value| parse_profile3(value))
        .transpose()?
        .ok_or_else(|| anyhow!("{} requires {}=<x,y,z;...>", command, attr_name))
}

fn parse_required_loft_sections(
    command: &str,
    attr_name: &str,
    attrs: &HashMap<String, String>,
) -> Result<Vec<LoftSection>> {
    attrs
        .get(attr_name)
        .map(|value| parse_loft_sections(value))
        .transpose()?
        .ok_or_else(|| anyhow!("{} requires {}=<z:profile|...>", command, attr_name))
}

fn parse_required_scalar_list(
    command: &str,
    attr_name: &str,
    attrs: &HashMap<String, String>,
) -> Result<Vec<f64>> {
    attrs
        .get(attr_name)
        .map(|value| parse_scalar_list(value))
        .transpose()?
        .ok_or_else(|| anyhow!("{} requires {}=<n;n;...>", command, attr_name))
}

fn parse_positions(value: &str) -> Result<Vec<Vec3>> {
    let mut points = Vec::new();
    for triple in value.split(';') {
        let trimmed = triple.trim();
        if trimmed.is_empty() {
            continue;
        }
        points.push(parse_vec3(trimmed)?);
    }
    if points.is_empty() {
        bail!("positions cannot be empty");
    }
    Ok(points)
}

fn parse_scalar_list(value: &str) -> Result<Vec<f64>> {
    let mut values = Vec::new();
    for item in value.split(';') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        values.push(parse_f64(trimmed)?);
    }
    if values.is_empty() {
        bail!("scalar list cannot be empty");
    }
    Ok(values)
}

fn parse_loft_sections(value: &str) -> Result<Vec<LoftSection>> {
    let mut sections = Vec::new();
    let mut expected_points = None;

    for chunk in value.split('|') {
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((z, profile)) = trimmed.split_once(':') else {
            bail!("invalid loft section '{}': expected z:profile", trimmed);
        };
        let z = parse_f64(z)?;
        let profile = parse_profile2(profile)?;
        if profile.len() < 3 {
            bail!("loft sections require profiles with at least three points");
        }
        match expected_points {
            Some(count) if count != profile.len() => {
                bail!("all loft sections must use the same profile point count")
            }
            None => expected_points = Some(profile.len()),
            _ => {}
        }
        sections.push(LoftSection { z, profile });
    }

    if sections.len() < 2 {
        bail!("loft requires at least two sections");
    }

    Ok(sections)
}

fn parse_f64(value: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .with_context(|| format!("invalid number '{}'", value))
}

fn parse_usize(value: &str) -> Result<usize> {
    value
        .parse::<usize>()
        .with_context(|| format!("invalid integer '{}'", value))
}

fn parse_vec3(value: &str) -> Result<Vec3> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 3 {
        bail!("expected x,y,z vector, got '{}'", value);
    }
    Ok(Vec3(
        parse_f64(parts[0])?,
        parse_f64(parts[1])?,
        parse_f64(parts[2])?,
    ))
}

fn parse_scale(value: &str) -> Result<Vec3> {
    if !value.contains(',') {
        let uniform = parse_f64(value)?;
        return Ok(Vec3(uniform, uniform, uniform));
    }
    parse_vec3(value)
}

fn parse_axis(value: &str) -> Result<Axis> {
    match value.to_ascii_lowercase().as_str() {
        "x" => Ok(Axis::X),
        "y" => Ok(Axis::Y),
        "z" => Ok(Axis::Z),
        _ => bail!("invalid axis '{}'", value),
    }
}

fn parse_mirror_axis(value: &str) -> Result<MirrorAxis> {
    match value.to_ascii_lowercase().as_str() {
        "x" => Ok(MirrorAxis::X),
        "y" => Ok(MirrorAxis::Y),
        "z" => Ok(MirrorAxis::Z),
        _ => bail!("invalid mirror axis '{}'", value),
    }
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|item| {
            let trimmed = item.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect()
}

fn parse_profile2(value: &str) -> Result<Vec<Vec2>> {
    let mut points = Vec::new();
    for pair in value.split(';') {
        let parts: Vec<&str> = pair.split(',').collect();
        if parts.len() != 2 {
            bail!("expected x,y points in '{}'", value);
        }
        points.push(Vec2(parse_f64(parts[0])?, parse_f64(parts[1])?));
    }
    if points.len() < 2 {
        bail!("profile requires at least two points");
    }
    Ok(points)
}

fn parse_profile3(value: &str) -> Result<Vec<Vec3>> {
    let mut points = Vec::new();
    for triple in value.split(';') {
        points.push(parse_vec3(triple)?);
    }
    if points.len() < 2 {
        bail!("path requires at least two points");
    }
    Ok(points)
}

fn parse_color(value: &str) -> Result<Color> {
    let named = match value.to_ascii_lowercase().as_str() {
        "red" => Some(Color(1.0, 0.0, 0.0, 1.0)),
        "green" => Some(Color(0.0, 1.0, 0.0, 1.0)),
        "blue" => Some(Color(0.0, 0.0, 1.0, 1.0)),
        "white" => Some(Color(1.0, 1.0, 1.0, 1.0)),
        "black" => Some(Color(0.0, 0.0, 0.0, 1.0)),
        "yellow" => Some(Color(1.0, 1.0, 0.0, 1.0)),
        "cyan" => Some(Color(0.0, 1.0, 1.0, 1.0)),
        "magenta" => Some(Color(1.0, 0.0, 1.0, 1.0)),
        "orange" => Some(Color(1.0, 0.5, 0.0, 1.0)),
        "purple" => Some(Color(0.5, 0.0, 0.5, 1.0)),
        "gray" | "grey" => Some(Color(0.5, 0.5, 0.5, 1.0)),
        _ => None,
    };
    if let Some(color) = named {
        return Ok(color);
    }

    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }

    let parts: Vec<&str> = value.split(',').collect();
    match parts.len() {
        3 => {
            let values = [
                parse_color_channel(parts[0])?,
                parse_color_channel(parts[1])?,
                parse_color_channel(parts[2])?,
            ];
            Ok(Color(values[0], values[1], values[2], 1.0))
        }
        4 => {
            let values = [
                parse_color_channel(parts[0])?,
                parse_color_channel(parts[1])?,
                parse_color_channel(parts[2])?,
                parse_color_channel(parts[3])?,
            ];
            Ok(Color(values[0], values[1], values[2], values[3]))
        }
        _ => bail!("invalid color '{}'", value),
    }
}

fn parse_color_channel(value: &str) -> Result<f32> {
    if let Ok(int_value) = value.parse::<u8>() {
        return Ok(f32::from(int_value) / 255.0);
    }
    let float_value = value
        .parse::<f32>()
        .with_context(|| format!("invalid color channel '{}'", value))?;
    if !(0.0..=1.0).contains(&float_value) {
        bail!("color channels must be in 0..1 or 0..255");
    }
    Ok(float_value)
}

fn parse_hex_color(hex: &str) -> Result<Color> {
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16)?;
            let g = u8::from_str_radix(&hex[2..4], 16)?;
            let b = u8::from_str_radix(&hex[4..6], 16)?;
            Ok(Color(
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                1.0,
            ))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16)?;
            let g = u8::from_str_radix(&hex[2..4], 16)?;
            let b = u8::from_str_radix(&hex[4..6], 16)?;
            let a = u8::from_str_radix(&hex[6..8], 16)?;
            Ok(Color(
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                f32::from(a) / 255.0,
            ))
        }
        _ => bail!("hex colors must be #RRGGBB or #RRGGBBAA"),
    }
}

fn add_vec3(left: Vec3, right: Vec3) -> Vec3 {
    Vec3(left.0 + right.0, left.1 + right.1, left.2 + right.2)
}

fn mul_vec3(vector: Vec3, scalar: f64) -> Vec3 {
    Vec3(vector.0 * scalar, vector.1 * scalar, vector.2 * scalar)
}

fn mirror_vec3(vector: Vec3, axis: MirrorAxis) -> Vec3 {
    match axis {
        MirrorAxis::X => Vec3(-vector.0, vector.1, vector.2),
        MirrorAxis::Y => Vec3(vector.0, -vector.1, vector.2),
        MirrorAxis::Z => Vec3(vector.0, vector.1, -vector.2),
    }
}

fn mirror_transform(transform: Transform, axis: MirrorAxis) -> Transform {
    let rotation = mirror_rotation(transform.rotation_degrees, axis);
    let scale = match axis {
        MirrorAxis::X => Vec3(-transform.scale.0, transform.scale.1, transform.scale.2),
        MirrorAxis::Y => Vec3(transform.scale.0, -transform.scale.1, transform.scale.2),
        MirrorAxis::Z => Vec3(transform.scale.0, transform.scale.1, -transform.scale.2),
    };
    Transform {
        translation: mirror_vec3(transform.translation, axis),
        rotation_degrees: rotation,
        scale,
        color: transform.color,
    }
}

fn mirror_rotation(rotation_degrees: Vec3, axis: MirrorAxis) -> Vec3 {
    let matrix = rotation_matrix(rotation_degrees);
    let reflected = match axis {
        MirrorAxis::X => mirror_matrix(matrix, [-1.0, 1.0, 1.0]),
        MirrorAxis::Y => mirror_matrix(matrix, [1.0, -1.0, 1.0]),
        MirrorAxis::Z => mirror_matrix(matrix, [1.0, 1.0, -1.0]),
    };
    euler_from_matrix(reflected)
}

fn rotation_matrix(rotation_degrees: Vec3) -> [[f64; 3]; 3] {
    let (sx, cx) = rotation_degrees.0.to_radians().sin_cos();
    let (sy, cy) = rotation_degrees.1.to_radians().sin_cos();
    let (sz, cz) = rotation_degrees.2.to_radians().sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

fn mirror_matrix(matrix: [[f64; 3]; 3], signs: [f64; 3]) -> [[f64; 3]; 3] {
    let mut result = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            result[row][col] = signs[row] * matrix[row][col] * signs[col];
        }
    }
    result
}

fn euler_from_matrix(matrix: [[f64; 3]; 3]) -> Vec3 {
    let sy = -matrix[2][0];
    let y = sy.clamp(-1.0, 1.0).asin();
    let cy = y.cos();
    let (x, z) = if cy.abs() > 1e-8 {
        (
            matrix[2][1].atan2(matrix[2][2]),
            matrix[1][0].atan2(matrix[0][0]),
        )
    } else {
        (0.0, (-matrix[0][1]).atan2(matrix[1][1]))
    };
    Vec3(x.to_degrees(), y.to_degrees(), z.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{ObjectKind, OutputFormat};

    #[test]
    fn parses_positional_sphere() {
        let scene = parse_scene("sphere 1.5 1,2,3 red\n").unwrap();
        assert_eq!(scene.objects.len(), 1);
        let object = &scene.objects[0];
        match object.kind {
            ObjectKind::Sphere { radius } => assert_eq!(radius, 1.5),
            _ => panic!("expected sphere"),
        }
        assert_eq!(object.transform.translation, Vec3(1.0, 2.0, 3.0));
        assert!(scene.uses_color());
    }

    #[test]
    fn parses_named_scene_features() {
        let source = r#"
            sphere ball radius=1 at=0,0,0 color=#ff0000
            cube box size=2 at=2,0,0
            group cluster children=ball,box rotate=0,0,45
            transform lift at=0,0,5
            apply lift to=cluster
            expect_attach ball box
            expect_intersect ball box
            expect_ground box
            union blob ball box
            extrude wall profile=0,0;1,0;1,1;0,1 depth=2
            loft snout sections=0:0,0;1,0;1,1;0,1|1:0.1,0.1;0.9,0.1;0.9,0.9;0.1,0.9
            skin bridge path=0,0,0;0,0,1 radii=0.4;0.2
            revolve vase profile=1,0;0.5,2 axis=z angle=180
            sweep rail profile=0,0;0.25,0;0.25,0.25;0,0.25 path=0,0,0;0,0,2
        "#;
        let scene = parse_scene(source).unwrap();
        scene.validate().unwrap();
        assert_eq!(scene.objects.len(), 7);
        assert_eq!(scene.groups.len(), 1);
        assert_eq!(scene.transforms.len(), 1);
        assert_eq!(scene.applies.len(), 1);
        assert_eq!(scene.constraints.len(), 3);
        assert_eq!(scene.booleans.len(), 1);
    }

    #[test]
    fn expands_repeat_blocks() {
        let source = r#"
            repeat row count=2 step=0,8,0 start=0,-4,0 {
              repeat rack count=3 step=4,0,0 start=-4,0,0 {
                cube node size=2 at=0,0,2 scale=1,0.5,2
              }
            }
        "#;
        let scene = parse_scene(source).unwrap();
        scene.validate().unwrap();
        assert_eq!(scene.objects.len(), 6);
        assert!(scene
            .objects
            .iter()
            .any(|object| object.transform.translation == Vec3(-4.0, -4.0, 2.0)));
        assert!(scene
            .objects
            .iter()
            .any(|object| object.transform.translation == Vec3(4.0, 4.0, 2.0)));
    }

    #[test]
    fn parses_capsule_and_mirror_blocks() {
        let source = r#"
            mirror side axis=x {
              capsule leg radius=0.2 depth=2 at=1,0,1 rotate=5,10,0
            }
        "#;
        let scene = parse_scene(source).unwrap();
        scene.validate().unwrap();
        assert_eq!(scene.objects.len(), 2);
        assert!(scene.objects.iter().any(|object| object.name == "side_pos__leg"));
        assert!(scene.objects.iter().any(|object| object.name == "side_neg__leg"));
        assert!(scene
            .objects
            .iter()
            .any(|object| object.transform.translation == Vec3(1.0, 0.0, 1.0)));
        assert!(scene
            .objects
            .iter()
            .any(|object| object.transform.translation == Vec3(-1.0, 0.0, 1.0)));
        assert!(scene
            .objects
            .iter()
            .all(|object| matches!(object.kind, ObjectKind::Capsule { .. })));
    }

    #[test]
    fn rejects_unknown_group_child() {
        let scene = parse_scene("group g children=missing\n").unwrap();
        let error = scene.validate().unwrap_err().to_string();
        assert!(error.contains("unknown child"));
    }

    #[test]
    fn rejects_stl_for_color_scene() {
        let scene = parse_scene("sphere radius=1 color=red\n").unwrap();
        assert!(scene.uses_color());
        assert!(!OutputFormat::Stl.supports_color());
    }
}
