export type Protocol = 'responses' | 'chat_completions' | 'anthropic' | 'gemini';
export interface Profile { id: string; name: string; provider: string; protocol: Protocol; baseUrl: string; model: string; timeoutSeconds: number; hasKey: boolean }
export interface Prompt { id: string; name: string; content: string; builtin: boolean }
export interface Settings { schemaVersion: number; profiles: Profile[]; activeProfileId: string; activePromptId: string; targetLanguage: string; shortcut: string; recordHistory: boolean }
export interface HistoryEntry { id: string; createdAt: string; source: string; translation: string; profileName: string; model: string; promptName: string; targetLanguage: string }
export interface ChatGptAccount { id: string; clientId: string; subject: string | null; email: string | null; connected: boolean; planEnabled: boolean; usageAcknowledged: boolean }
export interface ChatGptModel { slug: string; displayName: string }
export interface Snapshot { settings: Settings; prompts: Prompt[]; history: HistoryEntry[]; warnings: string[]; platform: string; chatgptAccounts?: ChatGptAccount[] }
export interface TranslationEvent { requestId: string; kind: 'started' | 'delta' | 'completed' | 'cancelled' | 'error'; text: string; message: string }
export type Panel = 'models' | 'prompts' | 'history' | 'preferences' | null;
export const LANGUAGES = [['auto', '中英自动互译'], ['zh-CN', '简体中文'], ['zh-TW', '繁體中文'], ['en', 'English'], ['ja', '日本語'], ['ko', '한국어'], ['fr', 'Français'], ['de', 'Deutsch'], ['es', 'Español']] as const;
export const PROTOCOLS: [Protocol, string][] = [['responses', 'OpenAI Responses'], ['chat_completions', 'OpenAI 兼容 Chat Completions'], ['anthropic', 'Anthropic Messages'], ['gemini', 'Google Gemini']];
export const PRESETS: { id: string; name: string; protocol: Protocol; baseUrl: string; hint: string }[] = [
  { id: 'openai', name: 'OpenAI', protocol: 'responses', baseUrl: 'https://api.openai.com/v1', hint: '填写账户中可用的模型 ID' },
  { id: 'anthropic', name: 'Anthropic / Claude', protocol: 'anthropic', baseUrl: 'https://api.anthropic.com/v1', hint: '填写 Claude 模型 ID' },
  { id: 'gemini', name: 'Google / Gemini', protocol: 'gemini', baseUrl: 'https://generativelanguage.googleapis.com/v1beta', hint: '填写 Gemini 模型 ID' },
  { id: 'deepseek', name: 'DeepSeek', protocol: 'chat_completions', baseUrl: 'https://api.deepseek.com/v1', hint: '填写 DeepSeek 控制台中的模型 ID' },
  { id: 'qwen', name: '通义千问 / 百炼', protocol: 'chat_completions', baseUrl: 'https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1', hint: '将 Base URL 中的 WorkspaceId 替换为业务空间 ID，并核对地区' },
  { id: 'doubao', name: '豆包 / 火山方舟', protocol: 'chat_completions', baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', hint: '填写模型 ID 或已创建的推理接入点 ID（ep-…）' },
  { id: 'kimi', name: 'Kimi / 月之暗面', protocol: 'chat_completions', baseUrl: 'https://api.moonshot.cn/v1', hint: '国内账户预设；国际账户请改为控制台提供的地址' },
  { id: 'zhipu', name: '智谱 / GLM', protocol: 'chat_completions', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', hint: '填写智谱开放平台中的模型 ID' },
  { id: 'custom', name: '自定义服务', protocol: 'chat_completions', baseUrl: '', hint: '支持自建兼容服务；无需鉴权的本地服务可留空 API Key' },
];

export function newProfile(presetId = 'openai'): Profile {
  const preset = PRESETS.find(p => p.id === presetId) ?? PRESETS[0];
  return { id: crypto.randomUUID(), name: preset.name, provider: preset.id, protocol: preset.protocol, baseUrl: preset.baseUrl, model: '', timeoutSeconds: 120, hasKey: false };
}
export function shouldTranslate(event: { key: string; ctrlKey: boolean; metaKey: boolean; shiftKey?: boolean; altKey?: boolean; isComposing?: boolean; keyCode?: number }): boolean {
  return event.key === 'Enter' && !event.shiftKey && !event.altKey && !event.isComposing && event.keyCode !== 229;
}
export interface TranslationState { requestId: string; output: string; status: 'idle' | 'running' | 'completed' | 'cancelled' | 'error'; message: string }
export const emptyTranslation: TranslationState = { requestId: '', output: '', status: 'idle', message: '' };
export function receiveEvent(state: TranslationState, event: TranslationEvent): TranslationState {
  if (state.requestId !== event.requestId || state.status !== 'running') return state;
  if (event.kind === 'started') return state;
  if (event.kind === 'delta') return { ...state, output: state.output + event.text };
  return { ...state, output: event.kind === 'completed' ? event.text : state.output, status: event.kind, message: event.message };
}
