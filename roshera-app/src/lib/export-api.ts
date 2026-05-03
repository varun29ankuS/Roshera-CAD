// Shared export pipeline used by the TopBar File menu and the
// ToolBar Export flyout. Both surfaces hit the kernel directly via
// `POST /api/export` so a missing AI key (which would 5xx the NLP
// pipeline) doesn't block deterministic export operations.
//
// Two-step flow:
//   1. POST /api/export         → returns { filename, download_url, ... }
//   2. GET  /api/download/:file → streams the actual bytes
//
// The split lets the server emit large STEP / ROS payloads off disk
// without buffering them into the JSON response.

import { useSceneStore } from '@/stores/scene-store'

// Host root — empty string in dev (Vite proxies /api), full origin in
// production. Routes are prefixed with `/api/...` per request so the
// kernel's REST surface is reachable from both flows.
const API_HOST = import.meta.env.VITE_API_URL || ''

// MIME types per export format. Used both to label the File System
// Access API picker and to set blob.type so the browser's built-in
// "Save As" fallback knows what extension to suggest.
const EXPORT_MIME: Record<string, string> = {
  STL: 'model/stl',
  OBJ: 'model/obj',
  STEP: 'application/step',
  IGES: 'application/iges',
  glTF: 'model/gltf-binary',
  FBX: 'application/octet-stream',
  ROS: 'application/octet-stream',
}

interface FileSystemWritableFileStream {
  write(data: Blob): Promise<void>
  close(): Promise<void>
}
interface FileSystemFileHandle {
  createWritable(): Promise<FileSystemWritableFileStream>
}
interface SaveFilePickerOptions {
  suggestedName?: string
  types?: Array<{ description: string; accept: Record<string, string[]> }>
}
type ShowSaveFilePicker = (
  options?: SaveFilePickerOptions,
) => Promise<FileSystemFileHandle>

export interface ExportResult {
  ok: boolean
  /** Server-supplied filename, e.g. "scene_1234.stl". */
  filename?: string
  /** Human-readable error string when `ok` is false. */
  error?: string
}

/**
 * Export the current selection (or, if nothing is selected, the entire
 * scene) in `format` and prompt the user to save it. Returns `ok: true`
 * on success or user-cancelled save; `ok: false` with an `error` string
 * on any backend or network failure.
 */
export async function exportSceneAs(format: string): Promise<ExportResult> {
  const selectedIds = Array.from(useSceneStore.getState().selectedIds)
  try {
    const exportResp = await fetch(`${API_HOST}/api/export`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        format,
        objects: selectedIds,
        options: { binary: true, include_materials: true, merge_objects: false },
      }),
    })
    if (!exportResp.ok) {
      const detail = await exportResp.text().catch(() => '')
      return {
        ok: false,
        error: `export failed: ${exportResp.status} ${exportResp.statusText}${detail ? ` — ${detail}` : ''}`,
      }
    }
    const meta = (await exportResp.json()) as {
      filename: string
      download_url: string
      file_size: number
      success: boolean
    }
    if (!meta.success || !meta.download_url) {
      return { ok: false, error: 'export response was not successful' }
    }

    const fileResp = await fetch(`${API_HOST}${meta.download_url}`)
    if (!fileResp.ok) {
      return {
        ok: false,
        error: `download failed: ${fileResp.status} ${fileResp.statusText}`,
      }
    }
    const mime = EXPORT_MIME[format] ?? 'application/octet-stream'
    const blob = new Blob([await fileResp.arrayBuffer()], { type: mime })

    const dotIdx = meta.filename.lastIndexOf('.')
    const ext = dotIdx > 0 ? meta.filename.slice(dotIdx) : ''

    // Chromium-based browsers get a real native Save dialog. Firefox /
    // Safari fall through to the anchor download below.
    const picker = (
      window as unknown as { showSaveFilePicker?: ShowSaveFilePicker }
    ).showSaveFilePicker
    if (typeof picker === 'function') {
      try {
        const handle = await picker({
          suggestedName: meta.filename,
          types: ext
            ? [{ description: `${format} file`, accept: { [mime]: [ext] } }]
            : undefined,
        })
        const writable = await handle.createWritable()
        await writable.write(blob)
        await writable.close()
        return { ok: true, filename: meta.filename }
      } catch (err) {
        // User cancelled the picker — silent abort, treat as success.
        // Any other failure falls through to the anchor fallback.
        if (err instanceof DOMException && err.name === 'AbortError') {
          return { ok: true, filename: meta.filename }
        }
        // eslint-disable-next-line no-console
        console.warn('showSaveFilePicker failed, falling back:', err)
      }
    }

    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = meta.filename
    a.click()
    URL.revokeObjectURL(url)
    return { ok: true, filename: meta.filename }
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    return { ok: false, error: msg }
  }
}
