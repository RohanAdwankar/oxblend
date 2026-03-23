import json
import math
import sys
from pathlib import Path

import bpy
import bmesh
from mathutils import Vector, Matrix

DEFAULT_COLOR = (0.72, 0.72, 0.72, 1.0)


def parse_args():
    if "--" not in sys.argv:
        raise SystemExit("expected scene json and output path after --")
    index = sys.argv.index("--")
    args = sys.argv[index + 1 :]
    if len(args) != 3:
        raise SystemExit("usage: blender --python driver.py -- <scene.json> <output> <format>")
    return Path(args[0]), Path(args[1]), args[2]


def load_scene(path):
    return json.loads(path.read_text())


def clear_scene():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)


def vec2_to_3d(point):
    return Vector((point[0], point[1], 0.0))


def apply_transform(obj, transform):
    obj.location = transform["translation"]
    obj.rotation_euler = [math.radians(v) for v in transform["rotation_degrees"]]
    obj.scale = transform["scale"]


def ensure_material(color):
    name = f"oxblend_{color[0]:.3f}_{color[1]:.3f}_{color[2]:.3f}_{color[3]:.3f}"
    material = bpy.data.materials.get(name)
    if material is None:
        material = bpy.data.materials.new(name=name)
        material.use_nodes = True
        principled = material.node_tree.nodes["Principled BSDF"]
        principled.inputs["Base Color"].default_value = color
        principled.inputs["Alpha"].default_value = color[3]
        principled.inputs["Roughness"].default_value = 0.68
    return material


def apply_color(obj, color):
    if obj.type != "MESH":
        return
    material = ensure_material(color)
    if obj.data.materials:
        obj.data.materials[0] = material
    else:
        obj.data.materials.append(material)


def build_primitive(obj_spec):
    kind = obj_spec["kind"]
    kind_type = kind["type"]

    if kind_type == "sphere":
        bpy.ops.mesh.primitive_uv_sphere_add(radius=kind["radius"])
    elif kind_type == "cube":
        bpy.ops.mesh.primitive_cube_add(size=kind["size"])
    elif kind_type == "cylinder":
        bpy.ops.mesh.primitive_cylinder_add(radius=kind["radius"], depth=kind["depth"])
    elif kind_type == "capsule":
        create_capsule(obj_spec["name"], kind["radius"], kind["depth"])
    elif kind_type == "cone":
        bpy.ops.mesh.primitive_cone_add(
            radius1=kind["radius"], radius2=0.0, depth=kind["depth"]
        )
    elif kind_type == "torus":
        bpy.ops.mesh.primitive_torus_add(
            major_radius=kind["major_radius"], minor_radius=kind["minor_radius"]
        )
    elif kind_type == "extrude":
        create_extrusion(obj_spec["name"], kind["profile"], kind["depth"])
    elif kind_type == "revolve":
        create_revolve(obj_spec["name"], kind["profile"], kind["axis"], kind["angle_degrees"])
    elif kind_type == "sweep":
        create_sweep(obj_spec["name"], kind["profile"], kind["path"])
    else:
        raise SystemExit(f"unsupported object type {kind_type}")

    obj = bpy.context.active_object
    obj.name = obj_spec["name"]
    apply_transform(obj, obj_spec["transform"])
    apply_color(obj, obj_spec["transform"]["color"] or DEFAULT_COLOR)
    return obj


def create_mesh_object(name, bm):
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)
    return obj


def create_extrusion(name, profile, depth):
    bm = bmesh.new()
    verts = [bm.verts.new(vec2_to_3d(p)) for p in profile]
    bm.faces.new(verts)
    geom = bmesh.ops.extrude_face_region(bm, geom=bm.faces[:])["geom"]
    extruded_verts = [elem for elem in geom if isinstance(elem, bmesh.types.BMVert)]
    bmesh.ops.translate(bm, verts=extruded_verts, vec=Vector((0.0, 0.0, depth)))
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    create_mesh_object(name, bm)


def create_capsule(name, radius, depth):
    body_depth = max(depth - radius * 2.0, 0.0)
    pieces = []

    if body_depth > 1e-6:
        bpy.ops.mesh.primitive_cylinder_add(radius=radius, depth=body_depth)
        pieces.append(bpy.context.active_object)

    offset = max(depth * 0.5 - radius, 0.0)
    for z in (offset, -offset):
        bpy.ops.mesh.primitive_uv_sphere_add(radius=radius, location=(0.0, 0.0, z))
        pieces.append(bpy.context.active_object)

    bpy.ops.object.select_all(action="DESELECT")
    for piece in pieces:
        piece.select_set(True)
    bpy.context.view_layer.objects.active = pieces[0]
    bpy.ops.object.join()
    bpy.context.active_object.name = name


