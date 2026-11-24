/**
 * What each tool DOES to its world, as MCP tool annotations (spec
 * 2025-06-18+: readOnlyHint, destructiveHint, idempotentHint, openWorldHint).
 * A client reads them to decide what needs the person's approval and what can
 * run freely; an agent reads them to tell a look from a change.
 *
 *   read      -- changes nothing: the part, the document, the sheet, the study
 *                and the history are as they were (readOnlyHint, idempotent).
 *   additive  -- makes something new and leaves everything existing as it was:
 *                a new part, sketch entity, label, checkpoint, datum, file.
 *   (neither) -- changes or removes what exists: a fillet reshapes a part, a
 *                rename replaces a name, a new study replaces the last one, a
 *                delete removes. destructiveHint: true (the spec's default).
 *
 * Backend-served tools declare their own effects (served.ts) and keep them.
 * test/tool_annotations.test.mjs checks every listed name is registered, every
 * read tool is one the refusal cache also treats as read-only (gates.ts), and
 * every delete/clear/remove tool is destructive.
 */

/** Tools that change nothing. */
export const READ_TOOLS: ReadonlySet<string> = new Set([
  // the part
  "get_part", "get_pointer", "list_parts", "mass_properties", "render_part", "scene_view",
  "section_view", "select_edge", "select_face", "user_context", "verify_part",
  "lens_explain", "lens_why_not_sound",
  // asking the geometry
  "dfm_check", "get_face", "get_revolve_profile", "ground_truth", "kb_lookup", "measure_faces",
  "occupancy_view", "part_coverage", "part_distance", "part_features", "point_query", "ray_query",
  "region_query", "verify_claim",
  // reading a sheet
  "drawing_dimensions", "drawing_look", "drawing_query", "drawing_read_semantics", "drawing_spaces",
  "gdt_report", "iso286_limits",
  // reading a study
  "sim_materials", "sim_session",
  // reading a sketch
  "psketch_certify", "psketch_dof", "plane_from_face",
  // reading history
  "rebuild_certificate", "recipe_get", "timeline_dependencies", "timeline_material_change", "timeline_branches", "timeline_checkpoints",
  "timeline_conflicts", "timeline_history", "timeline_scrub",
  // reading labels and the notebook
  "blackboard_list", "label_list", "label_resolve",
  // finding tools
  "describe_tool", "find_tool",
]);

/** Tools that only ADD: nothing that exists is changed or removed. */
export const ADDITIVE_TOOLS: ReadonlySet<string> = new Set([
  // new parts, files, views of the work
  "create_box", "create_cone", "create_cylinder", "create_sphere", "revolve", "nurbs_loft",
  "import_step", "export_part", "guide_attention", "workbench",
  // new sheets and callouts
  "make_drawing", "dimension_part", "drawing_export_sheet", "gdt_datum", "gdt_fcf",
  // sketches being built
  "create_sketch", "sketch_add_shape", "sketch_points", "sketch_extrude", "psketch_begin",
  "psketch_add_entity", "psketch_constrain", "psketch_plane_from_face", "psketch_extrude",
  "psketch_revolve",
  // history markers
  "timeline_checkpoint", "timeline_branch", "bind_parameter_name",
  // names, notes, questions
  "ask_choice", "blackboard_add_entry", "label_create", "propose_labels",
]);

/** The MCP annotations for a hand-written tool, from the two sets above. */
export function annotationsOf(name: string): Record<string, boolean> {
  if (READ_TOOLS.has(name)) {
    return { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false };
  }
  return {
    readOnlyHint: false,
    destructiveHint: !ADDITIVE_TOOLS.has(name),
    openWorldHint: false,
  };
}
