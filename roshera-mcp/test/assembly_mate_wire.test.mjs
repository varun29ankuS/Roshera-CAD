/**
 * Mate wire proof — `assembly_mate`, defined by the BACKEND
 * (`roshera-backend/api-server/src/agent_tools/assembly.json`) and registered
 * generically (src/served.ts), against the server's OWN wire shapes.
 *
 * History: on 2026-11-17 the hand-written TypeScript composition read
 * `conn.id` where the connector route answers `connector_id`, so no agent
 * could create a mate; it was fixed by deleting the connectors afterwards,
 * which still left them behind whenever a delete failed. Since 2026-11-18 a
 * mate is ONE call to `POST /api/assembly/{id}/joint`, which adds both
 * connectors and the mate under one lock or nothing at all
 * (`assembly_mates.rs::create_joint_core`). And a pid is a 32-hex string end
 * to end: the form `select_face` and the Kernel Lens report is the form the
 * route reads (`assembly_mates.rs::pid_wire`).
 *
 * This drives the REAL dispatch stack (ToolTable wrapper + gates + the served
 * handler + core.ts `api()`/`fail()`, compiled to test/.build) against a stub
 * that answers as the server does:
 *
 *   (w1) create is ONE request to the joint route, carrying kind, both ends
 *        (the hex pid verbatim) and couples;
 *   (w2) the result is the server's answer: the mate and each connector's
 *        anchor provenance;
 *   (w3) a refused joint reaches the agent as a typed error, and nothing else
 *        is called (no clean-up requests: the server made nothing);
 *   (w4) edit and remove go to the mate routes; a missing mate_id is refused
 *        before any request;
 *   (w5) the schema the agent validates against accepts only the canonical
 *        pid form and refuses an unknown field (the SDK path, validateOp);
 *   (w6) the stub's shapes are pinned to the Rust source, so a server-side
 *        rename breaks here instead of in an agent's session.
 *
 *     npm run test:gates:build && node test/assembly_mate_wire.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const ASM = "aaaaaaaa-0000-4000-8000-000000000001";
const INST_A = "aaaaaaaa-0000-4000-8000-0000000000a1";
const INST_B = "aaaaaaaa-0000-4000-8000-0000000000b1";
const MATE = "aaaaaaaa-0000-4000-8000-00000000000e";
const CONN_A = "cccccccc-0000-4000-8000-000000000001";
const CONN_B = "cccccccc-0000-4000-8000-000000000002";
const PID = "0123456789abcdef0123456789abcdef";

/** Every request the stub saw, in order. */
let seen = [];

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const body = raw.length ? JSON.parse(raw) : undefined;
    seen.push({ method: req.method, url, body });
    const send = (status, payload) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(payload));
    };

    if (req.method === "POST" && url === `/api/assembly/${ASM}/joint`) {
      if (body.kind === "Cam") {
        return send(400, {
          success: false,
          error_code: "invalid_parameter",
          error: "mate kind Cam is refused: not implemented in the solver",
          retryable: false,
          hint: "the mate was refused; its two connectors were not kept",
        });
      }
      const end = (side, id) => ({
        connector_id: id,
        instance_id: side.instance_id,
        provenance: side.frame ? "Raw" : side.pid ? "Pid" : "Label",
        frame: { origin: [0, 0, 0], z_axis: [0, 0, 1], x_axis: [1, 0, 0] },
        radius: null,
      });
      // assembly_mates.rs::JointResponse, field for field.
      return send(200, {
        mate_id: MATE,
        kind: body.kind,
        a: CONN_A,
        b: CONN_B,
        at: [],
        connectors: { a: end(body.a, CONN_A), b: end(body.b, CONN_B) },
      });
    }
    if (req.method === "PATCH" && url === `/api/assembly/${ASM}/mate/${MATE}`) {
      return send(200, { success: true, mate_id: MATE, kind: body.kind });
    }
    if (req.method === "DELETE" && url === `/api/assembly/${ASM}/mate/${MATE}`) {
      return send(200, { success: true, mate_id: MATE });
    }
    send(404, {});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { validateOp } = await import(pathToFileURL(join(HERE, ".build", "metatools.js")).href);
const table = buildTable();
/** Validate as the SDK does (defaults applied), then dispatch the real handler. */
const call = async (args) => {
  const { parsed } = await validateOp(table, "assembly_mate", args);
  return table.get("assembly_mate").handler(parsed, {});
};
const jsonOf = (result) => JSON.parse(result.content[0].text);
const createArgs = (kind) => ({
  assembly_id: ASM,
  kind,
  a: { instance_id: INST_A, pid: PID },
  b: { instance_id: INST_B, frame: { origin: [0, 0, 0], z_axis: [0, 0, 1], x_axis: [1, 0, 0] } },
});

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await check("(w1) create is one request to the joint route", async () => {
  const result = await call(createArgs({ Revolute: { limits: [-0.1, 0.1] } }));
  assert.notEqual(result.isError, true, result.content[0].text);
  assert.equal(seen.length, 1, `exactly one request, got ${seen.map((s) => `${s.method} ${s.url}`)}`);
  const [joint] = seen;
  assert.equal(joint.method, "POST");
  assert.equal(joint.url, `/api/assembly/${ASM}/joint`);
  assert.deepEqual(joint.body.kind, { Revolute: { limits: [-0.1, 0.1] } });
  assert.deepEqual(joint.body.a, { instance_id: INST_A, pid: PID }, "the hex pid goes through verbatim");
  assert.equal(joint.body.couples, null, "absent couples are sent as null, as the route defaults");
});

