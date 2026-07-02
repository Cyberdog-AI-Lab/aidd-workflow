import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { parseArgs } from "util";

const { values } = parseArgs({
	args: Bun.argv.slice(2),
	options: {
		port: { type: "string", default: "8788" },
		hostname: { type: "string", default: "127.0.0.1" },
	},
});
const port = Number(values.port);
const hostname = values.hostname!;

// Create the MCP server and declare it as a channel
const mcp = new McpServer(
	{ name: "webhook", version: "0.0.1" },
	{
		// this key is what makes it a channel — Claude Code registers a listener for it
		capabilities: { experimental: { "claude/channel": {} } },
		// added to Claude's system prompt so it knows how to handle these events
		instructions: `\
Events from the webhook channel arrive as <channel source="webhook" ...>.
They are one-way: read them and act, no reply to the channel is needed.

## Task instruction format

When a task instruction arrives, the channel body is a JSON object:

\`\`\`json
{
  "task_id":     "implement",
  "task":        "Implement the feature",
  "prompt":      "Follow the design doc and implement …",
  "skills":      [],
  "agents":      [],
  "callback_url": "http://127.0.0.1:8789",
  "workflow_id": "4fd261ba-…",
  "outputs":     ["src/**", "tests/**"],
  "deny":        { "files": [".env"] }
}
\`\`\`

\`skills\` and \`agents\` are mutually exclusive with each other's presence:
- If \`agents\` is a non-empty list (and \`prompt\` is null), spawn each named
  custom agent (\`.claude/agents/<name>.md\`) in parallel via the Agent tool.
  Each agent reports its own completion by calling /complete on
  \`{task_id}/{agent_name}\` (see step 4). Only call /complete on the bare
  \`task_id\` once every agent has finished.
- If \`skills\` is a non-empty list, invoke each named skill via the Skill tool
  (in addition to \`prompt\`, if also present).

## How to handle a task instruction

1. Parse the JSON from the channel body.
2. Read \`prompt\` (and/or \`skills\`/\`agents\`, see above) and carry out the requested work (edit files, run tests, etc.).
3. Respect \`outputs\` (only modify matching paths) and \`deny\` (never touch listed files/commands).
4. When the work is fully complete, call /complete with a brief summary:

\`\`\`bash
curl -sX POST {callback_url}/complete/{task_id} \\
  -H 'Content-Type: application/json' \\
  -d '{"summary":"implemented X, all tests passed"}'
\`\`\`

5. Optionally report progress mid-task with a brief summary:

\`\`\`bash
curl -sX POST {callback_url}/report/{task_id} \\
  -H 'Content-Type: application/json' \\
  -d '{"summary":"ran tests, 12 passed"}'
\`\`\`

Send /report when you finish a significant step (e.g. tests pass, a phase of work completes).
Do NOT use /report to signal start or end of the task — use /complete for the latter.

6. If you must pause to ask the user a question before continuing, notify the orchestrator:

\`\`\`bash
curl -sX POST {callback_url}/pause/{task_id} \\
  -H 'Content-Type: application/json' \\
  -d '{"reason":"need clarification on which file to modify"}'
\`\`\`

Only call /pause when you genuinely cannot proceed without human input.
After the user answers in your session, call /complete when the task is done.

Do NOT call /complete until all work for that task is truly done.`,
	},
);

// Connect to Claude Code over stdio (Claude Code spawns this process)
await mcp.connect(new StdioServerTransport());

// Start an HTTP server that forwards every POST to Claude
Bun.serve({
	port,
	hostname,
	async fetch(req) {
		const body = await req.text();
		await mcp.server.notification({
			method: "notifications/claude/channel",
			params: {
				content: body, // becomes the body of the <channel> tag
				// each key becomes a tag attribute, e.g. <channel path="/" method="POST">
				meta: { path: new URL(req.url).pathname, method: req.method },
			},
		});
		return new Response("ok");
	},
});
