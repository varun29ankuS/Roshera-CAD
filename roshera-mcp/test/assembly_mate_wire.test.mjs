/**
 * Mate wire proof — `assembly_mate` creates a mate, end to end, against the
 * backend's OWN wire shapes.
 *
 * The defect (2026-11-17): the connector route answers
 * `{ connector_id, instance_id, provenance, frame, radius }`
 * (`api-server/src/assembly_mates.rs::ConnectorResponse`), and the tool read
 * `conn.id`. Both connector ids were therefore `undefined`, JSON dropped them
 * from the mate body, and the mate route refused the body for a missing
 * field. No agent could create a mate, and every attempt left two connectors
 * in the assembly that no mate referenced.
 *
 * This drives the REAL dispatch stack (ToolTable wrapper + gates + the real
 * `assembly_mate` handler + `core.ts` `api()`/`fail()`, compiled from src to
 * test/.build) against a stub that answers as the server does:
 *
 *   (w1) the mate body carries the two ids the connector route returned;
 *   (w2) the result names the mate AND each connector's anchor provenance
 *        (the durability rung the certificate later judges);
 *   (w3) a mate the server refuses leaves NO connector behind: both are
 *        deleted, and the refusal reaches the agent as an error;
 *   (w4) a second connector the server refuses takes the first one with it;
 *   (w5) a connector answer that names no id is refused, never posted on;
 *   (w6) the stub's connector shape is pinned to the Rust source, so a
 *        server-side rename breaks here instead of in an agent's session.
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
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** Every request the stub saw, in order. */
let seen = [];
/** How many connectors the stub has minted (ids are distinct per mint). */
let minted = 0;
/** The connector ids currently alive in the stub's assembly. */
let alive = new Set();
/** When set, the Nth connector POST (1-based) is refused. */
let refuseConnectorNumber = 0;
/** When true, the connector answer carries no id field at all. */
let namelessConnector = false;

const connectorId = (n) => `cccccccc-0000-4000-8000-${String(n).padStart(12, "0")}`;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const body = raw.length ? JSON.parse(raw) : undefined;
    seen.push({ method: req.method, url, body });
    const send = (status, payload, json = true) => {
      res.writeHead(status, { "Content-Type": json ? "application/json" : "text/plain" });
      res.end(json ? JSON.stringify(payload) : payload);
    };

    if (req.method === "POST" && url === `/api/assembly/${ASM}/connector`) {
      minted += 1;
      if (minted === refuseConnectorNumber) {
        return send(400, {
          success: false,
          error_code: "invalid_parameter",
          error: "label 'no-such-face' does not name a face on this instance",
          retryable: false,
        });
      }
      const id = connectorId(minted);
      alive.add(id);
      // assembly_mates.rs::ConnectorResponse, field for field.
      const answer = {
        connector_id: id,
        instance_id: body.instance_id,
        provenance: body.frame ? "Raw" : "Label",
        frame: { origin: [0, 0, 0], z_axis: [0, 0, 1], x_axis: [1, 0, 0] },
        radius: null,
      };
      if (namelessConnector) delete answer.connector_id;
      return send(200, answer);
    }

    const del = url.match(new RegExp(`^/api/assembly/${ASM}/connector/([0-9a-f-]+)$`));
    if (req.method === "DELETE" && del) {
      if (!alive.delete(del[1])) return send(400, { success: false, error: "connector not found" });
      return send(200, { success: true, connector_id: del[1] });
    }

    if (req.method === "POST" && url === `/api/assembly/${ASM}/mate`) {
      // axum's Json extractor: `a: Uuid` / `b: Uuid` are required fields.
      for (const side of ["a", "b"]) {
        if (typeof body?.[side] !== "string" || !UUID_RE.test(body[side])) {
          return send(
            422,
            `Failed to deserialize the JSON body into the target type: missing field \`${side}\``,
            false,
          );
        }
      }
      if (body.kind === "Cam") {
        return send(400, {
          success: false,
          error_code: "invalid_parameter",
          error: "mate kind Cam is refused: not implemented in the solver",
          retryable: false,
        });
      }
      // assembly_mates.rs::create_mate answer.
      return send(200, { mate_id: MATE, kind: body.kind, a: body.a, b: body.b, at: null });
    }

    send(404, {});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});
