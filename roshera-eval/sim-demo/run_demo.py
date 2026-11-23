#!/usr/bin/env python3
"""FEA workflow demo: part -> mesh -> solve -> check against a hand calculation.

A first look at the process (2026-11-23), not the product: the mesher here is
Gmsh, run as a separate program -- one of the open "oracle" meshers our own
Rust mesher will be benchmarked against -- and the solver is CalculiX. Both
ship with FreeCAD 1.0. Two cases with textbook answers, each on a coarse and a
finer mesh so convergence is visible:

  cantilever   100 x 10 x 10 mm bar, fixed at x = 0, 0.1 MPa on its top face
               (w = 1 N/mm, 100 N). Euler-Bernoulli: tip deflection
               w L^4 / (8 E I), root bending stress (w L^2 / 2) c / I.
  plate_hole   200 x 50 x 5 mm plate, central hole d = 10, fixed at x = 0,
               pulled at x = 200 with 10 MPa. Peak stress at the hole edge
               = Kt x net-section stress; Kt from Howland's finite-width fit
               (as tabulated in Peterson's Stress Concentration Factors --
               edition to verify before this is cited outside the demo).

    python -I run_demo.py [--out OUT_DIR]

Writes, per case and mesh: the Gmsh and CalculiX inputs and outputs, a mesh
picture, a stress picture on the deformed shape, and summary.json /
summary.md with every number against its hand calculation.
"""
import argparse
import json
import math
import os
import subprocess
import sys
import time

import numpy as np
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from mpl_toolkits.mplot3d.art3d import Poly3DCollection  # noqa: E402

FREECAD_BIN = os.environ.get("ROSHERA_FREECAD_BIN", r"C:\Program Files\FreeCAD 1.0\bin")
GMSH = os.path.join(FREECAD_BIN, "gmsh.exe")
CCX = os.path.join(FREECAD_BIN, "ccx.exe")

# Aluminium 6061-T6, nominal (MPa, -): to be replaced by the cited material
# library; a demo constant only.
E_MPA = 68900.0
NU = 0.33

# Gmsh 10-node tetrahedron -> CalculiX C3D10: the last two mid-side nodes are
# swapped (Gmsh: 9 = edge 3-4, 10 = edge 2-4; CalculiX: 9 = 2-4, 10 = 3-4).
GMSH_TO_CCX_TET10 = [0, 1, 2, 3, 4, 5, 6, 7, 9, 8]
# CalculiX tetrahedron faces (1-based corner nodes), for *DLOAD Pn.
CCX_TET_FACES = {1: (1, 2, 3), 2: (1, 4, 2), 3: (2, 4, 3), 4: (3, 4, 1)}


# ── Cases ────────────────────────────────────────────────────────────────────

def cantilever_geo(h):
    return f"""SetFactory("OpenCASCADE");
Box(1) = {{0, 0, 0, 100, 10, 10}};
Mesh.ElementOrder = 2;
Mesh.MeshSizeMax = {h};
Mesh.MeshSizeMin = {h / 2};
fixed[] = Surface In BoundingBox{{-0.01, -0.01, -0.01, 0.01, 10.01, 10.01}};
load[] = Surface In BoundingBox{{-0.01, -0.01, 9.99, 100.01, 10.01, 10.01}};
Physical Volume("solid") = {{1}};
Physical Surface("fixed") = {{fixed[]}};
Physical Surface("load") = {{load[]}};
"""


def plate_geo(h, h_hole):
    return f"""SetFactory("OpenCASCADE");
Box(1) = {{0, 0, 0, 200, 50, 5}};
Cylinder(2) = {{100, 25, -1, 0, 0, 7, 5}};
BooleanDifference(3) = {{ Volume{{1}}; Delete; }}{{ Volume{{2}}; Delete; }};
Mesh.ElementOrder = 2;
Mesh.MeshSizeFromPoints = 0;
Mesh.MeshSizeFromCurvature = 0;
Mesh.MeshSizeExtendFromBoundary = 0;
hole[] = Surface In BoundingBox{{94.99, 19.99, -0.01, 105.01, 30.01, 5.01}};
Field[1] = Distance;
Field[1].SurfacesList = {{hole[]}};
Field[2] = Threshold;
Field[2].InField = 1;
Field[2].SizeMin = {h_hole};
Field[2].SizeMax = {h};
Field[2].DistMin = 1;
Field[2].DistMax = 20;
Background Field = 2;
fixed[] = Surface In BoundingBox{{-0.01, -0.01, -0.01, 0.01, 50.01, 5.01}};
load[] = Surface In BoundingBox{{199.99, -0.01, -0.01, 200.01, 50.01, 5.01}};
Physical Volume("solid") = {{3}};
Physical Surface("fixed") = {{fixed[]}};
Physical Surface("load") = {{load[]}};
"""


