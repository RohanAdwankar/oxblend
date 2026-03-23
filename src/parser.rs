use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use shlex::split as shlex_split;

use crate::scene::{
    ApplySpec, Axis, BooleanOp, BooleanSpec, Color, GroupSpec, NamedTransform, ObjectKind,
    ObjectSpec, Scene, Transform, Vec2, Vec3,
};

pub fn parse_scene(source: &str) -> Result<Scene> {
    let mut parser = SceneParser::new();
    parser.parse(source)
}

struct SceneParser {
    anonymous_counter: usize,
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

        while index < lines.len() {
            let line_no = index + 1;
            let line = strip_comments(lines[index]).trim();
            index += 1;
            if line.is_empty() {
                continue;
            }

            if line.ends_with('{') {
                let header = line.trim_end_matches('{').trim();
                let header_tokens = tokenize(header)
                    .with_context(|| format!("line {}: invalid block header", line_no))?;
                if header_tokens.len() != 2 {
                    bail!("line {}: expected '<group|transform> <name> {{'", line_no);
                }

                let mut block_lines = Vec::new();
                loop {
                    if index >= lines.len() {
                        bail!("line {}: unterminated block", line_no);
                    }
                    let body_line_no = index + 1;
                    let body_line = strip_comments(lines[index]).trim();
                    index += 1;
                    if body_line == "}" {
                        break;
                    }
                    if body_line.is_empty() {
                        continue;
                    }
                    block_lines.push((body_line_no, body_line.to_string()));
                }

                match header_tokens[0].as_str() {
                    "group" => {
                        scene
                            .groups
                            .push(parse_group_block(&header_tokens[1], &block_lines)?);
                    }
                    "transform" => {
                        scene
                            .transforms
                            .push(parse_transform_block(&header_tokens[1], &block_lines)?);
                    }
                    other => bail!("line {}: unsupported block '{}'", line_no, other),
                }
                continue;
            }

            let tokens = tokenize(line).with_context(|| format!("line {}: invalid syntax", line_no))?;
            if tokens.is_empty() {
                continue;
            }

            match tokens[0].as_str() {
                "sphere" | "cube" | "cylinder" | "cone" | "torus" | "extrude" | "revolve"
                | "sweep" => {
                    scene.objects.push(self.parse_object(line_no, &tokens)?);
                }
                "group" => scene.groups.push(parse_group_inline(line_no, &tokens)?),
                "transform" => scene.transforms.push(parse_transform_inline(line_no, &tokens)?),
                "apply" => scene.applies.push(parse_apply(line_no, &tokens)?),
                "union" | "difference" | "intersection" => {
                    scene.booleans.push(parse_boolean(line_no, &tokens)?);
                }
                other => bail!("line {}: unknown command '{}'", line_no, other),
            }
        }

        Ok(scene)
    }

    fn next_generated_name(&mut self, prefix: &str) -> String {
        self.anonymous_counter += 1;
        format!("{}_{}", prefix, self.anonymous_counter)
    }

    fn parse_object(&mut self, line_no: usize, tokens: &[String]) -> Result<ObjectSpec> {
        let command = tokens[0].as_str();
        let (name, positional, attrs) = split_name_and_attrs(command, &tokens[1..], || {
            self.next_generated_name(command)
        });

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
        let transform = parse_transform_attrs(&attrs, remaining)?;

        Ok(ObjectSpec {
            name,
            kind,
            transform,
        })
    }
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
            "sphere" | "cube" | "cylinder" | "cone" | "torus" => parse_f64(first).is_err(),
            "extrude" | "revolve" | "sweep" => !first.contains(';') && parse_f64(first).is_err(),
            _ => false,
        };
        if treat_as_name {
            name = Some(positional.remove(0));
        }
    }

    (name.unwrap_or_else(default_name), positional, attrs)
}

