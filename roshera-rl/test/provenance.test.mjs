/**
 * ATTRIBUTION REFUSES RATHER THAN GUESSES.
 *
 * `kernel_sha` used to be `process.env.ROSHERA_KERNEL_SHA ?? "unknown"` — whatever
 * the operator typed, checked against nothing. A live trajectory recorded
 * "f71773b4" while the running binary had been compiled before that commit
 * existed. Harmless there; a lie in general.
 *
 * So: the harness records what the SERVER reported. If the operator also claimed
 * a build and the two disagree, the batch refuses — a corpus that cannot say
 * which kernel produced it is not training data. And when the server cannot say,
 * the absence is stated with a reason rather than defaulted to a value.
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { resolveKernelIdentity, KernelIdentityConflict, buildProvenance, digestOf } from "../lib/provenance.mjs";
import { defineTask } from "../lib/task.mjs";
import { scriptedPolicy } from "../lib/policy.mjs";

let reply = {};
const stub = http.createServer((req, res) => {
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end(JSON.stringify(reply));
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const baseUrl = `http://127.0.0.1:${stub.address().port}`;

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("the server's build is what gets recorded", async () => {
  reply = { build: { sha: "abc1234", dirty: false } };
  const k = await resolveKernelIdentity({ baseUrl, authHeader: {}, claimed: undefined });
  assert.equal(k.sha, "abc1234");
  assert.equal(k.dirty, false);
  assert.equal(k.reported_by, "server", "never anything but the server");
});

check("an operator claim that AGREES is accepted", async () => {
  reply = { build: { sha: "abc1234", dirty: false } };
  const k = await resolveKernelIdentity({ baseUrl, authHeader: {}, claimed: "abc1234" });
  assert.equal(k.sha, "abc1234");
  assert.equal(k.reported_by, "server");
});

check("an operator claim that DISAGREES refuses the batch", async () => {
  reply = { build: { sha: "abc1234", dirty: false } };
  await assert.rejects(
    () => resolveKernelIdentity({ baseUrl, authHeader: {}, claimed: "deadbee" }),
    (e) => e instanceof KernelIdentityConflict && /deadbee/.test(e.message) && /abc1234/.test(e.message),
    "the refusal must name BOTH builds so the operator can see which is wrong",
  );
});

check("a server that cannot say produces a STATED absence, never a value", async () => {
  reply = { build: { absent: "built without git context" } };
  const k = await resolveKernelIdentity({ baseUrl, authHeader: {}, claimed: undefined });
  assert.ok(!("sha" in k), "no sha may be invented");
  assert.match(k.absent, /git context/);
  assert.equal(k.reported_by, "server");
});

check("an operator claim cannot fill an absence the server declared", async () => {
  reply = { build: { absent: "built without git context" } };
  const k = await resolveKernelIdentity({ baseUrl, authHeader: {}, claimed: "abc1234" });
  assert.ok(!("sha" in k),
    "an operator claim is not evidence — it must never be promoted into a field that means 'the server said so'");
  assert.match(k.absent, /git context/);
});

check("an unreachable server is an absence with a reason, not a throw", async () => {
  const k = await resolveKernelIdentity({
    baseUrl: "http://127.0.0.1:1", authHeader: {}, claimed: undefined,
  });
  assert.ok(!("sha" in k));
  assert.ok(typeof k.absent === "string" && k.absent.length > 0);
});

check("a 200 whose body is not JSON states THAT, never 'could not be reached' — it WAS reached", async () => {
  const badBodyServer = http.createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end("not json{{{");
  });
  badBodyServer.listen(0, "127.0.0.1");
  await once(badBodyServer, "listening");
  const badBaseUrl = `http://127.0.0.1:${badBodyServer.address().port}`;
  try {
    const k = await resolveKernelIdentity({ baseUrl: badBaseUrl, authHeader: {}, claimed: undefined });
    assert.ok(!("sha" in k), "no sha may be invented from an unparseable body");
    assert.match(k.absent, /not valid JSON/i, "must name the real fact: reached, body unparseable");
    assert.doesNotMatch(k.absent, /could not be reached/i, "it WAS reached — a different fact, a different sentence");
    assert.equal(k.reported_by, "server");
  } finally {
    badBodyServer.close();
  }
});

check("a server that never answers times out with a reason naming the timeout, not 'could not be reached'", async () => {
  const slowServer = http.createServer(() => { /* never respond */ });
  slowServer.listen(0, "127.0.0.1");
  await once(slowServer, "listening");
  const slowBaseUrl = `http://127.0.0.1:${slowServer.address().port}`;
  try {
    const k = await resolveKernelIdentity({
      baseUrl: slowBaseUrl, authHeader: {}, claimed: undefined, timeoutMs: 50,
    });
    assert.ok(!("sha" in k));
    assert.match(k.absent, /within 50ms/, "the reason must name the timeout, not a generic unreachability");
    assert.equal(k.reported_by, "server");
  } finally {
    slowServer.close();
  }
});

