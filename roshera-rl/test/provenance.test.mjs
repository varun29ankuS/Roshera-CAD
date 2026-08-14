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
import { resolveKernelIdentity, KernelIdentityConflict } from "../lib/provenance.mjs";

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

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
process.stdout.write(`\nprovenance: ${checks.length} checks passed\n`);
