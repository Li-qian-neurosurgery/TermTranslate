import { useRef, useState } from 'react';
import { Channel, invoke } from '@tauri-apps/api/core';
import type { HistoryEntry, Profile, Settings, TranslationEvent, TranslationState } from './types';
import { receiveEvent } from './types';

export interface Turn extends TranslationState {
  id: string;
  source: string;
  model: string;
  profileName: string;
  promptName: string;
  targetLanguage: string;
  startedAt: number;
  elapsedMs: number;
  restored?: boolean;
}

export function useConversation(reload: () => Promise<void>, notice: (text: string) => void) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const active = useRef<{ id: string; startedAt: number } | null>(null);
  const running = turns.some(turn => turn.status === 'running');

  const send = async (text: string, profile: Profile, settings: Settings, promptName: string) => {
    if (!text.trim()) return;
    const requestId = crypto.randomUUID();
    const startedAt = Date.now();
    active.current = { id: requestId, startedAt };
    setTurns(previous => [...previous.map(turn => turn.status === 'running'
      ? { ...turn, status: 'cancelled' as const, message: '已停止，开始下一条翻译。', elapsedMs: startedAt - turn.startedAt }
      : turn), {
      id: requestId, requestId, source: text, output: '', status: 'running', message: '',
      model: profile.model, profileName: profile.name, promptName, targetLanguage: settings.targetLanguage,
      startedAt, elapsedMs: 0,
    }]);
    const channel = new Channel<TranslationEvent>();
    channel.onmessage = event => {
      // A cancelled/replaced stream cannot change either an earlier turn or the active reply.
      if (active.current?.id !== event.requestId) return;
      const terminal = event.kind === 'completed' || event.kind === 'error' || event.kind === 'cancelled';
      const elapsedMs = Date.now() - startedAt;
      setTurns(previous => previous.map(turn => turn.requestId === requestId
        ? { ...turn, ...receiveEvent(turn, event), elapsedMs } : turn));
      if (terminal) {
        active.current = null;
        void reload().catch(error => notice(String(error)));
      }
    };
    try {
      await invoke('start_translation', {
        request: { requestId, profileId: profile.id, promptId: settings.activePromptId, text, targetLanguage: settings.targetLanguage },
        onEvent: channel,
      });
    } catch (error) {
      if (active.current?.id !== requestId) return;
      active.current = null;
      setTurns(previous => previous.map(turn => turn.requestId === requestId
        ? { ...turn, status: 'error', message: String(error), elapsedMs: Date.now() - startedAt } : turn));
    }
  };

  const stop = async () => {
    const requestId = active.current?.id;
    if (!requestId) return;
    try { await invoke('cancel_translation', { requestId }); }
    catch (error) { notice(String(error)); }
  };

  const clear = () => {
    const requestId = active.current?.id;
    active.current = null;
    setTurns([]);
    if (requestId) void invoke('cancel_translation', { requestId }).catch(error => notice(String(error)));
  };

  const restore = (entry: HistoryEntry) => {
    setTurns(previous => [...previous, {
      id: crypto.randomUUID(), requestId: '', source: entry.source, output: entry.translation,
      status: 'completed', message: '', model: entry.model, profileName: entry.profileName,
      promptName: entry.promptName, targetLanguage: entry.targetLanguage,
      startedAt: new Date(entry.createdAt).getTime(), elapsedMs: 0, restored: true,
    }]);
  };

  return { turns, running, send, stop, clear, restore };
}
