#!/usr/bin/env python3
"""OpenCascade STEP round-trip validator for the Roshera export engine.

Loads a STEP file through OCP's ``STEPControl_Reader`` (the same
OpenCascade core FreeCAD/most CAD systems use), transfers every root,
and runs ``BRepCheck_Analyzer`` over each resulting solid and every one
of its faces. The script exits:

* ``0`` if every solid is valid AND no face carries a non-``NoError``
  ``BRepCheck`` status (in particular ``BRepCheck_UnorientableShape``,
  enum 27 — the failure the missing-pcurve seam bug produced); and
* non-zero, with a per-solid / per-face diagnostic on stderr, on the
  first violation.

This is the OCC half of the STEP-round-trip verification gate. A Rust
integration test exports a closed/periodic B-spline solid at several
rotations and shells out to this script for each, asserting all pass.

Usage::

    python step_occt_validate.py <file.step> [<file2.step> ...]

If OCP / OpenCascade is not importable the script prints a clear SKIP
message and exits ``0`` (so a CI without OCC does not hard-fail); callers
that need to distinguish "skipped" from "passed" can pass ``--require-occ``
to make an unavailable OCC an error instead.
"""

import sys


def _load_occ():
    """Import the OCP symbols we need, or return ``None`` if unavailable."""
    try:
        from OCP.STEPControl import STEPControl_Reader
        from OCP.IFSelect import IFSelect_ReturnStatus
        from OCP.BRepCheck import BRepCheck_Analyzer, BRepCheck_Status
        from OCP.TopExp import TopExp_Explorer
        from OCP.TopAbs import TopAbs_ShapeEnum
        from OCP.TopoDS import TopoDS
    except Exception as exc:  # noqa: BLE001 - any import failure means no OCC
        return None, str(exc)
    return (
        {
            "STEPControl_Reader": STEPControl_Reader,
            "IFSelect_ReturnStatus": IFSelect_ReturnStatus,
            "BRepCheck_Analyzer": BRepCheck_Analyzer,
            "BRepCheck_Status": BRepCheck_Status,
            "TopExp_Explorer": TopExp_Explorer,
            "TopAbs_ShapeEnum": TopAbs_ShapeEnum,
            "TopoDS": TopoDS,
        },
        None,
    )


def _status_name(occ, status):
    """Human-readable name for a ``BRepCheck_Status`` value."""
    bcs = occ["BRepCheck_Status"]
    for name in dir(bcs):
        if name.startswith("BRepCheck_"):
            try:
                if int(getattr(bcs, name)) == int(status):
                    return name
            except (TypeError, ValueError):
                continue
    return f"status_{int(status)}"


def _face_statuses(occ, analyzer, face):
    """Return the list of non-NoError BRepCheck statuses for ``face``.

    ``BRepCheck_Analyzer.Result(shape).Status()`` yields a list of
    statuses; ``BRepCheck_NoError`` (0) is the all-clear sentinel and is
    filtered out.
    """
    result = analyzer.Result(face)
    bad = []
    try:
        statuses = list(result.Status())
    except Exception:  # noqa: BLE001 - some builds expose an iterator wrapper
        statuses = [s for s in result.Status()]
    for s in statuses:
        if int(s) != 0:  # BRepCheck_NoError == 0
            bad.append(s)
    return bad


def validate_file(occ, path):
    """Validate one STEP file. Returns ``True`` if clean, else ``False``."""
    reader = occ["STEPControl_Reader"]()
    status = reader.ReadFile(path)
    ok_status = occ["IFSelect_ReturnStatus"].IFSelect_RetDone
    if status != ok_status:
        print(f"FAIL  {path}: STEPControl_Reader.ReadFile returned {status}", file=sys.stderr)
        return False

    n_roots = reader.TransferRoots()
    if n_roots == 0:
        print(f"FAIL  {path}: TransferRoots transferred 0 roots", file=sys.stderr)
        return False

    shape = reader.OneShape()
    if shape.IsNull():
        print(f"FAIL  {path}: OneShape() is null", file=sys.stderr)
        return False

    TopExp_Explorer = occ["TopExp_Explorer"]
    TopAbs = occ["TopAbs_ShapeEnum"]
    TopoDS = occ["TopoDS"]

    clean = True
    n_solids = 0
    n_faces = 0

    solid_exp = TopExp_Explorer(shape, TopAbs.TopAbs_SOLID)
    while solid_exp.More():
        n_solids += 1
        solid = TopoDS.Solid_s(solid_exp.Current())
        analyzer = occ["BRepCheck_Analyzer"](solid)

        if not analyzer.IsValid():
            clean = False
            print(
                f"FAIL  {path}: solid #{n_solids} is NOT valid (BRepCheck_Analyzer.IsValid()==False)",
                file=sys.stderr,
            )

        face_exp = TopExp_Explorer(solid, TopAbs.TopAbs_FACE)
        local_face_idx = 0
        while face_exp.More():
            local_face_idx += 1
            n_faces += 1
            face = TopoDS.Face_s(face_exp.Current())
            bad = _face_statuses(occ, analyzer, face)
            if bad:
                clean = False
                names = ", ".join(_status_name(occ, s) for s in bad)
                print(
                    f"FAIL  {path}: solid #{n_solids} face #{local_face_idx} "
                    f"status [{names}]",
                    file=sys.stderr,
                )
            face_exp.Next()
        solid_exp.Next()

    if n_solids == 0:
        print(f"FAIL  {path}: no solids found after transfer", file=sys.stderr)
        return False

    if clean:
        print(f"PASS  {path}: {n_solids} solid(s), {n_faces} face(s) all OCC-valid")
    return clean


def main(argv):
    require_occ = "--require-occ" in argv
    files = [a for a in argv[1:] if not a.startswith("--")]
    if not files:
        print("usage: step_occt_validate.py <file.step> [...]", file=sys.stderr)
        return 2

    occ, err = _load_occ()
    if occ is None:
        msg = f"SKIP  OCP/OpenCascade not available ({err})"
        if require_occ:
            print(msg.replace("SKIP", "FAIL"), file=sys.stderr)
            return 3
        print(msg)
        return 0

    all_ok = True
    for path in files:
        try:
            if not validate_file(occ, path):
                all_ok = False
        except Exception as exc:  # noqa: BLE001 - report, do not crash the gate
            all_ok = False
            print(f"FAIL  {path}: exception during validation: {exc}", file=sys.stderr)

    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
