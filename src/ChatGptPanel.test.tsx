import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Props } from './Panels';
import type { ChatGptAccount, Snapshot } from './types';
const mock = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mock.invoke }));
import { ChatGptPanel } from './ChatGptPanel';
const account = (id: string): ChatGptAccount => ({ id, email: `${id}@example.invalid`, clientId: `client-${id}`, subject: id, connected: true, planEnabled: true, usageAcknowledged: true });
const snapshot: Snapshot = { platform: 'windows', warnings: [], history: [], prompts: [], chatgptAccounts: [], settings: { schemaVersion: 1, profiles: [], activeProfileId: '', activePromptId: 'general', targetLanguage: 'auto', shortcut: 'Control+Alt+T', recordHistory: true } };
function props(accounts: ChatGptAccount[] = []): Props { return { panel: 'models', setPanel: vi.fn(), snapshot: { ...snapshot, chatgptAccounts: accounts }, reload: vi.fn().mockResolvedValue(undefined), preferences: vi.fn(), notice: vi.fn(), busy: false, setTesting: vi.fn(), onCopy: vi.fn(), onRestore: vi.fn() }; }
beforeEach(() => {
  mock.invoke.mockReset();
  mock.invoke.mockImplementation(async (command, args) => command === 'chatgpt_models' ? [{ slug: `${args.accountId}-model`, displayName: `${args.accountId} model` }] : undefined);
  HTMLDialogElement.prototype.showModal = function() { this.setAttribute('open', ''); };
});
afterEach(cleanup);
it('starts official login without asking for an API key and supports cancellation', async () => {
  let rejectLogin: (error: string) => void = () => {};
  mock.invoke.mockImplementation((command) => {
    if (command === 'chatgpt_login') return new Promise((_resolve, reject) => { rejectLogin = reject; });
    if (command === 'cancel_chatgpt_login') rejectLogin('已取消 ChatGPT 登录');
    return Promise.resolve();
  });
  const p = props(); render(<ChatGptPanel {...p}/>);
  fireEvent.click(screen.getByRole('button', { name: '使用 ChatGPT 登录' }));
  await screen.findByText('等待你在浏览器中完成授权…');
  expect(mock.invoke).toHaveBeenCalledWith('chatgpt_login', { accountId: null, consent: false });
  expect(screen.queryByLabelText('API Key')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: '取消登录' }));
  await screen.findByText('已取消 ChatGPT 登录');
  expect(p.setTesting).toHaveBeenLastCalledWith(false);
});
it('loads and selects models separately for each account and links to usage', async () => {
  const p = props([account('first'), account('second')]); render(<ChatGptPanel {...p}/>);
  await screen.findByRole('option', { name: 'first model / first-model' });
  fireEvent.change(screen.getByLabelText('ChatGPT 账户'), { target: { value: 'second' } });
  await screen.findByRole('option', { name: 'second model / second-model' });
  expect(screen.queryByRole('option', { name: 'first model / first-model' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: '使用此模型' }));
  await waitFor(() => expect(mock.invoke).toHaveBeenCalledWith('select_chatgpt_model', { accountId: 'second', model: 'second-model' }));
  fireEvent.click(screen.getByRole('button', { name: '管理订阅额度' }));
  expect(mock.invoke).toHaveBeenCalledWith('open_chatgpt_usage');
});
it('does not allow inference for an identity-only grant', async () => {
  render(<ChatGptPanel {...props([{ ...account('limited'), planEnabled: false }])}/>);
  expect(screen.queryByLabelText('ChatGPT 模型')).toBeNull();
  expect(mock.invoke.mock.calls.some(([command]) => command === 'chatgpt_models')).toBe(false);
  expect(screen.getByRole('button', { name: '启用订阅额度' })).toBeDefined();
});
it('shows the first-use plan notice and persists acknowledgement', async () => {
  render(<ChatGptPanel {...props([{ ...account('first'), usageAcknowledged: false }])}/>);
  await screen.findByRole('dialog', { name: 'ChatGPT 套餐使用说明' });
  fireEvent.click(screen.getByRole('button', { name: '我知道了' }));
  await waitFor(() => expect(mock.invoke).toHaveBeenCalledWith('acknowledge_chatgpt_usage', { accountId: 'first' }));
});