def cantilever_expected():
    L, b, t, p = 100.0, 10.0, 10.0, 0.1
    w = p * b
    inertia = b * t**3 / 12.0
    return {
        "tip_deflection_mm": w * L**4 / (8.0 * E_MPA * inertia),
        "root_bending_stress_mpa": (w * L**2 / 2.0) * (t / 2.0) / inertia,
        "note": "Euler-Bernoulli; shear deflection adds about 1% at L/h = 10",
    }


def plate_expected():
    W, d, t, s = 50.0, 10.0, 5.0, 10.0
    r = d / W
    kt = 3.0 - 3.13 * r + 3.66 * r**2 - 1.53 * r**3
    s_net = s * W * t / ((W - d) * t)
    return {
        "kt_net": kt,
        "net_stress_mpa": s_net,
        "peak_stress_mpa": kt * s_net,
        "note": "Howland finite-width fit, Kt on the net section",
    }


CASES = {
    "cantilever": {
        "geo": lambda m: cantilever_geo(m["h"]),
        "meshes": [{"name": "coarse", "h": 5.0}, {"name": "fine", "h": 2.5}],
        "pressure": 0.1,  # positive: pushes into the top face (downward)
        "expected": cantilever_expected,
    },
    "plate_hole": {
        "geo": lambda m: plate_geo(m["h"], m["h_hole"]),
        "meshes": [
            {"name": "coarse", "h": 8.0, "h_hole": 1.5},
            {"name": "fine", "h": 6.0, "h_hole": 0.75},
        ],
        "pressure": -10.0,  # negative: pulls the end face (tension)
        "expected": plate_expected,
    },
}


# ── Gmsh .msh (v2 ASCII) ─────────────────────────────────────────────────────

def read_msh2(path):
    with open(path, encoding="utf-8") as f:
        lines = [ln.strip() for ln in f]
    names = {}
    nodes = {}
    elements = []  # (type, physical tag, node ids)
    i = 0
    while i < len(lines):
        if lines[i] == "$PhysicalNames":
            n = int(lines[i + 1])
            for k in range(n):
                dim, tag, name = lines[i + 2 + k].split(maxsplit=2)
                names[name.strip('"')] = int(tag)
            i += n + 2
        elif lines[i] == "$Nodes":
            n = int(lines[i + 1])
            for k in range(n):
                parts = lines[i + 2 + k].split()
                nodes[int(parts[0])] = [float(x) for x in parts[1:4]]
            i += n + 2
        elif lines[i] == "$Elements":
            n = int(lines[i + 1])
            for k in range(n):
                parts = [int(x) for x in lines[i + 2 + k].split()]
                etype, ntags = parts[1], parts[2]
                phys = parts[3] if ntags > 0 else 0
                elements.append((etype, phys, parts[3 + ntags:]))
            i += n + 2
        else:
            i += 1
    return names, nodes, elements


# ── CalculiX input / output ──────────────────────────────────────────────────

