use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    pub objects: Vec<ObjectSpec>,
    pub groups: Vec<GroupSpec>,
    pub booleans: Vec<BooleanSpec>,
    pub transforms: Vec<NamedTransform>,
    pub applies: Vec<ApplySpec>,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            objects: Vec::new(),
            groups: Vec::new(),
            booleans: Vec::new(),
            transforms: Vec::new(),
            applies: Vec::new(),
        }
    }

    pub fn uses_color(&self) -> bool {
        self.objects.iter().any(|item| item.transform.color.is_some())
            || self.groups.iter().any(|item| item.transform.color.is_some())
            || self.transforms.iter().any(|item| item.transform.color.is_some())
    }

    pub fn validate(&self) -> Result<()> {
        let mut node_names = HashSet::new();
        for object in &self.objects {
            if !node_names.insert(object.name.as_str()) {
                bail!("duplicate node name '{}'", object.name);
            }
        }
        for group in &self.groups {
            if !node_names.insert(group.name.as_str()) {
                bail!("duplicate node name '{}'", group.name);
            }
        }
        for boolean in &self.booleans {
            if !node_names.insert(boolean.name.as_str()) {
                bail!("duplicate node name '{}'", boolean.name);
            }
        }

        let mut transform_names = HashSet::new();
        for transform in &self.transforms {
            if !transform_names.insert(transform.name.as_str()) {
                bail!("duplicate transform name '{}'", transform.name);
            }
        }

        let mesh_names: HashSet<&str> = self
            .objects
            .iter()
            .map(|item| item.name.as_str())
            .chain(self.booleans.iter().map(|item| item.name.as_str()))
            .collect();

        for group in &self.groups {
            for child in &group.children {
                if !node_names.contains(child.as_str()) {
                    bail!("group '{}' references unknown child '{}'", group.name, child);
                }
            }
        }

        for boolean in &self.booleans {
            if !mesh_names.contains(boolean.left.as_str()) {
                bail!(
                    "boolean '{}' references unknown or non-mesh left operand '{}'",
                    boolean.name,
                    boolean.left
                );
            }
            if !mesh_names.contains(boolean.right.as_str()) {
                bail!(
                    "boolean '{}' references unknown or non-mesh right operand '{}'",
                    boolean.name,
                    boolean.right
                );
            }
        }

        for apply in &self.applies {
            if !transform_names.contains(apply.transform.as_str()) {
                bail!(
                    "apply references unknown transform '{}'",
                    apply.transform
                );
            }
            for target in &apply.targets {
                if !node_names.contains(target.as_str()) {
                    bail!("apply references unknown target '{}'", target);
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Stl,
    Obj,
    Ply,
    Glb,
}

impl OutputFormat {
    pub fn from_path(path: &Path) -> Result<Self> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow::anyhow!("output path must include an extension"))?;
        match extension.to_ascii_lowercase().as_str() {
            "stl" => Ok(Self::Stl),
            "obj" => Ok(Self::Obj),
            "ply" => Ok(Self::Ply),
            "glb" => Ok(Self::Glb),
            other => bail!("unsupported output format '.{}'", other),
        }
    }

    pub fn supports_color(self) -> bool {
        !matches!(self, Self::Stl)
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Stl => ".stl",
            Self::Obj => ".obj",
            Self::Ply => ".ply",
            Self::Glb => ".glb",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectSpec {
    pub name: String,
    pub kind: ObjectKind,
    pub transform: Transform,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ObjectKind {
    Sphere { radius: f64 },
    Cube { size: f64 },
    Cylinder { radius: f64, depth: f64 },
    Cone { radius: f64, depth: f64 },
    Torus { major_radius: f64, minor_radius: f64 },
    Extrude { profile: Vec<Vec2>, depth: f64 },
    Revolve {
        profile: Vec<Vec2>,
        axis: Axis,
        angle_degrees: f64,
    },
    Sweep { profile: Vec<Vec2>, path: Vec<Vec3> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSpec {
    pub name: String,
    pub children: Vec<String>,
    pub transform: Transform,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedTransform {
    pub name: String,
    pub transform: Transform,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplySpec {
    pub transform: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BooleanSpec {
    pub name: String,
    pub op: BooleanOp,
    pub left: String,
    pub right: String,
    pub transform: Transform,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    Union,
    Difference,
    Intersection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation_degrees: Vec3,
    pub scale: Vec3,
    pub color: Option<Color>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation_degrees: Vec3::ZERO,
            scale: Vec3::ONE,
            color: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Vec2(pub f64, pub f64);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Vec3(pub f64, pub f64, pub f64);

impl Vec3 {
    pub const ZERO: Self = Self(0.0, 0.0, 0.0);
    pub const ONE: Self = Self(1.0, 1.0, 1.0);
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3},{:.3},{:.3},{:.3}", self.0, self.1, self.2, self.3)
    }
}
