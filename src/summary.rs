use std::collections::HashMap;
use std::fmt::Write;

use anyhow::{Result, bail};

use crate::scene::{
    BooleanOp, ObjectKind, Scene, Transform, Vec2, Vec3,
};

pub fn summarize_scene(scene: &Scene) -> Result<String> {
    let mut working = scene.clone();
    apply_named_transforms(&mut working)?;

    let mut mesh_bounds = HashMap::new();
    for object in &working.objects {
        mesh_bounds.insert(object.name.clone(), bounds_for_object(&object.kind, object.transform));
    }
    for boolean in &working.booleans {
        let left = mesh_bounds
            .get(&boolean.left)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("missing left boolean operand '{}'", boolean.left))?;
        let right = mesh_bounds
            .get(&boolean.right)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("missing right boolean operand '{}'", boolean.right))?;
        let combined = bounds_for_boolean(left, right, boolean.op, boolean.transform);
        mesh_bounds.insert(boolean.name.clone(), combined);
    }

    let mut group_bounds = HashMap::new();
    for group in &working.groups {
        let mut children = Vec::new();
        for child in &group.children {
            if let Some(bounds) = mesh_bounds.get(child).copied().or_else(|| group_bounds.get(child).copied()) {
                children.push(bounds);
            } else {
                bail!("group '{}' references missing child '{}'", group.name, child);
            }
        }
        let merged = merge_bounds(&children)
            .map(|bounds| transform_bounds(bounds, group.transform))
            .unwrap_or_else(|| transform_bounds(Bounds::point(Vec3::ZERO), group.transform));
        group_bounds.insert(group.name.clone(), merged);
    }

    let mut all_nodes = Vec::new();
    for object in &working.objects {
        all_nodes.push(NodeSummary {
            name: object.name.clone(),
            kind: object_kind_name(&object.kind).to_string(),
            bounds: mesh_bounds[&object.name],
            detail: object_detail(&object.kind),
        });
    }
    for boolean in &working.booleans {
        all_nodes.push(NodeSummary {
            name: boolean.name.clone(),
            kind: format!("boolean::{:?}", boolean.op).to_lowercase(),
            bounds: mesh_bounds[&boolean.name],
            detail: format!("left={} right={}", boolean.left, boolean.right),
        });
    }
    for group in &working.groups {
        all_nodes.push(NodeSummary {
            name: group.name.clone(),
            kind: "group".to_string(),
            bounds: group_bounds[&group.name],
            detail: format!("children={}", group.children.join(",")),
        });
    }
    all_nodes.sort_by(|a, b| a.name.cmp(&b.name));

    let scene_bounds = merge_bounds(&all_nodes.iter().map(|node| node.bounds).collect::<Vec<_>>())
        .unwrap_or_else(|| Bounds::point(Vec3::ZERO));
    let scene_size = scene_bounds.size();
    let scene_diagonal = magnitude(scene_size);

    let mut output = String::new();
    writeln!(&mut output, "Scene Summary").unwrap();
    writeln!(&mut output, "objects: {}", working.objects.len()).unwrap();
    writeln!(&mut output, "booleans: {}", working.booleans.len()).unwrap();
    writeln!(&mut output, "groups: {}", working.groups.len()).unwrap();
    writeln!(&mut output, "transforms: {}", working.transforms.len()).unwrap();
    writeln!(&mut output, "applies: {}", working.applies.len()).unwrap();
    writeln!(
        &mut output,
        "scene_bounds_min: {}",
        format_vec3(scene_bounds.min)
    )
    .unwrap();
    writeln!(
        &mut output,
        "scene_bounds_max: {}",
        format_vec3(scene_bounds.max)
    )
    .unwrap();
    writeln!(&mut output, "scene_size: {}", format_vec3(scene_size)).unwrap();
    writeln!(&mut output, "scene_diagonal: {:.6}", scene_diagonal).unwrap();
    writeln!(&mut output).unwrap();

    writeln!(&mut output, "Nodes").unwrap();
    for node in &all_nodes {
        let center = node.bounds.center();
        let size = node.bounds.size();
        writeln!(
            &mut output,
            "{} | kind={} | center={} | size={} | min={} | max={} | volume_estimate={:.6} | {}",
            node.name,
            node.kind,
            format_vec3(center),
            format_vec3(size),
            format_vec3(node.bounds.min),
            format_vec3(node.bounds.max),
            node.bounds.volume(),
            node.detail
        )
        .unwrap();
    }

    writeln!(&mut output).unwrap();
    writeln!(&mut output, "Pairwise").unwrap();
    for (idx, left) in all_nodes.iter().enumerate() {
        for right in all_nodes.iter().skip(idx + 1) {
            let center_distance = distance(left.bounds.center(), right.bounds.center());
            let gap_distance = aabb_gap_distance(left.bounds, right.bounds);
            let intersects = bounds_intersect(left.bounds, right.bounds);
            let relative_center = if scene_diagonal > 0.0 {
                center_distance / scene_diagonal
            } else {
                0.0
            };
            let relative_gap = if scene_diagonal > 0.0 {
                gap_distance / scene_diagonal
            } else {
                0.0
            };
            writeln!(
                &mut output,
                "{} <-> {} | intersects={} | center_distance={:.6} | gap_distance={:.6} | center_distance_relative={:.6} | gap_distance_relative={:.6}",
                left.name,
                right.name,
                intersects,
                center_distance,
                gap_distance,
                relative_center,
                relative_gap
            )
            .unwrap();
        }
    }

    Ok(output)
}