fn parse_group_inline(line_no: usize, tokens: &[String]) -> Result<GroupSpec> {
    if tokens.len() < 2 {
        bail!("line {}: expected 'group <name> ...'", line_no);
    }
    let mut attrs = parse_attrs(&tokens[2..]);
    let children = parse_children_attr(&mut attrs, line_no, "group")?;
    Ok(GroupSpec {
        name: tokens[1].clone(),
        children,
        transform: parse_transform_attrs(&attrs, &[])?,
    })
}

fn parse_group_block(name: &str, lines: &[(usize, String)]) -> Result<GroupSpec> {
    let mut attrs = collect_block_attrs(lines)?;
    let children = parse_children_attr(&mut attrs, lines[0].0, "group")?;
    Ok(GroupSpec {
        name: name.to_string(),
        children,
        transform: parse_transform_attrs(&attrs, &[])?,
    })
}

fn parse_transform_inline(line_no: usize, tokens: &[String]) -> Result<NamedTransform> {
    if tokens.len() < 2 {
        bail!("line {}: expected 'transform <name> ...'", line_no);
    }
    let attrs = parse_attrs(&tokens[2..]);
    Ok(NamedTransform {
        name: tokens[1].clone(),
        transform: parse_transform_attrs(&attrs, &[])?,
    })
}

fn parse_transform_block(name: &str, lines: &[(usize, String)]) -> Result<NamedTransform> {
    let attrs = collect_block_attrs(lines)?;
    Ok(NamedTransform {
        name: name.to_string(),
        transform: parse_transform_attrs(&attrs, &[])?,
    })
}

fn parse_apply(line_no: usize, tokens: &[String]) -> Result<ApplySpec> {
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
        transform: tokens[1].clone(),
        targets,
    })
}

fn parse_boolean(line_no: usize, tokens: &[String]) -> Result<BooleanSpec> {
    if tokens.len() < 4 {
        bail!(
            "line {}: expected '{} <name> <left> <right>'",
            line_no,
            tokens[0]
        );
    }
    let mut attrs = parse_attrs(&tokens[2..]);
    let name = tokens[1].clone();

    let left = attrs
        .remove("left")
        .or_else(|| attrs.remove("base"))
        .unwrap_or_else(|| tokens[2].clone());
    let right = attrs
        .remove("right")
        .or_else(|| attrs.remove("tool"))
        .unwrap_or_else(|| tokens[3].clone());

    let op = match tokens[0].as_str() {
        "union" => BooleanOp::Union,
        "difference" => BooleanOp::Difference,
        "intersection" => BooleanOp::Intersection,
        other => bail!("line {}: unsupported boolean op '{}'", line_no, other),
    };

    Ok(BooleanSpec {
        name,
        op,
        left,
        right,
        transform: parse_transform_attrs(&attrs, &[])?,
    })
}

fn collect_block_attrs(lines: &[(usize, String)]) -> Result<HashMap<String, String>> {
    let mut attrs = HashMap::new();
    for (line_no, line) in lines {
        for token in tokenize(line)? {
            if let Some((key, value)) = token.split_once('=') {
                attrs.insert(key.to_ascii_lowercase(), value.to_string());
            } else {
                bail!("line {}: block entries must be key=value", line_no);
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

fn parse_f64(value: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .with_context(|| format!("invalid number '{}'", value))
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
            union blob ball box
            extrude wall profile=0,0;1,0;1,1;0,1 depth=2
            revolve vase profile=1,0;0.5,2 axis=z angle=180
            sweep rail profile=0,0;0.25,0;0.25,0.25;0,0.25 path=0,0,0;0,0,2
        "#;
        let scene = parse_scene(source).unwrap();
        scene.validate().unwrap();
        assert_eq!(scene.objects.len(), 5);
        assert_eq!(scene.groups.len(), 1);
        assert_eq!(scene.transforms.len(), 1);
        assert_eq!(scene.applies.len(), 1);
        assert_eq!(scene.booleans.len(), 1);
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