def create_revolve(name, profile, axis, angle_degrees):
    bm = bmesh.new()
    points = []
    for point in profile:
        radius, height = point
        if axis == "x":
            points.append(Vector((height, radius, 0.0)))
        elif axis == "y":
            points.append(Vector((radius, height, 0.0)))
        else:
            points.append(Vector((radius, 0.0, height)))
    geom_verts = [bm.verts.new(point) for point in points]
    geom_edges = [
        bm.edges.new((geom_verts[i], geom_verts[i + 1])) for i in range(len(geom_verts) - 1)
    ]
    if axis == "x":
        spin_axis = Vector((1.0, 0.0, 0.0))
    elif axis == "y":
        spin_axis = Vector((0.0, 1.0, 0.0))
    else:
        spin_axis = Vector((0.0, 0.0, 1.0))
    bmesh.ops.spin(
        bm,
        geom=geom_verts + geom_edges,
        cent=(0.0, 0.0, 0.0),
        axis=spin_axis,
        angle=math.radians(angle_degrees),
        steps=max(12, int(abs(angle_degrees) / 15)),
    )
    bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=0.0001)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    create_mesh_object(name, bm)


def create_poly_curve(name, points, dimensions):
    curve = bpy.data.curves.new(name=name, type="CURVE")
    curve.dimensions = dimensions
    spline = curve.splines.new("POLY")
    spline.points.add(len(points) - 1)
    for point_ref, point in zip(spline.points, points):
        if len(point) == 2:
            point_ref.co = (point[0], point[1], 0.0, 1.0)
        else:
            point_ref.co = (point[0], point[1], point[2], 1.0)
    obj = bpy.data.objects.new(name, curve)
    bpy.context.collection.objects.link(obj)
    return obj


def create_sweep(name, profile, path):
    path_obj = create_poly_curve(f"{name}_path", path, "3D")
    profile_closed = profile + [profile[0]]
    profile_obj = create_poly_curve(f"{name}_profile", profile_closed, "2D")
    profile_obj.data.fill_mode = "BOTH"
    path_obj.data.bevel_mode = "OBJECT"
    path_obj.data.bevel_object = profile_obj
    bpy.context.view_layer.objects.active = path_obj
    path_obj.select_set(True)
    bpy.ops.object.convert(target="MESH")
    path_obj.name = name
    profile_obj.hide_set(True)


def duplicate_object(source, name):
    duplicate = source.copy()
    duplicate.data = source.data.copy()
    duplicate.name = name
    bpy.context.collection.objects.link(duplicate)
    return duplicate


def apply_group_color(group_obj, color):
    for child in group_obj.children_recursive:
        apply_color(child, color)


def export_scene(output_path, output_format):
    bpy.ops.object.select_all(action="DESELECT")
    for obj in bpy.context.scene.objects:
        if obj.type in {"MESH", "EMPTY"}:
            obj.select_set(True)
    bpy.context.view_layer.objects.active = bpy.context.selected_objects[0]

    if output_format == ".stl":
        bpy.ops.wm.stl_export(filepath=str(output_path), export_selected_objects=True)
    elif output_format == ".obj":
        bpy.ops.wm.obj_export(filepath=str(output_path), export_selected_objects=True)
    elif output_format == ".ply":
        bpy.ops.wm.ply_export(filepath=str(output_path), export_selected_objects=True)
    elif output_format == ".glb":
        bpy.ops.export_scene.gltf(
            filepath=str(output_path), export_format="GLB", use_selection=True
        )
    else:
        raise SystemExit(f"unsupported output format {output_format}")


def main():
    scene_path, output_path, output_format = parse_args()
    payload = load_scene(scene_path)
    clear_scene()

    nodes = {}
    transforms = {item["name"]: item["transform"] for item in payload["transforms"]}

    for obj_spec in payload["objects"]:
        nodes[obj_spec["name"]] = build_primitive(obj_spec)

    for group_spec in payload["groups"]:
        group = bpy.data.objects.new(group_spec["name"], None)
        bpy.context.collection.objects.link(group)
        apply_transform(group, group_spec["transform"])
        nodes[group_spec["name"]] = group
        for child_name in group_spec["children"]:
            nodes[child_name].parent = group
        if group_spec["transform"]["color"] is not None:
            apply_group_color(group, group_spec["transform"]["color"])

    for boolean in payload["booleans"]:
        left = nodes[boolean["left"]]
        right = nodes[boolean["right"]]
        result = duplicate_object(left, boolean["name"])
        modifier = result.modifiers.new(name=f"{boolean['name']}_bool", type="BOOLEAN")
        modifier.object = right
        modifier.operation = boolean["op"].upper()
        bpy.context.view_layer.objects.active = result
        result.select_set(True)
        bpy.ops.object.modifier_apply(modifier=modifier.name)
        apply_transform(result, boolean["transform"])
        apply_color(result, boolean["transform"]["color"] or DEFAULT_COLOR)
        nodes[boolean["name"]] = result

    for apply in payload["applies"]:
        transform = transforms[apply["transform"]]
        for target_name in apply["targets"]:
            target = nodes[target_name]
            target.location = Vector(target.location) + Vector(transform["translation"])
            target.rotation_euler = [
                a + math.radians(b)
                for a, b in zip(target.rotation_euler, transform["rotation_degrees"])
            ]
            target.scale = Vector(
                [a * b for a, b in zip(target.scale, transform["scale"])]
            )
            if transform["color"] is not None:
                if target.type == "EMPTY":
                    apply_group_color(target, transform["color"])
                else:
                    apply_color(target, transform["color"])

    export_scene(output_path, output_format)


if __name__ == "__main__":
    main()
