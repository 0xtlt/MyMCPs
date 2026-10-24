// A minimal MCP server over streamable HTTP, for the device of
// headscale-e2e.sh: it answers initialize, tools/list and tools/call with
// plain JSON and keeps no session. It has no dependency.
const http = require("node:http");
const os = require("node:os");

const port = Number(process.env.PORT || 8931);
const tools = [
  {
    name: "whoami",
    description: "Names the device this MCP server runs on.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "echo",
    description: "Returns its text, with the name of the device.",
    inputSchema: {
      type: "object",
      properties: { text: { type: "string" } },
      required: ["text"],
      additionalProperties: false,
    },
  },
];

function reply(message, id) {
  switch (message.method) {
    case "initialize":
      return {
        jsonrpc: "2.0",
        id,
        result: {
          protocolVersion: message.params?.protocolVersion || "2025-03-26",
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: "tailnet-e2e-device", version: "1.0.0" },
        },
      };
    case "ping":
      return { jsonrpc: "2.0", id, result: {} };
    case "tools/list":
      return { jsonrpc: "2.0", id, result: { tools } };
    case "tools/call": {
      const { name, arguments: args = {} } = message.params || {};
      const host = os.hostname();
      if (name === "whoami") {
        return { jsonrpc: "2.0", id, result: { content: [{ type: "text", text: `device:${host}` }] } };
      }
      if (name === "echo") {
        return {
          jsonrpc: "2.0",
          id,
          result: { content: [{ type: "text", text: `device:${host} echo:${args.text}` }] },
        };
      }
      return { jsonrpc: "2.0", id, error: { code: -32602, message: `Unknown tool: ${name}` } };
    }
    default:
      return { jsonrpc: "2.0", id, error: { code: -32601, message: `Method not found: ${message.method}` } };
  }
}

http
  .createServer((request, response) => {
    const path = request.url.split("?")[0];
    if (path !== "/mcp" || request.method !== "POST") {
      // A server that has no stream to offer says so, as the spec allows.
      response.writeHead(path === "/mcp" ? 405 : 404, { Allow: "POST" }).end();
      return;
    }
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      let message;
      try {
        message = JSON.parse(body);
      } catch {
        response.writeHead(400).end();
        return;
      }
      console.log(`${request.socket.remoteAddress} ${message.method}`);
      if (message.id === undefined) {
        response.writeHead(202).end();
        return;
      }
      response
        .writeHead(200, { "Content-Type": "application/json" })
        .end(JSON.stringify(reply(message, message.id)));
    });
  })
  .listen(port, "0.0.0.0", () => console.log(`MCP device listening on ${port}`));
