/**
 * Classification of the tools the BACKEND defines (served.ts) — bench,
 * stability and effects, taken from each served definition when it is
 * registered. A leaf module with no imports so registry.ts (metaFor) and
 * gates.ts (the read-only set) can both read it without an import cycle.
 *
 * There is deliberately no hand-kept copy of these values anywhere in
 * roshera-mcp: a served tool is classified by its definition or not at all.
 */

export type ServedEffects = "read_only" | "mutates" | "destructive";

export interface ServedMeta {
  bench: string;
  stability: "stable" | "experimental";
  effects: ServedEffects;
  idempotent: boolean;
}

const SERVED = new Map<string, ServedMeta>();

/** Record a served tool's classification (served.ts, at registration). */
export function setServedMeta(name: string, meta: ServedMeta): void {
  SERVED.set(name, meta);
}

/** A served tool's classification, or undefined for a hand-written tool. */
export function servedMeta(name: string): ServedMeta | undefined {
  return SERVED.get(name);
}

/** Names of every served tool registered in this process. */
export function servedNames(): string[] {
  return [...SERVED.keys()];
}
