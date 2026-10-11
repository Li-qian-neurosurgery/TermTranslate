import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Snapshot, TranslationEvent } from './types';
const mock = vi.hoisted(() => ({
  invoke: vi.fn(), readText: vi.fn(), writeText: vi.fn(),
  channels: [] as { onmessage: (event: TranslationEvent) => void }[],
  requests: [] as { requestId: string; text: string; profileId: string; targetLanguage: string }[],
}));
vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => true, invoke: mock.invoke,
  Channel: class { onmessage = (_event: TranslationEvent) => {}; constructor() { mock.channels.push(this); } },
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ readText: mock.readText, writeText: mock.writeText }));
import App from './App';
const initial: Snapshot = { platform: 'windows', warnings: [], history: [], prompts: [{ id: 'general', name: '通用翻译', content: '{{target_language}}', builtin: true }], settings: { schemaVersion: 1, profiles: [{ id: 'one', name: 'Test', provider: 'custom', protocol: 'responses', baseUrl: 'http://127.0.0.1:1', model: 'test', timeoutSeconds: 30, hasKey: false }], activeProfileId: 'one', activePromptId: 'general', targetLanguage: 'auto', shortcut: 'Control+Alt+T', recordHistory: true } };
let snapshot: Snapshot;
beforeEach(() => {
  snapshot = structuredClone(initial);
  mock.invoke.mockReset(); mock.readText.mockReset(); mock.writeText.mockReset();
  mock.channels.length = 0; mock.requests.length = 0;
  HTMLDialogElement.prototype.showModal = function() { this.setAttribute('open', ''); };
  mock.invoke.mockImplementation(async (command, args) => {
    if (command === 'get_snapshot') return structuredClone(snapshot);
    if (command === 'start_translation') mock.requests.push(args.request);
    if (command === 'cancel_translation') {
      const i = mock.requests.findIndex(request => request.requestId === args.requestId);
      mock.channels[i]?.onmessage({ requestId: args.requestId, kind: 'cancelled', text: '', message: '已停止生成' });
    }
    if (command === 'set_preferences') snapshot.settings.targetLanguage = args.targetLanguage;
  });
});
afterEach(cleanup);
async function send(text: string) {
  const input = await screen.findByLabelText('待翻译原文');
  fireEvent.change(input, { target: { value: text } });
  fireEvent.keyDown(input, { key: 'Enter' });
}
async function event(index: number, kind: TranslationEvent['kind'], text = '', message = '') {
  await act(async () => { mock.channels[index].onmessage({ requestId: mock.requests[index].requestId, kind, text, message }); });
}

it('pastes multiline text, clears the sent draft, streams and copies plain text', async () => {
  mock.readText.mockResolvedValue('English\n\n`code`');
  render(<App/>);
  const input = await screen.findByLabelText('待翻译原文') as HTMLTextAreaElement;
  fireEvent.click(screen.getByRole('button', { name: '粘贴' }));
  await waitFor(() => expect(input.value).toBe('English\n\n`code`'));
  fireEvent.click(screen.getByRole('button', { name: '发送' }));
  expect(input.value).toBe('');
  expect(screen.getByLabelText('原文').textContent).toBe('English\n\n`code`');
  await event(0, 'delta', '中文\n\n`code`');
  await event(0, 'completed', '中文\n\n`code`');
  expect(screen.getByLabelText('翻译结果').textContent).toBe('中文\n\n`code`');
  fireEvent.click(screen.getByRole('button', { name: '复制译文' }));
  await waitFor(() => expect(mock.writeText).toHaveBeenCalledWith('中文\n\n`code`'));
});

it('does not send on Shift+Enter or Chinese candidate confirmation', async () => {
  render(<App/>);
  const input = await screen.findByLabelText('待翻译原文');
  fireEvent.change(input, { target: { value: '你好\n世界' } });
  fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
  fireEvent.keyDown(input, { key: 'Enter', isComposing: true });
  fireEvent.keyDown(input, { key: 'Enter', keyCode: 229 });
  fireEvent.compositionStart(input);
  fireEvent.keyDown(input, { key: 'Enter' });
  expect(mock.requests).toHaveLength(0);
  fireEvent.compositionEnd(input);
  fireEvent.keyDown(input, { key: 'Enter' });
  expect(mock.requests).toHaveLength(1);
  expect(mock.requests[0].text).toBe('你好\n世界');
});

it('keeps consecutive question/answer pairs and copies the selected reply', async () => {
  render(<App/>);
  await send('First source'); await event(0, 'completed', '第一段译文');
  await send('第二段原文'); await event(1, 'completed', 'Second translation');
  const turns = screen.getAllByRole('article');
  expect(turns).toHaveLength(2);
  expect(within(turns[0]).getByLabelText('原文').textContent).toBe('First source');
  expect(within(turns[0]).getByLabelText('翻译结果').textContent).toBe('第一段译文');
  expect(within(turns[1]).getByLabelText('翻译结果').textContent).toBe('Second translation');
  fireEvent.click(within(turns[0]).getByRole('button', { name: '复制译文' }));
  await waitFor(() => expect(mock.writeText).toHaveBeenCalledWith('第一段译文'));
  // Each translation sends only its own original text, not the transcript.
  expect(mock.requests[1].text).toBe('第二段原文');
});

