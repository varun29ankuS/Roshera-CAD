// Records the agent's drawing show frames for one episode, so the results
// run can be replayed step by step (spec 4b: "slice 4's results run is
// recorded so the replay can be shown").
//
// The look / act / judge frames are emitted by the drawing routes themselves
// (render.png, POST /layout) and broadcast to every open app on the live
// channel (`/ws`, `AgentShow`). They are NOT history -- showing is not
// editing, and timeline undo walks every event, so a stored look would be
// undone in place of a verb. The recorder is therefore an observer, exactly
// like an open app: it connects to `/ws`, keeps the `AgentShow` frames whose
// `drawing_id` is this episode's drawing, and writes them with their arrival
// time and order to `show.jsonl`. Nothing on the server changes.
//
// Auth: under the dev bypass the socket needs nothing; otherwise the recorder
// logs in (ROSHERA_USERNAME / ROSHERA_PASSWORD) and sends the same
// `Authenticate` frame the app does.

import { appendFileSync, writeFileSync } from "node:fs";
import { api, BASE } from "./common.mjs";

/** Whether `frame` (a parsed live-channel message) is a show frame for `drawingId`. */
export function isShowFor(frame, drawingId) {
  return frame?.type === "AgentShow" && frame?.payload?.drawing_id === drawingId;
}

async function token() {
  const username = process.env.ROSHERA_USERNAME;
  const password = process.env.ROSHERA_PASSWORD;
  if (!username || !password) return null;
  const r = await api("POST", "/api/auth/login", { username, password });
  return r.status === 200 ? (r.json?.token ?? null) : null;
}

/**
 * Start recording. -> { stop(): Promise<{ frames, error }> }. Never throws:
 * a recorder that cannot connect says so in its result, and the episode
 * still runs (the frames are the replay, not the score).
 */
export async function recordShow(drawingId, path) {
  writeFileSync(path, "");
  let frames = 0;
  let error = null;
  let ws;
  const started = Date.now();
  try {
    ws = new WebSocket(BASE.replace(/^http/, "ws") + "/ws");
  } catch (e) {
    return { stop: async () => ({ frames: 0, error: `could not open /ws: ${e?.message ?? e}` }) };
  }
  const jwt = await token();
  await new Promise((resolve) => {
    const t = setTimeout(() => {
      error = "the live channel did not open within 10 s";
      resolve();
    }, 10_000);
    ws.addEventListener("open", () => {
      clearTimeout(t);
      if (jwt) ws.send(JSON.stringify({ type: "Authenticate", data: { token: jwt } }));
      resolve();
    });
    ws.addEventListener("error", () => {
      clearTimeout(t);
      error = "the live channel refused the connection";
      resolve();
    });
  });
  ws.addEventListener("message", (ev) => {
    let frame;
    try {
      frame = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    if (!isShowFor(frame, drawingId)) return;
    appendFileSync(path, JSON.stringify({ seq: frames, ms: Date.now() - started, frame: frame.payload }) + "\n");
    frames += 1;
  });
  ws.addEventListener("close", (ev) => {
    if (ev.code !== 1000 && !error) error = `the live channel closed (${ev.code}${ev.reason ? `: ${ev.reason}` : ""})`;
  });
  return {
    stop: async () => {
      // Let the last judge frame of the final verb arrive.
      await new Promise((r) => setTimeout(r, 1500));
      try {
        ws.close(1000);
      } catch {
        // already closed: the close listener recorded why
      }
      return { frames, error, auth: jwt ? "token" : "none (dev bypass expected)" };
    },
  };
}