const reset = () => {
  seen = [];
  minted = 0;
  alive = new Set();
  refuseConnectorNumber = 0;
  namelessConnector = false;
};
const jsonOf = (result) => JSON.parse(result.content[0].text);
const mateArgs = (kind) => ({
  assembly_id: ASM,
  action: "create",
  kind,
  a: { instance_id: INST_A, label: "bore" },
  b: {
    instance_id: INST_B,
    frame: { origin: [0, 0, 0], z_axis: [0, 0, 1], x_axis: [1, 0, 0] },
  },
});

let passed = 0;
const check = async (label, fn) => {
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await check("(w1) the mate body carries the ids the connector route returned", async () => {
  reset();
  const result = await call("assembly_mate", mateArgs({ Revolute: { limits: [-0.1, 0.1] } }));
  assert.notEqual(result.isError, true, result.content[0].text);
  const mate = seen.find((s) => s.method === "POST" && s.url.endsWith("/mate"));
  assert.ok(mate, "the mate route was never reached");
  assert.equal(mate.body.a, connectorId(1), "side A is the first connector the server minted");
  assert.equal(mate.body.b, connectorId(2), "side B is the second connector the server minted");
  assert.deepEqual(mate.body.kind, { Revolute: { limits: [-0.1, 0.1] } });
});

await check("(w2) the result names the mate and each connector's anchor provenance", async () => {
  reset();
  const out = jsonOf(await call("assembly_mate", mateArgs("Fastened")));
  assert.equal(out.mate_id, MATE);
  assert.equal(out.a, connectorId(1));
  assert.equal(out.b, connectorId(2));
  assert.deepEqual(
    [out.connectors.a.connector_id, out.connectors.a.provenance],
    [connectorId(1), "Label"],
  );
  assert.deepEqual(
    [out.connectors.b.connector_id, out.connectors.b.provenance],
    [connectorId(2), "Raw"],
    "a raw frame is reported as raw: the rung certify judges",
  );
  assert.equal(alive.size, 2, "a created mate keeps both its connectors");
});

await check("(w3) a refused mate leaves no connector behind", async () => {
  reset();
  const result = await call("assembly_mate", mateArgs("Cam"));
  assert.equal(result.isError, true, "the server's refusal must reach the agent as an error");
  assert.match(result.content[0].text, /Cam is refused/);
  assert.equal(result.structuredContent?.error_code, "invalid_parameter");
  const deletes = seen.filter((s) => s.method === "DELETE").map((s) => s.url.split("/").pop());
  assert.deepEqual(deletes.sort(), [connectorId(1), connectorId(2)]);
  assert.equal(alive.size, 0, "both connectors made for the refused mate are gone");
});

await check("(w4) a refused second connector takes the first one with it", async () => {
  reset();
  refuseConnectorNumber = 2;
  const result = await call("assembly_mate", mateArgs("Fastened"));
  assert.equal(result.isError, true);
  assert.match(result.content[0].text, /no-such-face/);
  assert.equal(alive.size, 0, "the first connector is not left orphaned");
  assert.equal(
    seen.filter((s) => s.url.endsWith("/mate")).length, 0,
    "no mate is attempted on half a pair",
  );
});

await check("(w5) a connector answer that names no id is refused, never posted on", async () => {
  reset();
  namelessConnector = true;
  const result = await call("assembly_mate", mateArgs("Fastened"));
  assert.equal(result.isError, true);
  assert.match(result.content[0].text, /connector_id/);
  assert.equal(
    seen.filter((s) => s.url.endsWith("/mate")).length, 0,
    "a mate must never be posted with an id the server did not give",
  );
});

await check("(w6) the stub's connector shape is the server's own", async () => {
  const src = readFileSync(
    join(HERE, "..", "..", "roshera-backend", "api-server", "src", "assembly_mates.rs"),
    "utf8",
  );
  const m = src.match(/pub struct ConnectorResponse \{([^}]*)\}/);
  assert.ok(m, "ConnectorResponse is no longer declared in assembly_mates.rs");
  const fields = [...m[1].matchAll(/pub (\w+):/g)].map((f) => f[1]);
  assert.deepEqual(fields, ["connector_id", "instance_id", "provenance", "frame", "radius"]);
  assert.match(src, /"mate_id": mate\.id\.0/, "the mate route no longer answers `mate_id`");
});

stub.close();
console.log(`\nassembly_mate_wire: ${passed} checks passed`);