it('keeps an interrupted answer and ignores its late events after sending a new turn', async () => {
  render(<App/>);
  await send('Old source'); await event(0, 'delta', '部分旧译文');
  await send('New source');
  await event(0, 'delta', '迟到内容'); await event(0, 'completed', '过期完整内容');
  await event(1, 'completed', '新的完整译文');
  const turns = screen.getAllByRole('article');
  expect(within(turns[0]).getByLabelText('翻译结果').textContent).toBe('部分旧译文');
  expect(within(turns[0]).getByText('已停止，开始下一条翻译。')).toBeDefined();
  expect(within(turns[1]).getByLabelText('翻译结果').textContent).toBe('新的完整译文');
  expect(screen.queryByText('迟到内容')).toBeNull();
});

it('retries a cancelled source in a new turn without discarding the next draft or partial answer', async () => {
  render(<App/>);
  await send('Hello'); await event(0, 'delta', '部分译文');
  fireEvent.click(screen.getByRole('button', { name: '停止生成' }));
  await screen.findByRole('button', { name: '重新翻译' });
  const input = screen.getByLabelText('待翻译原文') as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: '尚未发送的下一段' } });
  fireEvent.change(screen.getByLabelText('目标语言'), { target: { value: 'ja' } });
  await waitFor(() => expect((screen.getByLabelText('目标语言') as HTMLSelectElement).value).toBe('ja'));
  fireEvent.click(screen.getByRole('button', { name: '重新翻译' }));
  expect(mock.requests[1].text).toBe('Hello');
  expect(mock.requests[1].targetLanguage).toBe('ja');
  expect(input.value).toBe('尚未发送的下一段');
  await event(1, 'completed', 'こんにちは');
  expect(screen.getAllByLabelText('翻译结果').map(node => node.textContent)).toEqual(['部分译文', 'こんにちは']);
});

it('starts a new conversation, cancels work and rejects stale results without deleting saved history', async () => {
  render(<App/>);
  await send('Old source'); await event(0, 'delta', 'Old partial');
  fireEvent.click(screen.getByRole('button', { name: '新对话' }));
  await event(0, 'completed', 'Late final');
  expect(screen.queryAllByRole('article')).toHaveLength(0);
  expect(mock.invoke).toHaveBeenCalledWith('cancel_translation', { requestId: mock.requests[0].requestId });
  expect(mock.invoke.mock.calls.some(([command]) => command === 'delete_history')).toBe(false);
  await send('Fresh source'); await event(1, 'completed', 'Fresh result');
  expect(screen.getAllByRole('article')).toHaveLength(1);
  expect(screen.queryByText('Late final')).toBeNull();
});

it('preserves failed output and offers retry for an error response', async () => {
  render(<App/>);
  await send('Long source'); await event(0, 'delta', '未完成部分');
  await event(0, 'error', '', '连接中断，请重试');
  await event(0, 'delta', '无效的后续输出');
  expect(screen.getByLabelText('翻译结果').textContent).toBe('未完成部分');
  expect(screen.getByText('连接中断，请重试')).toBeDefined();
  expect(screen.getByRole('button', { name: '重新翻译' })).toBeDefined();
});

it('keeps the draft and opens subscription configuration when no model is selected', async () => {
  snapshot.settings.profiles = []; snapshot.settings.activeProfileId = '';
  render(<App/>);
  await send('Do not lose this draft');
  await screen.findByRole('button', { name: '使用 ChatGPT 登录' });
  expect((screen.getByLabelText('待翻译原文') as HTMLTextAreaElement).value).toBe('Do not lose this draft');
  expect(mock.requests).toHaveLength(0);
});

it('adds saved history to the conversation without making a model request', async () => {
  snapshot.history = [{ id: 'saved', source: 'Saved source', translation: '保存的译文', profileName: 'Test', model: 'test', promptName: '通用翻译', targetLanguage: 'auto', createdAt: '2026-10-11T12:00:00Z' }];
  render(<App/>);
  await send('Current source'); await event(0, 'completed', '当前译文');
  fireEvent.click(screen.getByRole('button', { name: '历史' }));
  fireEvent.click(await screen.findByRole('button', { name: /Saved source/ }));
  fireEvent.click(screen.getByRole('button', { name: '加入当前对话' }));
  expect(screen.getAllByRole('article')).toHaveLength(2);
  expect(screen.getAllByLabelText('翻译结果').map(node => node.textContent)).toEqual(['当前译文', '保存的译文']);
  expect(mock.requests).toHaveLength(1);
});