check("an operator claim differing only in case from the server's sha AGREES, and the server's casing is kept", async () => {
  reply = { build: { sha: "ABC1234", dirty: false } };
  const k = await resolveKernelIdentity({ baseUrl, authHeader: {}, claimed: "abc1234" });
  assert.equal(k.sha, "ABC1234", "the RECORDED sha is the server's own casing, never the operator's");
  assert.equal(k.reported_by, "server");
});

// NOTE: the brief's claim shape (`bindings: { s: "solid:0" }`) predates the
// stricter binding schema `defineTask` now enforces — an array of
// `{var, measure}` closed over verify_claim's five measure kinds
// (task.mjs:57-86). Rewritten here to a valid claim carrying the same name,
// expected value and tolerance, so the test still exercises exactly what it
// says: a real task's `family` field and a digest that moves with tolerance.
const t = defineTask({
  id: "t", family: "fam", prompt: "p", toolAllowlist: ["create_cylinder"],
  claims: [{
    name: "v", expr: "v",
    bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
    expected: 1, tolerance: 0.1,
  }],
  stepBudget: 5, tokenBudget: 100, split: "train",
});

check("a complete block is attributable", async () => {
  const p = await buildProvenance({
    kernel: { sha: "abc1234", dirty: false, reported_by: "server" },
    policy: scriptedPolicy([]), task: t,
    // fileURLToPath, not `.pathname`: on Windows `.pathname` is percent-encoded
    // and carries a leading slash before the drive letter (`/C:/Users/Varun%20…`),
    // which is not a path `fs`/`execFile` can open — it breaks on any account
    // name containing a space, which this machine's does.
    mcpEntry: fileURLToPath(new URL("../package.json", import.meta.url)),
    harnessRoot: fileURLToPath(new URL("..", import.meta.url)),
  });
  assert.equal(p.attributable, true);
  assert.equal(p.kernel.reported_by, "server");
  assert.equal(p.policy.kind, "scripted");
  assert.match(p.policy.script_digest, /^sha256:/);
  assert.match(p.task.digest, /^sha256:/);
  assert.match(p.mcp.dist_digest, /^sha256:/);
});

check("an absent kernel identity makes the whole block UNATTRIBUTABLE", async () => {
  const p = await buildProvenance({
    kernel: { reported_by: "server", absent: "could not be reached" },
    policy: scriptedPolicy([]), task: t,
    // fileURLToPath, not `.pathname`: on Windows `.pathname` is percent-encoded
    // and carries a leading slash before the drive letter (`/C:/Users/Varun%20…`),
    // which is not a path `fs`/`execFile` can open — it breaks on any account
    // name containing a space, which this machine's does.
    mcpEntry: fileURLToPath(new URL("../package.json", import.meta.url)),
    harnessRoot: fileURLToPath(new URL("..", import.meta.url)),
  });
  assert.equal(p.attributable, false,
    "one absent identity makes the row untrustworthy — the flag is the whole point");
  assert.ok(!("sha" in p.kernel));
});

check("an unreadable mcp dist is a stated absence, not a zero digest", async () => {
  const p = await buildProvenance({
    kernel: { sha: "abc1234", dirty: false, reported_by: "server" },
    policy: scriptedPolicy([]), task: t,
    mcpEntry: "C:/definitely/not/here/index.js",
    harnessRoot: fileURLToPath(new URL("..", import.meta.url)),
  });
  assert.ok(!("dist_digest" in p.mcp), "no digest may be invented for a file that was not read");
  assert.ok(typeof p.mcp.absent === "string" && p.mcp.absent.length > 0);
  assert.equal(p.attributable, false);
});

check("the task digest changes when a TOLERANCE changes", () => {
  const a = defineTask({ ...t, claims: [{ ...t.claims[0], tolerance: 0.1 }] });
  const b = defineTask({ ...t, claims: [{ ...t.claims[0], tolerance: 0.2 }] });
  assert.notEqual(digestOf(a), digestOf(b),
    "task ids are stable NAMES, not stable MEANINGS — a changed tolerance is a different task");
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
process.stdout.write(`\nprovenance: ${checks.length} checks passed\n`);