#[derive(Clone, Copy)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    fn point(point: Vec3) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    fn center(self) -> Vec3 {
        Vec3(
            (self.min.0 + self.max.0) * 0.5,
            (self.min.1 + self.max.1) * 0.5,
            (self.min.2 + self.max.2) * 0.5,
        )
    }

    fn size(self) -> Vec3 {
        Vec3(
            self.max.0 - self.min.0,
            self.max.1 - self.min.1,
            self.max.2 - self.min.2,
        )
    }

    fn volume(self) -> f64 {
        let size = self.size();
        size.0.abs() * size.1.abs() * size.2.abs()
    }
}

struct NodeSummary {
    name: String,
    kind: String,
    bounds: Bounds,
    detail: String,
}

fn apply_named_transforms(scene: &mut Scene) -> Result<()> {
    let named: HashMap<String, Transform> = scene
        .transforms
        .iter()
        .map(|item| (item.name.clone(), item.transform))
        .collect();

    for apply in &scene.applies {
        let transform = *named
            .get(&apply.transform)
            .ok_or_else(|| anyhow::anyhow!("unknown transform '{}'", apply.transform))?;
        for target in &apply.targets {
            if let Some(object) = scene.objects.iter_mut().find(|item| item.name == *target) {
                object.transform = combine_transform(object.transform, transform);
                continue;
            }
            if let Some(boolean) = scene.booleans.iter_mut().find(|item| item.name == *target) {
                boolean.transform = combine_transform(boolean.transform, transform);
                continue;
            }
            if let Some(group) = scene.groups.iter_mut().find(|item| item.name == *target) {
                group.transform = combine_transform(group.transform, transform);
                continue;
            }
            bail!("unknown apply target '{}'", target);
        }
    }

    Ok(())
}

fn combine_transform(base: Transform, delta: Transform) -> Transform {
    Transform {
        translation: add_vec3(base.translation, delta.translation),
        rotation_degrees: add_vec3(base.rotation_degrees, delta.rotation_degrees),
        scale: mul_vec3(base.scale, delta.scale),
        color: delta.color.or(base.color),
    }
}

fn bounds_for_object(kind: &ObjectKind, transform: Transform) -> Bounds {
    transform_bounds(local_bounds_for_kind(kind), transform)
}

fn local_bounds_for_kind(kind: &ObjectKind) -> Bounds {
    match kind {
        ObjectKind::Sphere { radius } => Bounds {
            min: Vec3(-radius, -radius, -radius),
            max: Vec3(*radius, *radius, *radius),
        },
        ObjectKind::Cube { size } => {
            let half = *size * 0.5;
            Bounds {
                min: Vec3(-half, -half, -half),
                max: Vec3(half, half, half),
            }
        }
        ObjectKind::Cylinder { radius, depth } | ObjectKind::Cone { radius, depth } => Bounds {
            min: Vec3(-radius, -radius, -depth * 0.5),
            max: Vec3(*radius, *radius, *depth * 0.5),
        },
        ObjectKind::Torus {
            major_radius,
            minor_radius,
        } => {
            let outer = major_radius + minor_radius;
            Bounds {
                min: Vec3(-outer, -outer, -*minor_radius),
                max: Vec3(outer, outer, *minor_radius),
            }
        }
        ObjectKind::Extrude { profile, depth } => {
            let (min_x, max_x, min_y, max_y) = profile2_extents(profile);
            Bounds {
                min: Vec3(min_x, min_y, 0.0),
                max: Vec3(max_x, max_y, *depth),
            }
        }
        ObjectKind::Revolve { profile, .. } => {
            let mut max_radius: f64 = 0.0;
            let mut min_height = f64::INFINITY;
            let mut max_height = f64::NEG_INFINITY;
            for point in profile {
                max_radius = max_radius.max(point.0.abs());
                min_height = min_height.min(point.1);
                max_height = max_height.max(point.1);
            }
            Bounds {
                min: Vec3(-max_radius, -max_radius, min_height),
                max: Vec3(max_radius, max_radius, max_height),
            }
        }
        ObjectKind::Sweep { profile, path } => {
            let (pmin_x, pmax_x, pmin_y, pmax_y) = profile2_extents(profile);
            let path_bounds = points3_bounds(path);
            Bounds {
                min: Vec3(
                    path_bounds.min.0 + pmin_x,
                    path_bounds.min.1 + pmin_y,
                    path_bounds.min.2,
                ),
                max: Vec3(
                    path_bounds.max.0 + pmax_x,
                    path_bounds.max.1 + pmax_y,
                    path_bounds.max.2 + (pmax_y - pmin_y).abs(),
                ),
            }
        }
    }
}

