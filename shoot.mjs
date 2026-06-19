// Render the whole live scene from az/el and save a PNG. Usage: node shoot.mjs <name> <az> <el>
const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";
import { writeFileSync } from "node:fs";
const [name = "scene", az = "35", el = "18", mode = "shaded"] = process.argv.slice(2);
const r = await fetch(`${BASE}/api/agent/scene/orbit?az=${az}&el=${el}&mode=${mode}&size=900`);
const j = await r.json();
if (!j.png_base64) { console.error("no png", Object.keys(j)); process.exit(1); }
writeFileSync(`./_${name}.png`, Buffer.from(j.png_base64, "base64"));
console.log(`_${name}.png  open=${j.open_edges} nm=${j.nonmanifold_edges}`);
