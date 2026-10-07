const readline = require('node:readline');

const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  const request = JSON.parse(line);
  let result;
  if (request.method === 'initialize') {
    result = {
      protocolVersion: '2024-11-05',
      capabilities: { tools: {} },
      serverInfo: { name: 'angelbot-e2e', version: '1.0.0' },
    };
  } else if (request.method === 'tools/list') {
    result = {
      tools: [{
        name: 'angelbot_e2e_ping',
        description: 'Deterministic local MCP discovery fixture',
        inputSchema: { type: 'object', properties: {} },
      }],
    };
  } else if (request.method === 'tools/call') {
    const accepted = request.params?.name === 'angelbot_e2e_ping'
      && process.env.ANGELBOT_E2E_TOKEN === 'angelbot-e2e-mcp-secret-canary';
    result = {
      content: [{
        type: 'text',
        text: accepted ? 'angelbot-e2e-mcp-call-ok' : 'angelbot-e2e-mcp-call-rejected',
      }],
      isError: !accepted,
    };
  } else {
    return;
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: request.id, result })}\n`);
});