def write_inp(path, names, nodes, elements, pressure):
    tets = [e[2] for e in elements if e[0] == 11 and e[1] == names["solid"]]
    def corner_nodes(group):
        out = set()
        for etype, phys, ids in elements:
            if etype == 9 and phys == names[group]:  # 6-node triangle
                out.update(ids[:3])
        return out
    def all_nodes(group):
        out = set()
        for etype, phys, ids in elements:
            if etype == 9 and phys == names[group]:
                out.update(ids)
        return out
    fixed = sorted(all_nodes("fixed"))
    load_corners = corner_nodes("load")
    dloads = []
    with open(path, "w", encoding="utf-8") as f:
        f.write("*HEADING\nRoshera FEA workflow demo\n*NODE, NSET=NALL\n")
        used = sorted({n for t in tets for n in t})
        for n in used:
            x, y, z = nodes[n]
            f.write(f"{n}, {x:.9g}, {y:.9g}, {z:.9g}\n")
        f.write("*ELEMENT, TYPE=C3D10, ELSET=EALL\n")
        for k, t in enumerate(tets, start=1):
            ccx = [t[j] for j in GMSH_TO_CCX_TET10]
            f.write(f"{k}, " + ", ".join(str(n) for n in ccx) + "\n")
            for face, corners in CCX_TET_FACES.items():
                if all(ccx[c - 1] in load_corners for c in corners):
                    dloads.append((k, face))
        f.write("*NSET, NSET=FIX\n")
        for k in range(0, len(fixed), 12):
            f.write(", ".join(str(n) for n in fixed[k:k + 12]) + "\n")
        f.write(f"*MATERIAL, NAME=AL6061\n*ELASTIC\n{E_MPA}, {NU}\n")
        f.write("*SOLID SECTION, ELSET=EALL, MATERIAL=AL6061\n")
        f.write("*STEP\n*STATIC\n*BOUNDARY\nFIX, 1, 3, 0.0\n*DLOAD\n")
        for k, face in dloads:
            f.write(f"{k}, P{face}, {pressure}\n")
        f.write("*NODE FILE\nU\n*EL FILE\nS\n*END STEP\n")
    return {"nodes": len(used), "elements": len(tets), "loaded_faces": len(dloads),
            "fixed_nodes": len(fixed)}


