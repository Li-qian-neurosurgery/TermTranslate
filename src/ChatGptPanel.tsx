import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowUpRight, Check, ExternalLink, LoaderCircle, LogOut, RefreshCw } from 'lucide-react';
import type { Props } from './Panels';
import type { ChatGptAccount, ChatGptModel } from './types';

export function ChatGptPanel({ snapshot, reload, notice, busy, setTesting }: Props) {
  const accounts = snapshot.chatgptAccounts ?? [];
  const [accountId, setAccountId] = useState(() => accounts.find(a => a.id === snapshot.settings.activeProfileId)?.id ?? accounts[0]?.id ?? '');
  const [models, setModels] = useState<ChatGptModel[]>([]);
  const [model, setModel] = useState('');
  const [working, setWorking] = useState('');
  const [message, setMessage] = useState('');
  const [welcome, setWelcome] = useState<string | null>(null);
  const [confirmLogout, setConfirmLogout] = useState(false);
  const active = useRef(true);
  const generation = useRef(0);
  const signingIn = useRef(false);
  const account = accounts.find(a => a.id === accountId);
  useEffect(() => { active.current = true; return () => { active.current = false; generation.current++; if (signingIn.current) void invoke('cancel_chatgpt_login'); }; }, []);
  useEffect(() => { if (account?.connected && account.planEnabled && !account.usageAcknowledged) setWelcome(account.id); }, [account?.id, account?.connected, account?.planEnabled, account?.usageAcknowledged]);

  useEffect(() => {
    const current = ++generation.current;
    setModels([]); setModel(''); setConfirmLogout(false);
    if (!account?.connected || !account.planEnabled) return;
    setWorking('models'); setMessage('正在加载账户可用模型…');
    void invoke<ChatGptModel[]>('chatgpt_models', { accountId: account.id }).then(rows => {
      if (!active.current || current !== generation.current) return;
      const savedModel = snapshot.settings.profiles.find(p => p.id === account.id)?.model;
      setModels(rows); setModel(rows.some(m => m.slug === savedModel) ? savedModel! : rows[0]?.slug ?? ''); setMessage('');
    }).catch(error => { if (active.current && current === generation.current) setMessage(String(error)); })
      .finally(() => { if (active.current && current === generation.current) setWorking(''); });
  }, [accountId, account?.connected, account?.planEnabled]);

  const run = async (kind: string, action: () => Promise<void>) => {
    setWorking(kind); setMessage(''); setTesting(true);
    try { await action(); } catch (error) { if (active.current) setMessage(String(error)); }
    finally { if (active.current) setWorking(''); setTesting(false); }
  };
  const login = (id: string | null, consent = false) => void run('login', async () => {
    signingIn.current = true;
    try {
      const result = await invoke<{ accountId: string; planEnabled: boolean }>('chatgpt_login', { accountId: id, consent });
      if (!active.current) return;
      await reload(); setAccountId(result.accountId);
      // Reauthorization of a connected account must also invalidate its previous model list.
      setModels([]);
      if (result.planEnabled) {
        const rows = await invoke<ChatGptModel[]>('chatgpt_models', { accountId: result.accountId });
        if (active.current) { setModels(rows); setModel(rows[0]?.slug ?? ''); setMessage('已登录，请选择模型并点击「使用此模型」。'); }
      } else setMessage('已登录，但尚未授权套餐额度。点击「启用订阅额度」可再次授权。');
    } finally { signingIn.current = false; await reload(); }
  });
  const refresh = () => void run('models', async () => {
    const rows = await invoke<ChatGptModel[]>('chatgpt_models', { accountId });
    if (active.current) { setModels(rows); setModel(previous => rows.some(m => m.slug === previous) ? previous : rows[0]?.slug ?? ''); }
  });
  const label = (a: ChatGptAccount) => `${a.email ?? 'ChatGPT 账户'} · ${a.id.slice(0, 8)}`;

  return <><div className="section-title"><span className="eyebrow">CHATGPT / SUBSCRIPTION</span><h2>用你的 ChatGPT 套餐翻译</h2><p>在浏览器中登录并授权，使用账户可用的模型与额度。</p></div>
    <div className="chatgpt-card"><div className="chatgpt-wordmark">ChatGPT<span>订阅连接</span></div><p>翻译会使用你的 ChatGPT 套餐或允许使用的点数，受账户和应用额度限制。可随时在 ChatGPT 中管理。</p>
      <button className="chatgpt-signin" disabled={!!working || busy} onClick={() => login(null)}>使用 ChatGPT 登录 <ArrowUpRight size={16}/></button>
      <span className="field-hint signin-hint">Continue with ChatGPT · 登录与授权在系统浏览器中完成</span>
    </div>
    {!!accounts.length && <label>已保存的 ChatGPT 账户<select aria-label="ChatGPT 账户" value={accountId} disabled={!!working || busy} onChange={event => { setAccountId(event.target.value); setMessage(''); }}>{accounts.map(a => <option key={a.id} value={a.id}>{label(a)}{a.connected ? a.planEnabled ? ' · 已连接' : ' · 未授权额度' : ' · 已退出'}</option>)}</select></label>}
    {account && <div className="account-status"><span className={`status-dot ${account.connected && account.planEnabled ? 'ready' : ''}`}/><span>{account.connected ? account.planEnabled ? '已授权使用 ChatGPT 套餐' : '已登录 · 尚未启用订阅额度' : '此账户已退出，注册信息已保留'}</span><button className="text-button" disabled={!!working || busy} onClick={() => login(account.id, account.connected && !account.planEnabled)}>{account.connected && !account.planEnabled ? '启用订阅额度' : '重新登录'}</button></div>}
    {account?.connected && account.planEnabled && <>
      <label>账户可用模型<select aria-label="ChatGPT 模型" value={model} disabled={!!working || busy || !models.length} onChange={e => setModel(e.target.value)}>{!models.length && <option value="">请加载可用模型</option>}{models.map(m => <option key={m.slug} value={m.slug}>{m.displayName} / {m.slug}</option>)}</select></label>
      <div className="form-actions"><button disabled={!!working || busy} onClick={refresh}><RefreshCw size={13}/>刷新模型</button><span className="spacer"/><button className="primary" disabled={!!working || busy || !models.some(m => m.slug === model)} onClick={() => void run('select', async () => { await invoke('select_chatgpt_model', { accountId, model }); await reload(); if (active.current) setMessage('已切换到 ChatGPT 订阅模型，可以开始翻译。'); })}><Check size={14}/>使用此模型</button></div>
    </>}
    {working === 'login' && <div className="login-wait"><LoaderCircle className="spin" size={16}/><span>等待你在浏览器中完成授权…</span><button className="text-button" onClick={() => void invoke('cancel_chatgpt_login').catch(error => setMessage(String(error)))}>取消登录</button></div>}
    {working && working !== 'login' && <div className="login-wait"><LoaderCircle className="spin" size={14}/><span>{working === 'logout' ? '正在退出并撤销会话…' : '正在处理…'}</span></div>}
    {message && <div role="status" className="inline-feedback">{message}</div>}
    <div className="subscription-links"><button className="text-button" onClick={() => void invoke('open_chatgpt_usage').catch(e => notice(String(e)))}><ExternalLink size={13}/>管理订阅额度</button>{account?.connected && <button className="text-button danger" disabled={!!working || busy} onClick={() => { if (!confirmLogout) { setConfirmLogout(true); return; } void run('logout', async () => { const result = await invoke<string>('chatgpt_logout', { accountId }); await reload(); if (active.current) { setModels([]); setMessage(result); setConfirmLogout(false); } }); }}><LogOut size={13}/>{confirmLogout ? '确认退出此账户' : '退出此账户'}</button>}</div>
    <p className="field-hint subscription-note">登录凭据由系统凭据库保存。授权不包含你的 ChatGPT 对话记录。订阅额度不可用时会显示原因，由你决定后续操作。</p>
    {welcome && <UsageWelcome onClose={() => void invoke('acknowledge_chatgpt_usage', { accountId: welcome }).then(reload).then(() => setWelcome(null)).catch(e => setMessage(String(e)))}/>}
  </>;
}

function UsageWelcome({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { dialog.current?.showModal(); }, []);
  return <dialog ref={dialog} className="usage-welcome" aria-label="ChatGPT 套餐使用说明" onCancel={event => { event.preventDefault(); onClose(); }}><span className="eyebrow">CHATGPT</span><h2>正在使用你的 ChatGPT 套餐</h2><p>本应用的翻译请求会使用你授权的套餐额度或点数。你可以在 ChatGPT 设置中查看使用量、调整此应用的上限或断开连接。</p><button className="primary" autoFocus onClick={onClose}>我知道了</button></dialog>;
}
