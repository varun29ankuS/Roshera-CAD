#!/usr/bin/env node
// Failure classifier for agent-eval runs. Reads results.json and each
// episode's Claude Code stream-json, and reports, per episode and in total:
// tool calls, failed calls by KIND, retries, and what the harness's checks
// found -- so the next thing to build is chosen by how often each kind of
// failure actually happens, not by guesswork.
//
// Kinds (first match wins, on the tool result's error_code / text):
//   selection   -- a face/edge/vertex/part reference that was missing, stale,
//                  ambiguous or of the wrong kind
//   gate        -- a harness gate refused (declared intent, unverified work,
//                  unsound base)
//   infeasible  -- a typed refusal saying the operation cannot be done as
//                  asked (blend_failed, boolean_disjoint, ...)
//   untyped     -- kernel_error: the kernel failed and said so only in prose
//   input       -- malformed arguments (missing/invalid field)
//   transport   -- timeouts, connection, rate limits
//   other       -- anything else that came back as an error
// plus `unsound_result`: a call that succeeded but returned sound: false.
//
// usage: node agent-eval/classify.mjs <run dir> [<run dir> ...]

import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

function kindOf(text) {
  const t = text.toLowerCase();
  if (/(face|edge|vertex|part|solid|object)[^.]{0,40}(not found|unknown|stale|ambiguous|no longer|does not exist)|resolve|selector|wrong kind|not a (face|edge)/.test(t))
    return "selection";
  if (/intent|unverified|verification_scope|unsound_base|gate/.test(t)) return "gate";
  if (/blend_failed|disjoint|infeasib|exceeds|radius|too (large|small|thick|thin)|cannot (fillet|chamfer|shell|offset)/.test(t))
    return "infeasible";
  if (/kernel_error|kernel error/.test(t)) return "untyped";
  if (/missing_field|invalid_(field|value|argument)|expected .* got|must be|validation/.test(t)) return "input";
  if (/timeout|timed out|econn|rate limit|429|503/.test(t)) return "transport";
  return "other";
}

function textOf(content) {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.map((c) => c?.text ?? JSON.stringify(c)).join("\n");
  return JSON.stringify(content ?? "");
}

function analyse(streamPath) {
  const out = {
    calls: 0,
    failed: 0,
    kinds: {},
    codes: {},
    retries: 0,
    unsound_results: 0,
    turns: null,
    final: null,
    tools: {},
    examples: [],
  };
  if (!existsSync(streamPath)) return out;
  const uses = new Map();
  let lastTool = null;
  let lastFailed = false;
  for (const line of readFileSync(streamPath, "utf8").split(/\r?\n/)) {
    if (!line.trim()) continue;
    let ev;
    try {
      ev = JSON.parse(line);
    } catch {
      continue;
    }
    const content = ev?.message?.content;
    if (ev.type === "assistant" && Array.isArray(content)) {
      for (const c of content) {
        if (c.type !== "tool_use") continue;
        out.calls += 1;
        const name = String(c.name ?? "").replace(/^mcp__roshera__/, "");
        out.tools[name] = (out.tools[name] ?? 0) + 1;
        uses.set(c.id, name);
        if (lastFailed && lastTool === name) out.retries += 1;
        lastTool = name;
      }
    } else if (ev.type === "user" && Array.isArray(content)) {
      for (const c of content) {
        if (c.type !== "tool_result") continue;
        const text = textOf(c.content);
        const tool = uses.get(c.tool_use_id) ?? "?";
        const code = /"error_code"\s*:\s*"([a-z_]+)"/.exec(text)?.[1];
        const failed = c.is_error === true || /"success"\s*:\s*false/.test(text) || Boolean(code);
        lastFailed = failed;
        if (failed) {
          out.failed += 1;
          const kind = kindOf(`${code ?? ""} ${text}`);
          out.kinds[kind] = (out.kinds[kind] ?? 0) + 1;
          if (code) out.codes[code] = (out.codes[code] ?? 0) + 1;
          if (out.examples.length < 6) {
            out.examples.push({ tool, kind, code: code ?? null, text: text.slice(0, 300) });
          }
        } else if (/"sound"\s*:\s*false/.test(text)) {
          out.unsound_results += 1;
        }
      }
    } else if (ev.type === "result") {
      out.turns = ev.num_turns ?? null;
      out.final = String(ev.result ?? "").slice(0, 200);
      out.subtype = ev.subtype ?? null;
    }
  }
  return out;
}

const dirs = process.argv.slice(2);
if (dirs.length === 0) {
  console.error("usage: classify.mjs <run dir> [...]");
  process.exit(2);
}
for (const dir of dirs) {
  const results = JSON.parse(readFileSync(join(dir, "results.json"), "utf8"));
  const total = { calls: 0, failed: 0, kinds: {}, codes: {}, retries: 0, unsound_results: 0 };
  const failedChecks = {};
  console.log(`\n=== ${dir}`);
  for (const r of results) {
    const a = analyse(r.stream);
    total.calls += a.calls;
    total.failed += a.failed;
    total.retries += a.retries;
    total.unsound_results += a.unsound_results;
    for (const [k, v] of Object.entries(a.kinds)) total.kinds[k] = (total.kinds[k] ?? 0) + v;
    for (const [k, v] of Object.entries(a.codes)) total.codes[k] = (total.codes[k] ?? 0) + v;
    for (const [k, v] of Object.entries(r.checks ?? {})) {
      if (v !== true) failedChecks[k] = (failedChecks[k] ?? 0) + 1;
    }
    console.log(
      `${r.passed ? "PASS" : "FAIL"} ${r.task}#${r.rep}  ${r.seconds}s  turns ${a.turns}  ` +
        `calls ${a.calls}  failed ${a.failed} ${JSON.stringify(a.kinds)}  retries ${a.retries}  ` +
        `checks ${JSON.stringify(r.checks)}  final: ${JSON.stringify(a.final)}`,
    );
    for (const e of a.examples) console.log(`     - [${e.kind}] ${e.tool} ${e.code ?? ""}: ${e.text.replace(/\s+/g, " ")}`);
  }
  const passed = results.filter((r) => r.passed).length;
  console.log(`\nSUMMARY ${passed}/${results.length} passed`);
  console.log(`  tool calls ${total.calls}, failed ${total.failed}, retries ${total.retries}, unsound results ${total.unsound_results}`);
  console.log(`  failed calls by kind: ${JSON.stringify(total.kinds)}`);
  console.log(`  error codes: ${JSON.stringify(total.codes)}`);
  console.log(`  harness checks failed: ${JSON.stringify(failedChecks)}`);
}
