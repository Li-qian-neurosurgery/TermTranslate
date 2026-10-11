import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager';
import { ArrowDown, ArrowUpRight, Check, Clipboard, Copy, CornerDownLeft, LoaderCircle, RotateCcw, Square, X } from 'lucide-react';
import type { Panel, Settings, Snapshot } from './types';
import { LANGUAGES, shouldTranslate } from './types';
import { WorkspacePanel } from './Panels';
import { useConversation } from './useConversation';

export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [fatal, setFatal] = useState('');
  const [source, setSource] = useState('');
  const [panel, setPanel] = useState<Panel>(null);
  const [notice, setNotice] = useState('');
  const [copiedId, setCopiedId] = useState('');
  const [testing, setTesting] = useState(false);
  const [clock, setClock] = useState(Date.now());
  const [showJump, setShowJump] = useState(false);
  const input = useRef<HTMLTextAreaElement>(null);
  const transcript = useRef<HTMLDivElement>(null);
  const composing = useRef(false);
  const followOutput = useRef(true);
  const mounted = useRef(true);
  const settings = snapshot?.settings;
  const profile = settings?.profiles.find(p => p.id === settings.activeProfileId);

  const reload = useCallback(async () => {
    const value = await invoke<Snapshot>('get_snapshot');
    if (mounted.current) setSnapshot(value);
  }, []);
  const { turns, running, send, stop, clear, restore } = useConversation(reload, setNotice);
  const busy = running || testing;

  useEffect(() => {
    mounted.current = true;
    if (!isTauri()) { setFatal('请通过 TermTranslate 桌面程序打开。'); return; }
    void reload().catch(error => setFatal(String(error)));
    const focusListener = listen('focus-input', () => { setPanel(null); window.setTimeout(() => input.current?.focus(), 0); });
    const settingsListener = listen('open-settings', () => setPanel('models'));
    return () => { mounted.current = false; void focusListener.then(unlisten => unlisten()); void settingsListener.then(unlisten => unlisten()); };
  }, [reload]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(''), 4500);
    return () => clearTimeout(timer);
  }, [notice]);
  useEffect(() => {
    if (!copiedId) return;
    const timer = setTimeout(() => setCopiedId(''), 1800);
    return () => clearTimeout(timer);
  }, [copiedId]);
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setClock(Date.now()), 250);
    return () => clearInterval(timer);
  }, [running]);
  useLayoutEffect(() => {
    const element = input.current;
    if (element) {
      element.style.height = '0px';
      element.style.height = `${Math.min(Math.max(element.scrollHeight, 36), 180)}px`;
    }
    if (followOutput.current && transcript.current) transcript.current.scrollTop = transcript.current.scrollHeight;
  }, [source, turns, snapshot]);
  useEffect(() => {
    if (!transcript.current || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (followOutput.current && transcript.current) transcript.current.scrollTop = transcript.current.scrollHeight;
    });
    observer.observe(transcript.current);
    return () => observer.disconnect();
  }, [snapshot !== null]);

  const preferences = async (patch: Partial<Settings>) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    await invoke('set_preferences', { profileId: next.activeProfileId, promptId: next.activePromptId, targetLanguage: next.targetLanguage, shortcut: next.shortcut, recordHistory: next.recordHistory });
    await reload();
  };
  const scrollToLatest = () => {
    followOutput.current = true;
    setShowJump(false);
    if (transcript.current) transcript.current.scrollTop = transcript.current.scrollHeight;
  };
  const translate = (text = source, clearDraft = true) => {
    if (!text.trim() || testing || !settings) return;
    if (!profile) { setPanel('models'); return; }
    scrollToLatest();
    setClock(Date.now());
    void send(text, profile, settings, snapshot?.prompts.find(p => p.id === settings.activePromptId)?.name ?? '翻译');
    if (clearDraft) setSource('');
    input.current?.focus();
  };
  const copy = async (text: string, id = 'history') => {
    try { await writeText(text); setCopiedId(id); if (id === 'history') setNotice('译文已复制'); }
    catch { setNotice('复制失败，请手动选择译文复制。'); }
  };
  const paste = async () => {
    try {
      const text = await readText();
      if (!text) { setNotice('剪贴板中没有文字'); return; }
      const start = input.current?.selectionStart ?? source.length;
      const end = input.current?.selectionEnd ?? source.length;
      setSource(source.slice(0, start) + text + source.slice(end));
      requestAnimationFrame(() => { input.current?.focus(); input.current?.setSelectionRange(start + text.length, start + text.length); });
    } catch { setNotice('无法读取剪贴板，请使用键盘粘贴。'); }
  };
  const newConversation = () => {
    clear(); setSource(''); scrollToLatest(); input.current?.focus();
    setNotice('已开始新对话，已保存的翻译仍可在历史中查看。');
  };

  if (fatal) return <div className="fatal"><span className="terminal-logo">&gt;_</span><h1>TermTranslate</h1><p>{fatal}</p><button onClick={() => location.reload()}>重新加载</button></div>;
  if (!snapshot || !settings) return <div className="fatal"><LoaderCircle className="spin"/><p>正在打开翻译终端…</p></div>;

  return <main className="app-shell">
    <header className="app-header">
      <nav className="header-actions" aria-label="工作台操作">
        <button className="text-button" onClick={newConversation} disabled={testing || (!turns.length && !source)}>新对话</button>
        <button className="text-button" onClick={() => setPanel('history')}>历史</button>
        <button className="text-button" onClick={() => setPanel('models')}>设置</button>
      </nav>
    </header>

    {snapshot.warnings.length > 0 && <div className="warning-banner" role="status">{snapshot.warnings.join(' ')}</div>}

    <div className="transcript-wrap">
      <div className="transcript" ref={transcript} role="log" aria-label="翻译对话" aria-live="off" tabIndex={0} onScroll={event => {
        const element = event.currentTarget;
        followOutput.current = element.scrollHeight - element.scrollTop - element.clientHeight < 80;
        setShowJump(!followOutput.current);
      }}>
        {turns.map((turn, index) => <article className="conversation-turn" key={turn.id} aria-label={`第 ${index + 1} 轮翻译`}>
          <div className="message user-message">
            <span className="message-marker" aria-hidden="true">›</span>
            <div className="message-body"><pre className="message-text" aria-label="原文">{turn.source}</pre></div>
          </div>
          <div className={`message assistant-message ${turn.status}`}>
            <span className="message-marker" aria-hidden="true">•</span>
            <div className="message-body">
              {turn.output ? <pre className="message-text" aria-label="翻译结果">{turn.output}{turn.status === 'running' && <span className="cursor-block" aria-hidden="true"/>}</pre>
                : <p className="reply-placeholder">{turn.status === 'running' ? '正在翻译…' : turn.status === 'cancelled' ? '翻译已停止' : '这次翻译没有完成'}</p>}
              {turn.message && <p className={`result-message ${turn.status === 'error' ? 'error' : ''}`} role="status">{turn.message}</p>}
              <div className="turn-footer">
                <span className={`turn-status ${turn.status === 'error' ? 'error' : ''}`}>
                  {turn.status === 'running' && <span className="working-dot"/>}
                  {turn.restored ? '来自历史' : ({ idle: '', running: '翻译中', completed: '已完成', error: '未完成', cancelled: '已停止' })[turn.status]}
                  {!turn.restored && <> · {Math.max(0, (turn.status === 'running' ? clock - turn.startedAt : turn.elapsedMs) / 1000).toFixed(1)}s</>}
                  <span className="turn-model" title={`${turn.profileName} · ${turn.promptName}`}> · {turn.model}</span>
                </span>
                <div className="turn-actions">
                  {turn.output && <button className="text-button" aria-label="复制译文" onClick={() => void copy(turn.output, turn.id)}>{copiedId === turn.id ? <Check size={12}/> : <Copy size={12}/>} {copiedId === turn.id ? '已复制' : '复制'}</button>}
                  {turn.status !== 'running' && <button className="text-button" disabled={busy} onClick={() => translate(turn.source, false)} title="使用当前模型、语言和提示词重新翻译"><RotateCcw size={12}/>{turn.status === 'completed' ? '再译一次' : '重新翻译'}</button>}
                </div>
              </div>
            </div>
          </div>
        </article>)}
      </div>
      {showJump && <button className="jump-latest" onClick={scrollToLatest}><ArrowDown size={13}/>回到最新</button>}
    </div>

    <div className="composer-area">
      <form className={`composer ${running ? 'is-running' : ''}`} onSubmit={event => { event.preventDefault(); translate(); }}>
        <div className="composer-line"><span className="composer-mark" aria-hidden="true">›</span>
          <textarea ref={input} autoFocus spellCheck={false} rows={1} aria-label="待翻译原文" placeholder="输入或粘贴文字，按 Enter 翻译…" value={source} onChange={event => setSource(event.target.value)}
            onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
            onKeyDown={event => {
              if (shouldTranslate({ key: event.key, ctrlKey: event.ctrlKey, metaKey: event.metaKey, shiftKey: event.shiftKey, altKey: event.altKey, isComposing: composing.current || event.nativeEvent.isComposing, keyCode: event.nativeEvent.keyCode })) {
                event.preventDefault(); translate();
              }
            }}/>
        </div>
        <div className="composer-actions">
          <button type="button" className="text-button" onClick={() => void paste()}><Clipboard size={12}/>粘贴</button>
          {source.length > 0 && <span className="character-count">{Array.from(source).length.toLocaleString()} 字</span>}
          <span className="spacer"/>
          {running && <button type="button" className="stop-button" onClick={() => void stop()}><Square size={11}/>停止生成</button>}
          <button type="submit" className="send-button" disabled={!source.trim() || testing} title={running ? '停止当前翻译并发送下一段' : 'Enter 发送'}>发送<CornerDownLeft size={14}/></button>
        </div>
      </form>
      <footer className="terminal-footer">
        <div className="session-context">
          {profile ? <select aria-label="模型配置" title={`${profile.name} / ${profile.model}`} value={settings.activeProfileId} disabled={busy} onChange={event => void preferences({ activeProfileId: event.target.value }).catch(e => setNotice(String(e)))}>{settings.profiles.map(p => <option key={p.id} value={p.id}>{p.model} · {p.name}</option>)}</select>
            : <button className="text-button connect-link" onClick={() => setPanel('models')}>连接模型 <ArrowUpRight size={12}/></button>}
          {profile?.provider === 'chatgpt' && <button className="text-button subscription-label" onClick={() => void invoke('open_chatgpt_usage').catch(e => setNotice(String(e)))}>ChatGPT 套餐</button>}
          <span className="context-separator">·</span>
          <select aria-label="目标语言" title="目标语言" value={settings.targetLanguage} disabled={busy} onChange={event => void preferences({ targetLanguage: event.target.value }).catch(e => setNotice(String(e)))}>{LANGUAGES.map(([value, name]) => <option key={value} value={value}>{name}</option>)}</select>
          <span className="context-separator">·</span>
          <select aria-label="提示词模板" title="提示词模板" disabled={busy} value={settings.activePromptId} onChange={event => void preferences({ activePromptId: event.target.value }).catch(e => setNotice(String(e)))}>{snapshot.prompts.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}</select>
          <button className="text-button" aria-label="编辑提示词" title="编辑提示词" onClick={() => setPanel('prompts')}><ArrowUpRight size={12}/></button>
        </div>
        <div className="keyboard-hints"><span><kbd>Enter</kbd> 发送 <span className="hint-gap"/><kbd>Shift+Enter</kbd> 换行</span><span className="history-hint">{settings.recordHistory ? '本机历史已开启' : '不保存历史'} <span className="context-separator">·</span> <kbd>{settings.shortcut.replace('Control', 'Ctrl').replace('Command', '⌘')}</kbd> 唤起</span></div>
      </footer>
    </div>
    {notice && <div role="status" className="toast"><span>{notice}</span><button aria-label="关闭提示" onClick={() => setNotice('')}><X size={14}/></button></div>}
    {panel && <WorkspacePanel panel={panel} setPanel={setPanel} snapshot={snapshot} reload={reload} preferences={preferences} notice={setNotice} busy={busy} setTesting={setTesting} onCopy={copy} onRestore={entry => { scrollToLatest(); restore(entry); setPanel(null); input.current?.focus(); }}/ >}
  </main>;
}
