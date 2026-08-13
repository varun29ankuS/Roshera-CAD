/**
 * The real MCP session: a stdio client against a `roshera-mcp` process
 * PINNED to one document via ROSHERA_DOCUMENT.
 *
 * The pin is what makes parallel episodes possible. Without it every process
 * discovers the globally-`active` document and they all land on the same one
 * (roshera-mcp/src/core.ts::bindSessionDocument).
 *
 * The SDK imports below are DYNAMIC, not top-level. `@modelcontextprotocol/sdk`
 * is not installed in every environment this module gets imported into (in
 * particular: unit tests that inject a fake `spawn` and never touch a real
 * MCP process). The transport is only needed when something actually spawns
 * a session — loading it eagerly at module scope would make the whole
 * episode module unimportable wherever the SDK is absent, which is exactly
 * where the fake-session tests need it to import cleanly.
 */
export async function spawnMcpSession({ documentId, baseUrl, authHeader, mcpEntry }) {
  const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
  const { StdioClientTransport } = await import("@modelcontextprotocol/sdk/client/stdio.js");

  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [mcpEntry ?? "../roshera-mcp/dist/index.js"],
    env: {
      ...process.env,
      ROSHERA_DOCUMENT: documentId,
      ROSHERA_URL: baseUrl,
      ...(authHeader?.Authorization
        ? { ROSHERA_API_KEY: authHeader.Authorization.replace(/^ApiKey /, "") }
        : {}),
    },
  });
  const client = new Client({ name: "roshera-rl", version: "0.1.0" }, { capabilities: {} });
  await client.connect(transport);

  const callJson = async (tool, args) => {
    const res = await client.callTool({ name: tool, arguments: args ?? {} });
    const text = res?.content?.find((c) => c.type === "text")?.text;
    if (typeof text !== "string") return {};
    try { return JSON.parse(text); } catch { return { raw: text }; }
  };

  return {
    call: callJson,
    async claims(taskClaims) {
      const out = [];
      for (const c of taskClaims) {
        const r = await callJson("verify_claim", {
          quantity: c.quantity, expected: c.expected, tolerance: c.tolerance,
        });
        out.push({ name: c.name, verified: r?.verified ?? null, measured: r?.measured ?? null });
      }
      return out;
    },
    async recipeRef() {
      const r = await callJson("recipe_get", {});
      return r?.ref ?? null;
    },
    async close() { await client.close(); },
  };
}