fn bounds_for_boolean(left: Bounds, right: Bounds, op: BooleanOp, transform: Transform) -> Bounds {
    let local = match op {
        BooleanOp::Union => union_bounds(left, right),
        BooleanOp::Difference => left,
        BooleanOp::Intersection => intersection_bounds(left, right).unwrap_or_else(|| Bounds::point(left.center())),
    };
    transform_bounds(local, transform)
}

fn transform_bounds(bounds: Bounds, transform: Transform) -> Bounds {
    let corners = [
        Vec3(bounds.min.0, bounds.min.1, bounds.min.2),
        Vec3(bounds.min.0, bounds.min.1, bounds.max.2),
        Vec3(bounds.min.0, bounds.max.1, bounds.min.2),
        Vec3(bounds.min.0, bounds.max.1, bounds.max.2),
        Vec3(bounds.max.0, bounds.min.1, bounds.min.2),
        Vec3(bounds.max.0, bounds.min.1, bounds.max.2),
        Vec3(bounds.max.0, bounds.max.1, bounds.min.2),
        Vec3(bounds.max.0, bounds.max.1, bounds.max.2),
    ];

    let transformed: Vec<Vec3> = corners
        .into_iter()
        .map(|corner| {
            let scaled = Vec3(
                corner.0 * transform.scale.0,
                corner.1 * transform.scale.1,
                corner.2 * transform.scale.2,
            );
            add_vec3(rotate_vec3(scaled, transform.rotation_degrees), transform.translation)
        })
        .collect();
    points3_bounds(&transformed)
}

fn rotate_vec3(point: Vec3, rotation_degrees: Vec3) -> Vec3 {
    let (sx, cx) = rotation_degrees.0.to_radians().sin_cos();
    let (sy, cy) = rotation_degrees.1.to_radians().sin_cos();
    let (sz, cz) = rotation_degrees.2.to_radians().sin_cos();

    let rx = Vec3(point.0, point.1 * cx - point.2 * sx, point.1 * sx + point.2 * cx);
    let ry = Vec3(rx.0 * cy + rx.2 * sy, rx.1, -rx.0 * sy + rx.2 * cy);
    Vec3(ry.0 * cz - ry.1 * sz, ry.0 * sz + ry.1 * cz, ry.2)
}

fn profile2_extents(profile: &[Vec2]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for point in profile {
        min_x = min_x.min(point.0);
        max_x = max_x.max(point.0);
        min_y = min_y.min(point.1);
        max_y = max_y.max(point.1);
    }
    (min_x, max_x, min_y, max_y)
}

