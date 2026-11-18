/**
 * KERNEL LENS — the kernel shows its work. Per face, what the kernel decided
 * (a bound walked reversed on import, an edge gap repaired) and which
 * certificate check the face blocks, with the rule followed and the numbers
 * measured. These tools read the same journal the app's Lens mode shows.
 * They are read-only: in slice 1, repairs are proposed to the user and never
 * applied by the agent.
 */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { BASE, ok, fail, scopeHeaders, readRefusable, PART_ID_DOC } from "../core.js";

/** One event as `GET /api/lens/{part}/face/{face_id}` returns it. */
export interface LensEventWire {
  stage: string;
  kind: string | { blocking: { conjunct: string } };
  step_instance: number | null;
  why: string;
  measured: [string, number][];
}

/** `GET /api/lens/{part}` */
export interface PartLensWire {
  solid_id: number;
  sound: boolean;
  overview: { kind: string; count: number }[];
  faces: { face_id: number; pid: string | null; status: string }[];
  solid_events: LensEventWire[];
}

/** `GET /api/lens/{part}/face/{face_id}` */
export interface FaceLensWire {
  face_id: number;
  pid: string | null;
  status: string;
  events: LensEventWire[];
}

/** Faces whose stories are fetched for one `lens_why_not_sound` call. */
export const MAX_FACES_EXPLAINED = 24;

/** What to look at next for each failing check. Advice only, never a mutation. */
export const NEXT_STEP: Record<string, string> = {
  watertight:
    "The mesh is open along these faces' edges. Look at each face's import events (an edge_tolerance_raised nearby means the file's own edges were apart), then render_part or section_view across the face to see the gap.",
  manifold:
    "A mesh edge here has three or more facets. Usually two faces overlap or a face is duplicated: check the faces sharing that edge.",
  oriented:
    "Facets here wind against a neighbour. If the face came from STEP, its loops may be reversed; the importer's loop_reversed_to_match_neighbours event says whether it already repaired a face nearby.",
  shells_outward:
    "A whole shell encloses volume of the wrong sign: the body is inside out, or a void is filed as a body. No single face is to blame.",
  mesh_quality:
    "A facet on this face crosses its boundary or folds off the surface by more than its chord. The geometry may be right while the mesh is wrong: section_view through the face to see which.",
  tessellation:
    "The display mesh for this face is degenerate or points away from the surface.",
  brep_valid: "The B-Rep fails validation; the certificate's errors list names what.",
  self_intersection_free: "The surface intersects itself; section_view to find where.",
  construction_consistent: "The solid no longer matches its source sketch.",
  eyes_consistent: "A recognised feature refers to a face the solid no longer has.",
};

function conjunctOf(e: LensEventWire): string | null {
  return typeof e.kind === "object" && e.kind !== null && "blocking" in e.kind
    ? e.kind.blocking.conjunct
    : null;
}

/** A failing check, the faces it is on, and what to look at next. */
export interface BlockingGroup {
  conjunct: string;
  next_step: string;
  faces: { face_id: number; pid: string | null; why: string; measured: [string, number][] }[];
  /** Failures no face is blamed for. */
  on_solid: { why: string; measured: [string, number][] }[];
}

/**
 * Group a part's blocking events by failing check. `stories` holds the face
 * route's answer for each blocking face that was fetched (at most
 * `MAX_FACES_EXPLAINED`).
 */
export function groupBlocking(
  part: PartLensWire,
  stories: FaceLensWire[],
): { sound: boolean; groups: BlockingGroup[]; faces_not_fetched: number } {
  const groups = new Map<string, BlockingGroup>();
  const group = (c: string): BlockingGroup => {
    let g = groups.get(c);
    if (!g) {
      g = { conjunct: c, next_step: NEXT_STEP[c] ?? "A certificate check fails.", faces: [], on_solid: [] };
      groups.set(c, g);
    }
    return g;
  };
  for (const s of stories) {
    for (const e of s.events) {
      const c = conjunctOf(e);
      if (c) group(c).faces.push({ face_id: s.face_id, pid: s.pid, why: e.why, measured: e.measured });
    }
  }
  for (const e of part.solid_events) {
    const c = conjunctOf(e);
    if (c) group(c).on_solid.push({ why: e.why, measured: e.measured });
  }
  const blockingFaces = part.faces.filter((f) => f.status === "blocking").length;
  return {
    sound: part.sound,
    groups: [...groups.values()],
    faces_not_fetched: Math.max(0, blockingFaces - stories.length),
  };
}

async function getLens<T>(path: string): Promise<T> {
  const res = await fetch(`${BASE}${path}`, { headers: { ...scopeHeaders() } });
  return (await readRefusable(res, `GET ${path}`)) as T;
}

export function registerLensTools(server: ToolHost) {
  server.tool(
    "lens_explain",
    "KERNEL LENS: what the kernel decided about a part, per face. Without face_id: " +
      "the overview (events by kind) and every face's status (as_written / repaired / " +
      "blocking). With face_id: that face's story, each decision with its rule and " +
      "measured numbers (e.g. an edge gap in mm).",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
      face_id: z.number().int().optional().describe("a face of the part, for its story"),
    },
    async ({ part_id, face_id }) => {
      try {
        const path =
          face_id === undefined
            ? `/api/lens/${part_id}`
            : `/api/lens/${part_id}/face/${face_id}`;
        return ok(await getLens(path));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "lens_why_not_sound",
    "KERNEL LENS: why a part is not sound. The failing certificate checks, each with " +
      "the faces it is on, what was measured, and what to look at next. Read-only: " +
      "propose any repair to the user, do not apply it.",
    { part_id: z.number().int().describe(PART_ID_DOC) },
    async ({ part_id }) => {
      try {
        const part = await getLens<PartLensWire>(`/api/lens/${part_id}`);
        const blocking = part.faces
          .filter((f) => f.status === "blocking")
          .slice(0, MAX_FACES_EXPLAINED);
        const stories: FaceLensWire[] = [];
        for (const f of blocking) {
          stories.push(await getLens<FaceLensWire>(`/api/lens/${part_id}/face/${f.face_id}`));
        }
        return ok({ part_id, ...groupBlocking(part, stories) });
      } catch (e) {
        return fail(e);
      }
    },
  );
}