def read_frd(path):
    """Nodal DISP (3) and STRESS (6) from a CalculiX .frd (ASCII)."""
    disp, stress = {}, {}
    block = None
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            if line.startswith(" -4"):
                name = line[5:13].strip()
                block = disp if name == "DISP" else stress if name == "STRESS" else None
            elif line.startswith(" -3"):
                block = None
            elif block is not None and line.startswith(" -1"):
                nid = int(line[3:13])
                vals = [float(line[13 + 12 * k:25 + 12 * k]) for k in range((len(line.rstrip()) - 13) // 12)]
                block[nid] = vals
    return disp, stress


def von_mises(s):
    sxx, syy, szz, sxy, syz, szx = s[:6]
    return math.sqrt(0.5 * ((sxx - syy) ** 2 + (syy - szz) ** 2 + (szz - sxx) ** 2)
                     + 3.0 * (sxy**2 + syz**2 + szx**2))


# ── Pictures ─────────────────────────────────────────────────────────────────

def boundary_triangles(elements):
    """Corner triangles of every 6-node boundary triangle Gmsh wrote."""
    return [ids[:3] for etype, phys, ids in elements if etype == 9]


def surface_mesh(names, nodes, elements):
    """Every boundary triangle of the volume mesh (faces used by one tet)."""
    count = {}
    for etype, phys, ids in elements:
        if etype != 11:
            continue
        a, b, c, d = ids[:4]
        for tri in ((a, b, c), (a, b, d), (a, c, d), (b, c, d)):
            key = tuple(sorted(tri))
            count[key] = count.get(key, 0) + 1
    return [list(k) for k, v in count.items() if v == 1]


def draw(path, nodes, tris, values=None, disp=None, scale=0.0, title="", unit=""):
    fig = plt.figure(figsize=(11, 5.5), dpi=130)
    ax = fig.add_subplot(111, projection="3d")
    def pos(n):
        p = np.array(nodes[n])
        if disp is not None and n in disp:
            p = p + scale * np.array(disp[n][:3])
        return p
    polys = [[pos(n) for n in t] for t in tris]
    if values is None:
        coll = Poly3DCollection(polys, facecolor="#d9dde3", edgecolor="#3a4250", linewidths=0.15)
    else:
        face_vals = np.array([np.mean([values.get(n, 0.0) for n in t]) for t in tris])
        norm = plt.Normalize(face_vals.min(), face_vals.max())
        coll = Poly3DCollection(polys, facecolor=plt.cm.turbo(norm(face_vals)),
                                edgecolor="none")
        sm = plt.cm.ScalarMappable(cmap="turbo", norm=norm)
        fig.colorbar(sm, ax=ax, shrink=0.6, label=unit)
    ax.add_collection3d(coll)
    pts = np.array([p for poly in polys for p in poly])
    lo, hi = pts.min(axis=0), pts.max(axis=0)
    ax.set_xlim(lo[0], hi[0]); ax.set_ylim(lo[1], hi[1]); ax.set_zlim(lo[2], hi[2])
    ax.set_box_aspect(tuple(np.maximum(hi - lo, 1e-6)))
    ax.view_init(elev=22, azim=-60)
    ax.set_axis_off()
    ax.set_title(title, fontsize=10)
    fig.tight_layout()
    fig.savefig(path)
    plt.close(fig)


# ── Run ──────────────────────────────────────────────────────────────────────

def run(cmd, cwd, log):
    env = dict(os.environ)
    env["PATH"] = FREECAD_BIN + os.pathsep + env.get("PATH", "")
    t = time.perf_counter()
    with open(log, "w", encoding="utf-8") as f:
        proc = subprocess.run(cmd, cwd=cwd, env=env, stdout=f, stderr=subprocess.STDOUT, timeout=900)
    if proc.returncode != 0:
        raise RuntimeError(f"{cmd[0]} failed ({proc.returncode}); see {log}")
    return time.perf_counter() - t


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "out"))
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    summary = {}
    for case, spec in CASES.items():
        expected = spec["expected"]()
        summary[case] = {"expected": expected, "meshes": []}
        for m in spec["meshes"]:
            tag = f"{case}_{m['name']}"
            wd = os.path.join(args.out, tag)
            os.makedirs(wd, exist_ok=True)
            geo, msh = os.path.join(wd, "part.geo"), os.path.join(wd, "part.msh")
            with open(geo, "w", encoding="utf-8") as f:
                f.write(spec["geo"](m))
            t_mesh = run([GMSH, geo, "-3", "-format", "msh2", "-o", msh], wd, os.path.join(wd, "gmsh.log"))
            names, nodes, elements = read_msh2(msh)
            stats = write_inp(os.path.join(wd, "job.inp"), names, nodes, elements, spec["pressure"])
            t_solve = run([CCX, "-i", "job"], wd, os.path.join(wd, "ccx.log"))
            disp, stress = read_frd(os.path.join(wd, "job.frd"))
            vm = {n: von_mises(s) for n, s in stress.items()}
            tris = surface_mesh(names, nodes, elements)
            row = dict(stats, mesh=m["name"], mesh_s=round(t_mesh, 2), solve_s=round(t_solve, 2))
            if case == "cantilever":
                tip = [n for n in disp if abs(nodes[n][0] - 100.0) < 1e-6]
                row["tip_deflection_mm"] = -min(disp[n][2] for n in tip)
                # Bending stress at the root, away from the clamped corners:
                # the axial stress on the top/bottom fibres a beam depth in.
                probe = [n for n in stress if abs(nodes[n][0] - 10.0) < 1.3
                         and (abs(nodes[n][2]) < 1e-6 or abs(nodes[n][2] - 10.0) < 1e-6)]
                row["bending_stress_at_x10_mpa"] = max(abs(stress[n][0]) for n in probe) if probe else None
                row["expected_bending_stress_at_x10_mpa"] = (1.0 * 90.0**2 / 2.0) * 5.0 / (10 * 10**3 / 12)
                scale = 20.0
            else:
                edge = [n for n in stress if abs(math.hypot(nodes[n][0] - 100.0, nodes[n][1] - 25.0) - 5.0) < 1e-3]
                row["peak_hole_stress_mpa"] = max(stress[n][0] for n in edge) if edge else None
                scale = 200.0
            row["max_von_mises_mpa"] = max(vm.values())
            summary[case]["meshes"].append(row)
            draw(os.path.join(args.out, f"{tag}_mesh.png"), nodes, tris,
                 title=f"{case} — {m['name']} mesh: {stats['nodes']} nodes, {stats['elements']} quadratic tets")
            draw(os.path.join(args.out, f"{tag}_stress.png"), nodes, tris, values=vm, disp=disp, scale=scale,
                 title=f"{case} — {m['name']}: von Mises stress on the deformed shape (x{scale:g})", unit="MPa")
            print(f"{tag}: {json.dumps(row)}", flush=True)
    with open(os.path.join(args.out, "summary.json"), "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    sys.exit(main())