fn points3_bounds(points: &[Vec3]) -> Bounds {
    let mut min = Vec3(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in points {
        min.0 = min.0.min(point.0);
        min.1 = min.1.min(point.1);
        min.2 = min.2.min(point.2);
        max.0 = max.0.max(point.0);
        max.1 = max.1.max(point.1);
        max.2 = max.2.max(point.2);
    }
    Bounds { min, max }
}

fn merge_bounds(bounds: &[Bounds]) -> Option<Bounds> {
    if bounds.is_empty() {
        return None;
    }
    let mut merged = bounds[0];
    for bound in bounds.iter().skip(1) {
        merged = union_bounds(merged, *bound);
    }
    Some(merged)
}

fn union_bounds(left: Bounds, right: Bounds) -> Bounds {
    Bounds {
        min: Vec3(
            left.min.0.min(right.min.0),
            left.min.1.min(right.min.1),
            left.min.2.min(right.min.2),
        ),
        max: Vec3(
            left.max.0.max(right.max.0),
            left.max.1.max(right.max.1),
            left.max.2.max(right.max.2),
        ),
    }
}

fn intersection_bounds(left: Bounds, right: Bounds) -> Option<Bounds> {
    let min = Vec3(
        left.min.0.max(right.min.0),
        left.min.1.max(right.min.1),
        left.min.2.max(right.min.2),
    );
    let max = Vec3(
        left.max.0.min(right.max.0),
        left.max.1.min(right.max.1),
        left.max.2.min(right.max.2),
    );
    (min.0 <= max.0 && min.1 <= max.1 && min.2 <= max.2).then_some(Bounds { min, max })
}

fn bounds_intersect(left: Bounds, right: Bounds) -> bool {
    intersection_bounds(left, right).is_some()
}

fn aabb_gap_distance(left: Bounds, right: Bounds) -> f64 {
    let dx = axis_gap(left.min.0, left.max.0, right.min.0, right.max.0);
    let dy = axis_gap(left.min.1, left.max.1, right.min.1, right.max.1);
    let dz = axis_gap(left.min.2, left.max.2, right.min.2, right.max.2);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn axis_gap(min_a: f64, max_a: f64, min_b: f64, max_b: f64) -> f64 {
    if max_a < min_b {
        min_b - max_a
    } else if max_b < min_a {
        min_a - max_b
    } else {
        0.0
    }
}

fn distance(left: Vec3, right: Vec3) -> f64 {
    magnitude(Vec3(left.0 - right.0, left.1 - right.1, left.2 - right.2))
}

fn magnitude(vector: Vec3) -> f64 {
    (vector.0 * vector.0 + vector.1 * vector.1 + vector.2 * vector.2).sqrt()
}

fn add_vec3(left: Vec3, right: Vec3) -> Vec3 {
    Vec3(left.0 + right.0, left.1 + right.1, left.2 + right.2)
}

fn mul_vec3(left: Vec3, right: Vec3) -> Vec3 {
    Vec3(left.0 * right.0, left.1 * right.1, left.2 * right.2)
}

fn format_vec3(value: Vec3) -> String {
    format!("{:.6},{:.6},{:.6}", value.0, value.1, value.2)
}

fn object_kind_name(kind: &ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Sphere { .. } => "sphere",
        ObjectKind::Cube { .. } => "cube",
        ObjectKind::Cylinder { .. } => "cylinder",
        ObjectKind::Cone { .. } => "cone",
        ObjectKind::Torus { .. } => "torus",
        ObjectKind::Extrude { .. } => "extrude",
        ObjectKind::Revolve { .. } => "revolve",
        ObjectKind::Sweep { .. } => "sweep",
    }
}

fn object_detail(kind: &ObjectKind) -> String {
    match kind {
        ObjectKind::Sphere { radius } => format!("radius={radius:.6}"),
        ObjectKind::Cube { size } => format!("size={size:.6}"),
        ObjectKind::Cylinder { radius, depth } => {
            format!("radius={radius:.6} depth={depth:.6}")
        }
        ObjectKind::Cone { radius, depth } => format!("radius={radius:.6} depth={depth:.6}"),
        ObjectKind::Torus {
            major_radius,
            minor_radius,
        } => format!("major_radius={major_radius:.6} minor_radius={minor_radius:.6}"),
        ObjectKind::Extrude { profile, depth } => {
            format!("profile_points={} depth={depth:.6}", profile.len())
        }
        ObjectKind::Revolve {
            profile,
            axis,
            angle_degrees,
        } => format!(
            "profile_points={} axis={:?} angle_degrees={angle_degrees:.6}",
            profile.len(),
            axis
        ),
        ObjectKind::Sweep { profile, path } => {
            format!("profile_points={} path_points={}", profile.len(), path.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_scene;

    #[test]
    fn summarizes_repeat_scene() {
        let scene = parse_scene(
            r#"
            repeat row count=2 step=0,5,0 {
              cube rack size=2 at=0,0,1 scale=1,1,2
            }
            "#,
        )
        .unwrap();
        let summary = summarize_scene(&scene).unwrap();
        assert!(summary.contains("objects: 2"));
        assert!(summary.contains("row_1__rack"));
        assert!(summary.contains("row_2__rack"));
        assert!(summary.contains("Pairwise"));
    }
}
