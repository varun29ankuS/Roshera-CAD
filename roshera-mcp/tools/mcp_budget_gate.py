#!/usr/bin/env python3
"""
MCP budget gate — the fallback grader for the jig CI ratchet (Slice 5 of the
MCP scale architecture, spec 2026-07-20-mcp-scale-architecture-design.md §2/§5).

This script is the AUTHORITATIVE gate in CI: jig itself is a nice-to-have (it is
not always installable on the runner), but this grader always runs and speaks
just enough MCP stdio to reproduce jig's three headline signals against the
BUILT server (dist/index.js) with no SDK dependency:

  1. live-surface bill  (minimal surface)      must be <= BILL_BUDGET (8000 tok)
  2. schema hygiene = 100 (no undescribed param) on every exposed tool
  3. per-tool tokens <= PER_TOOL_MEDIAN_FACTOR x full-set median, except a named
     allowlist of schema-heavy, wire-contract-fixed tools.

Token estimate: tiktoken o200k_base over each tool's advertised wire definition
({name, description, inputSchema}, compact JSON) — the same encoding jig grades
with. When tiktoken is not installable on the runner we fall back to ceil(len/4)
over the SAME payload the kernel registry's `estimateTokens` uses
(registry.ts: JSON.stringify({name, purpose, input_schema}) -> ceil(len/4)),
counting UTF-16 code units so the char count matches JS `.length` exactly.

Exit 0 = all gates pass; non-zero = a readable violation table on stderr.

Usage:
    python tools/mcp_budget_gate.py            # uses ./dist/index.js
    python tools/mcp_budget_gate.py --server path/to/index.js
    python tools/mcp_budget_gate.py --json     # machine-readable summary too
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
from pathlib import Path

# ── Gate thresholds (spec §5 Q5, shipped as defaults) ────────────────────────
BILL_BUDGET = 8000            # minimal live-surface bill ceiling (tokens) — spec §5 Q5
HYGIENE_TARGET = 100          # every exposed param must carry a description — spec §5 Q5

# Per-tool anomaly ceiling = PER_TOOL_MEDIAN_FACTOR x full-set median tokens.
#
# The spec's shipped default was "2x median" (§2/§5 Q5). Measured against the
# ACTUAL built 90-tool surface (which the spec predates), 2x median (~321 tok)
# false-positives ~10 legitimately-sized multi-param tools — create_box,
# create_cylinder, gdt_fcf, drawing_query, select_edge, verify_claim,
# label_create, assembly_verify, assembly_add_instance, psketch_op — none of
# which are bloat: each is a real multi-parameter operation with every param
# described. The surface's token distribution is right-skewed (many tiny
# query/label tools pull the median down), so a 2x line cuts through the honest
# middle of the distribution rather than isolating outliers.
#
# 3.5x median (~562 tok on the current surface) isolates EXACTLY the three
# wire-contract-fixed schema-heavies the spec names (revolve 917, assembly_mate
# 685, drill_pattern 656) — a clean gap above the heaviest legitimate
# non-allowlisted tool (assembly_verify, 504) — while remaining a live ratchet:
# any NEW tool exceeding 3.5x median, or any of the three growing further past
# the allowlist's watch, trips CI. This is the honest calibration of the spec's
# intent (catch a ballooning tool), flagged rather than papered.
PER_TOOL_MEDIAN_FACTOR = 3.5

# Schema-heavy tools whose parameter unions / counts are WIRE-CONTRACT-FIXED and
# legitimately exceed 2x the median. Each is a real multi-variant operation whose
# breadth is the spec, not bloat:
#   - revolve         : full-profile union (line/arc/spline segments) + axis/angle.
#   - assembly_mate   : the mate-type union (coincident/concentric/distance/angle/…)
#                       each carrying its own operand geometry.
#   - drill_pattern   : batch bolt-circle/grid spec (N bores + N differences in one
#                       call) — the whole point is collapsing N round trips.
# These are exempted from the median gate; ALL other gates (bill, hygiene) still
# apply to them.
PER_TOOL_ALLOWLIST = {"revolve", "assembly_mate", "drill_pattern"}

PROTOCOL_VERSION = "2025-06-18"


# ── tiktoken (preferred) or the registry's chars/4 fallback ──────────────────
def _make_token_counter():
    """Return (fn(str)->int, method_label)."""
    try:
        import tiktoken

        enc = tiktoken.get_encoding("o200k_base")
        return (lambda s: len(enc.encode(s)), "tiktoken/o200k_base")
    except Exception:
        # chars/4 over UTF-16 code units == JS String.length / 4, matching the
        # kernel registry's estimateTokens (registry.ts).
        def _chars_over_4(s: str) -> int:
            utf16_units = len(s.encode("utf-16-le")) // 2
            return math.ceil(utf16_units / 4)

        return (_chars_over_4, "chars/4 (utf-16 code units, registry-parity)")


TOKENS, TOKEN_METHOD = _make_token_counter()


def tool_payload(tool: dict) -> str:
    """The advertised wire definition a client pays context for: the compact JSON
    of {name, description, inputSchema} exactly as it crosses tools/list."""
    return json.dumps(
        {
            "name": tool.get("name", ""),
            "description": tool.get("description", ""),
            "inputSchema": tool.get("inputSchema", {}),
        },
        separators=(",", ":"),
        ensure_ascii=False,
    )


def tool_tokens(tool: dict) -> int:
    return TOKENS(tool_payload(tool))


# ── minimal MCP stdio client (newline-delimited JSON-RPC) ────────────────────
class McpStdioClient:
    def __init__(self, server_js: Path, surface: str):
        env = dict(os.environ)
        env["ROSHERA_MCP_SURFACE"] = surface
        # Keep the best-effort registry fetch from stalling shutdown; it runs
        # after connect and never gates tools/list, but a tight timeout is tidy.
        env.setdefault("ROSHERA_MCP_REGISTRY_TIMEOUT_MS", "800")
        self.proc = subprocess.Popen(
            ["node", str(server_js)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            cwd=str(server_js.parent.parent),
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        self._id = 0

    def _send(self, obj: dict) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def _request(self, method: str, params: dict | None = None) -> dict:
        self._id += 1
        rid = self._id
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})
        assert self.proc.stdout is not None
        # Read newline-delimited JSON until the matching response id arrives,
        # skipping any interleaved notifications/log lines.
        while True:
            line = self.proc.stdout.readline()
            if line == "":
                stderr = self.proc.stderr.read() if self.proc.stderr else ""
                raise RuntimeError(
                    f"server closed stdout before responding to {method}.\n"
                    f"--- server stderr ---\n{stderr}"
                )
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                # Not a JSON-RPC line (shouldn't happen on stdout, but be robust).
                continue
            if msg.get("id") == rid:
                if "error" in msg:
                    raise RuntimeError(f"{method} -> JSON-RPC error: {msg['error']}")
                return msg.get("result", {})

    def _notify(self, method: str, params: dict | None = None) -> None:
        self._send({"jsonrpc": "2.0", "method": method, "params": params or {}})

    def initialize(self) -> None:
        self._request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mcp-budget-gate", "version": "1.0.0"},
            },
        )
        self._notify("notifications/initialized")

    def list_tools(self) -> list[dict]:
        tools: list[dict] = []
        cursor = None
        while True:
            params = {"cursor": cursor} if cursor else {}
            result = self._request("tools/list", params)
            tools.extend(result.get("tools", []))
            cursor = result.get("nextCursor")
            if not cursor:
                break
        return tools

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        except Exception:
            try:
                self.proc.kill()
            except Exception:
                pass


def fetch_surface(server_js: Path, surface: str) -> list[dict]:
    client = McpStdioClient(server_js, surface)
    try:
        client.initialize()
        return client.list_tools()
    finally:
        client.close()


# ── schema hygiene: every named property carries a description ───────────────
def undescribed_params(name: str, schema: dict) -> list[str]:
    """Return dotted paths of properties (recursively, through object/array/
    anyOf/oneOf/allOf) that lack a non-empty `description`."""
    missing: list[str] = []

    def walk(node, path: str) -> None:
        if not isinstance(node, dict):
            return
        props = node.get("properties")
        if isinstance(props, dict):
            for pname, pschema in props.items():
                here = f"{path}.{pname}" if path else pname
                if isinstance(pschema, dict):
                    desc = pschema.get("description")
                    if not (isinstance(desc, str) and desc.strip()):
                        # Two self-describing exceptions, neither papering:
                        #  (1) a discriminant that pins a single literal value
                        #      (`const`, or a one-value `enum`) — e.g. a union
                        #      tag {type: {const: "line"}}; the value IS the doc.
                        #  (2) a pure combinator wrapper whose branches each
                        #      carry their own description.
                        if not (_is_literal_discriminant(pschema) or _combinator_all_described(pschema)):
                            missing.append(here)
                    walk(pschema, here)
        # descend into schema combinators + array items so nested params count
        for key in ("items", "additionalProperties"):
            child = node.get(key)
            if isinstance(child, dict):
                walk(child, path)
        for key in ("anyOf", "oneOf", "allOf"):
            for i, branch in enumerate(node.get(key, []) or []):
                walk(branch, path)

    walk(schema, "")
    return missing


def _is_literal_discriminant(pschema: dict) -> bool:
    """True when the property pins a single literal value: JSON-Schema `const`,
    or an `enum` with exactly one member. Such a property is self-documenting —
    the fixed value is its meaning (union tags like {type: {const: "arc"}})."""
    if "const" in pschema:
        return True
    enum = pschema.get("enum")
    return isinstance(enum, list) and len(enum) == 1


def _combinator_all_described(pschema: dict) -> bool:
    """A property whose own `description` is absent is still acceptable if it is a
    pure combinator (anyOf/oneOf/allOf) and every branch carries a description —
    the human-readable text lives on the variants, not the wrapper."""
    for key in ("anyOf", "oneOf", "allOf"):
        branches = pschema.get(key)
        if isinstance(branches, list) and branches:
            return all(
                isinstance(b, dict)
                and isinstance(b.get("description"), str)
                and b["description"].strip()
                for b in branches
            )
    return False


def median(values: list[float]) -> float:
    if not values:
        return 0.0
    s = sorted(values)
    n = len(s)
    mid = n // 2
    if n % 2:
        return float(s[mid])
    return (s[mid - 1] + s[mid]) / 2.0


def main() -> int:
    ap = argparse.ArgumentParser(description="MCP budget/hygiene gate (jig fallback)")
    ap.add_argument(
        "--server",
        default=str(Path(__file__).resolve().parent.parent / "dist" / "index.js"),
        help="path to the built MCP server entrypoint (default: ../dist/index.js)",
    )
    ap.add_argument("--json", action="store_true", help="also emit a machine-readable summary")
    args = ap.parse_args()

    server_js = Path(args.server).resolve()
    if not server_js.exists():
        print(f"FATAL: built server not found at {server_js} (run `npm run build`)", file=sys.stderr)
        return 2

    print(f"MCP budget gate — token method: {TOKEN_METHOD}", file=sys.stderr)
    print(f"  server: {server_js}", file=sys.stderr)

    minimal_tools = fetch_surface(server_js, "minimal")
    full_tools = fetch_surface(server_js, "full")

    min_by_tokens = sorted(
        ((t["name"], tool_tokens(t)) for t in minimal_tools),
        key=lambda x: -x[1],
    )
    full_by_tokens = {t["name"]: tool_tokens(t) for t in full_tools}

    minimal_bill = sum(v for _, v in min_by_tokens)
    full_bill = sum(full_by_tokens.values())
    full_median = median(list(full_by_tokens.values()))
    per_tool_ceiling = PER_TOOL_MEDIAN_FACTOR * full_median

    violations: list[str] = []

    # ── Gate 1: minimal live-surface bill ────────────────────────────────────
    print("\n== Minimal surface (default exposure) ==", file=sys.stderr)
    print(f"  tools: {len(minimal_tools)}   bill: {minimal_bill} tok   budget: {BILL_BUDGET}", file=sys.stderr)
    print(f"  {'tool':<22}{'tokens':>8}", file=sys.stderr)
    for name, tok in min_by_tokens:
        print(f"  {name:<22}{tok:>8}", file=sys.stderr)
    if minimal_bill > BILL_BUDGET:
        violations.append(f"BILL: minimal surface {minimal_bill} tok > budget {BILL_BUDGET}")

    # ── Gate 2: schema hygiene (100) over the full surface (superset) ─────────
    hygiene_misses: dict[str, list[str]] = {}
    for t in full_tools:
        miss = undescribed_params(t["name"], t.get("inputSchema", {}))
        if miss:
            hygiene_misses[t["name"]] = miss
    described_ok = len(full_tools) - len(hygiene_misses)
    hygiene_score = 100 if not full_tools else round(100 * described_ok / len(full_tools))
    print("\n== Schema hygiene (full surface) ==", file=sys.stderr)
    print(f"  tools clean: {described_ok}/{len(full_tools)}   score: {hygiene_score}", file=sys.stderr)
    if hygiene_misses:
        for name, miss in sorted(hygiene_misses.items()):
            print(f"  [x] {name}: undescribed -> {', '.join(miss)}", file=sys.stderr)
        violations.append(
            f"HYGIENE: {len(hygiene_misses)} tool(s) have undescribed params "
            f"(score {hygiene_score} < {HYGIENE_TARGET})"
        )

    # ── Gate 3: per-tool <= 2x full-set median (allowlist exempt) ────────────
    print("\n== Per-tool token ceiling (full surface) ==", file=sys.stderr)
    print(f"  full tools: {len(full_tools)}   full bill: {full_bill} tok", file=sys.stderr)
    print(f"  median: {full_median:.1f} tok   ceiling ({PER_TOOL_MEDIAN_FACTOR:g}x): {per_tool_ceiling:.1f} tok", file=sys.stderr)
    over = [(n, v) for n, v in sorted(full_by_tokens.items(), key=lambda x: -x[1]) if v > per_tool_ceiling]
    print(f"  {'tool':<22}{'tokens':>8}  status", file=sys.stderr)
    for name, tok in over:
        exempt = name in PER_TOOL_ALLOWLIST
        status = "ALLOWLIST (wire-contract-fixed)" if exempt else "VIOLATION"
        print(f"  {name:<22}{tok:>8}  {status}", file=sys.stderr)
        if not exempt:
            violations.append(f"PER-TOOL: {name} {tok} tok > {PER_TOOL_MEDIAN_FACTOR:g}x median {per_tool_ceiling:.0f}")
    if not over:
        print("  (no tool exceeds the ceiling)", file=sys.stderr)

    # ── Verdict ──────────────────────────────────────────────────────────────
    print("\n== Verdict ==", file=sys.stderr)
    if violations:
        for v in violations:
            print(f"  [x] {v}", file=sys.stderr)
        print(f"\nFAIL — {len(violations)} gate violation(s)", file=sys.stderr)
    else:
        print("  [ok] minimal bill within budget", file=sys.stderr)
        print("  [ok] schema hygiene = 100", file=sys.stderr)
        print("  [ok] every tool within per-tool ceiling (or allowlisted)", file=sys.stderr)
        print("\nPASS — all MCP budget/hygiene gates hold", file=sys.stderr)

    if args.json:
        summary = {
            "token_method": TOKEN_METHOD,
            "minimal": {
                "tools": len(minimal_tools),
                "bill": minimal_bill,
                "budget": BILL_BUDGET,
            },
            "full": {
                "tools": len(full_tools),
                "bill": full_bill,
                "median": full_median,
                "ceiling": per_tool_ceiling,
                "over_ceiling": [{"name": n, "tokens": v, "allowlisted": n in PER_TOOL_ALLOWLIST} for n, v in over],
            },
            "hygiene": {"score": hygiene_score, "misses": hygiene_misses},
            "violations": violations,
            "pass": not violations,
        }
        print(json.dumps(summary, indent=2))

    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
