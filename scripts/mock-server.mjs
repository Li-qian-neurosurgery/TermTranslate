// Local-only deterministic integration fixture. This is not a translation service.
import http from 'node:http';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
const port = Number(process.env.TERMTRANSLATE_MOCK_PORT || 43180);
if (process.argv.includes('--write-fixture')) {
  const root = resolve(process.env.TERMTRANSLATE_DATA_DIR || '.artifacts/ui-smoke');
  await mkdir(root, { recursive: true });
  const profiles = [['responses', 'OpenAI'], ['chat_completions', '兼容接口'], ['anthropic', 'Claude'], ['gemini', 'Gemini']].map(([protocol, name]) => ({
    id: `local-${protocol.replaceAll('_', '-')}`, name: `本地模拟 / ${name}`, provider: 'custom', protocol,
    baseUrl: `http://127.0.0.1:${port}/v1`, model: 'mock-translator', timeoutSeconds: 30, hasKey: false,
  }));
  await writeFile(resolve(root, 'settings.json'), JSON.stringify({ schemaVersion: 1, profiles, activeProfileId: profiles[0].id, activePromptId: 'general', targetLanguage: 'auto', shortcut: 'Control+Alt+Shift+F12', recordHistory: true }, null, 2));
  console.log('Fixture directory:', root);
}
const server = http.createServer(async (request, response) => {
  if (request.method !== 'POST') { response.writeHead(404).end(); return; }
  let bytes = '';
  for await (const chunk of request) bytes += chunk;
  let body;
  try { body = JSON.parse(bytes); } catch { response.writeHead(400).end(); return; }
  if (body.model === 'simulate-error') { response.writeHead(429, { 'Content-Type': 'application/json' }).end('{"error":{"message":"mock rate limit"}}'); return; }
  response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
  const source = typeof body.input === 'string' ? body.input : JSON.stringify(body.input || body.messages || body.contents || '');
  const text = source.includes('Translation should feel like a conversation.')
    ? '翻译应该像对话一样自然。\n粘贴一段文字，读完译文，然后继续。'
    : source.includes('保留段落、数字和代码。')
      ? 'Preserve paragraphs, numbers, and code.\n\n`const message = "Hello";`'
      : '翻译让思想更近一步。\n\n保留段落、术语与代码，让表达自然发生。\n\n`const greeting = "Hello";`';
  const send = value => response.write(`data: ${typeof value === 'string' ? value : JSON.stringify(value)}\n\n`);
  for (const char of text) {
    if (response.destroyed) return;
    if (request.url.endsWith('/responses')) send({ type: 'response.output_text.delta', delta: char });
    else if (request.url.endsWith('/messages')) send({ type: 'content_block_delta', delta: { type: 'text_delta', text: char } });
    else if (request.url.includes(':streamGenerateContent')) send({ candidates: [{ content: { parts: [{ text: char }] } }] });
    else send({ choices: [{ delta: { content: char } }] });
    await new Promise(resolve => setTimeout(resolve, body.model === 'simulate-slow' ? 150 : 18));
  }
  if (body.model !== 'simulate-disconnect') {
    if (request.url.endsWith('/responses')) send({ type: 'response.completed' });
    else if (request.url.endsWith('/messages')) send({ type: 'message_stop' });
    else if (request.url.includes(':streamGenerateContent')) send({ candidates: [{ finishReason: 'STOP' }] });
    else send('[DONE]');
  }
  response.end();
});
server.listen(port, '127.0.0.1', () => console.log(`Local SSE fixture: http://127.0.0.1:${port}`));