await check("(w2) the result is the server's answer, connectors included", async () => {
  const result = await call(createArgs("Fastened"));
  const out = jsonOf(result);
  assert.equal(out.mate_id, MATE);
  assert.deepEqual([out.connectors.a.connector_id, out.connectors.a.provenance], [CONN_A, "Pid"]);
  assert.deepEqual([out.connectors.b.connector_id, out.connectors.b.provenance], [CONN_B, "Raw"]);
  assert.deepEqual(result.structuredContent, out, "the same facts as structured content");
});

await check("(w3) a refused joint is a typed error and nothing else is called", async () => {
  const result = await call(createArgs("Cam"));
  assert.equal(result.isError, true, "the server's refusal must reach the agent as an error");
  assert.equal(result.structuredContent?.error_code, "invalid_parameter");
  assert.match(result.structuredContent?.hint ?? "", /connectors were not kept/);
  assert.equal(seen.length, 1, "no clean-up requests: the server made nothing");
});

await check("(w4) edit and remove reach the mate routes; a missing mate_id is refused first", async () => {
  const edited = await call({ assembly_id: ASM, action: "edit", mate_id: MATE, kind: "Fastened" });
  assert.notEqual(edited.isError, true, edited.content[0].text);
  assert.deepEqual([seen[0].method, seen[0].body], ["PATCH", { kind: "Fastened" }]);

  seen = [];
  const removed = await call({ assembly_id: ASM, action: "remove", mate_id: MATE });
  assert.notEqual(removed.isError, true, removed.content[0].text);
  assert.deepEqual([seen[0].method, seen[0].url], ["DELETE", `/api/assembly/${ASM}/mate/${MATE}`]);

  seen = [];
  const refused = await call({ assembly_id: ASM, action: "remove" });
  assert.equal(refused.isError, true);
  assert.deepEqual(refused.structuredContent?.details?.missing, ["mate_id"]);
  assert.equal(seen.length, 0, "nothing is sent for a call that cannot be made");

  const noKind = await call({ assembly_id: ASM, a: createArgs("x").a, b: createArgs("x").b });
  assert.equal(noKind.isError, true);
  assert.deepEqual(noKind.structuredContent?.details?.missing, ["kind"]);
});

await check("(w5) the schema takes only the canonical pid and refuses unknown fields", async () => {
  const bad = { ...createArgs("Fastened"), a: { instance_id: INST_A, pid: "1512366075204170929049582354406559" } };
  await assert.rejects(validateOp(table, "assembly_mate", bad), /pid|Invalid/);
  const upper = { ...createArgs("Fastened"), a: { instance_id: INST_A, pid: PID.toUpperCase() } };
  await assert.rejects(validateOp(table, "assembly_mate", upper));
  const typo = { ...createArgs("Fastened"), a: { instance_id: INST_A, lable: "bore" } };
  await assert.rejects(validateOp(table, "assembly_mate", typo), /lable|Unrecognized/);
  const top = { ...createArgs("Fastened"), mate: MATE };
  await assert.rejects(validateOp(table, "assembly_mate", top), /mate|Unrecognized/);
});

await check("(w6) the stub's shapes are the server's own", async () => {
  const src = readFileSync(
    join(HERE, "..", "..", "roshera-backend", "api-server", "src", "assembly_mates.rs"),
    "utf8",
  );
  const fields = (name) => {
    const m = src.match(new RegExp(`pub struct ${name} \\{([^}]*)\\}`));
    assert.ok(m, `${name} is no longer declared in assembly_mates.rs`);
    return [...m[1].matchAll(/pub (\w+):/g)].map((f) => f[1]);
  };
  assert.deepEqual(fields("JointResponse"), ["mate_id", "kind", "a", "b", "at", "connectors"]);
  assert.deepEqual(fields("JointConnectors"), ["a", "b"]);
  assert.deepEqual(fields("ConnectorResponse"), ["connector_id", "instance_id", "provenance", "frame", "radius"]);
  assert.deepEqual(fields("CreateJointRequest"), ["kind", "a", "b", "couples"]);
  assert.deepEqual(fields("JointEnd"), ["instance_id", "label", "pid", "face_id", "frame"]);
  // The schema's connector fields are exactly the JointEnd fields the route reads.
  const schema = table.get("assembly_mate").served.input_schema;
  assert.deepEqual(Object.keys(schema.properties.a.properties), fields("JointEnd"));
  const main = readFileSync(join(HERE, "..", "..", "roshera-backend", "api-server", "src", "main.rs"), "utf8");
  assert.ok(main.includes('"/api/assembly/{id}/joint"'), "main.rs routes the joint path");
});

stub.close();
console.log(`\nassembly_mate_wire: ${passed} checks passed`);
