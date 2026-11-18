/**
 * The Blackboard line a STEP import posts once the kernel has certified the
 * imported part: the full certificate (the route `verify_part` reads,
 * `/api/agent/parts/{id}/perception?full=1`) as a `roshera:soundness` card,
 * so the board's evidence strip shows the kernel's verdict instead of "no
 * certificate shown" (found rehearsing the demo video, 2026-11-18).
 *
 * `perception` is that route's answer, or `null` when it could not be read:
 * then `sound` is left absent -- not-run is never a pass.
 */
export interface ImportPerception {
  verdict?: string | null
  cert?: Record<string, unknown> | null
}

export function importCertificateLine(name: string, partId: number, perception: ImportPerception | null): string {
  const cert = perception?.cert ?? {}
  const card = {
    ...cert,
    part: name,
    part_id: partId,
    sound: typeof cert.sound === 'boolean' ? cert.sound : null,
    verdict: perception?.verdict ?? 'the kernel certificate could not be read for this part',
  }
  return `Kernel certificate for ${name}\n\n\`\`\`roshera:soundness\n${JSON.stringify(card)}\n\`\`\``
}
