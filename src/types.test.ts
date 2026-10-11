import { describe, expect, it } from 'vitest';
import { emptyTranslation, receiveEvent, shouldTranslate } from './types';

describe('translation lifecycle', () => {
  it('ignores a late response after a new request or clear', () => {
    const state = { ...emptyTranslation, requestId: 'new', status: 'running' as const, output: '新译文' };
    expect(receiveEvent(state, { requestId: 'old', kind: 'completed', text: '旧译文', message: '' })).toBe(state);
    expect(receiveEvent(emptyTranslation, { requestId: 'old', kind: 'delta', text: '旧译文', message: '' })).toBe(emptyTranslation);
  });
  it('preserves partial output on errors and cancellation, ignores late deltas', () => {
    const running = { ...emptyTranslation, requestId: 'one', status: 'running' as const, output: '部分译文🙂' };
    for (const kind of ['error', 'cancelled'] as const) {
      const ended = receiveEvent(running, { requestId: 'one', kind, text: '', message: '未完成' });
      expect(ended.output).toBe('部分译文🙂');
      expect(receiveEvent(ended, { requestId: 'one', kind: 'delta', text: 'late', message: '' })).toBe(ended);
    }
  });
  it('commits the canonical completed text', () => {
    const running = { ...emptyTranslation, requestId: 'one', status: 'running' as const, output: '部分' };
    expect(receiveEvent(running, { requestId: 'one', kind: 'completed', text: '完整译文', message: '' }).output).toBe('完整译文');
  });
});
describe('keyboard input', () => {
  it('sends on Enter and keeps Shift+Enter and Chinese composition as text input', () => {
    expect(shouldTranslate({ key: 'Enter', ctrlKey: true, metaKey: false, isComposing: true })).toBe(false);
    expect(shouldTranslate({ key: 'Enter', ctrlKey: true, metaKey: false, keyCode: 229 })).toBe(false);
    expect(shouldTranslate({ key: 'Enter', ctrlKey: false, metaKey: false, shiftKey: true })).toBe(false);
    expect(shouldTranslate({ key: 'Enter', ctrlKey: true, metaKey: false, shiftKey: true })).toBe(false);
    expect(shouldTranslate({ key: 'Enter', ctrlKey: false, metaKey: false, altKey: true })).toBe(false);
    expect(shouldTranslate({ key: 'Enter', ctrlKey: false, metaKey: false })).toBe(true);
    expect(shouldTranslate({ key: 'Enter', ctrlKey: true, metaKey: false })).toBe(true);
    expect(shouldTranslate({ key: 'Enter', ctrlKey: false, metaKey: true })).toBe(true);
  });
});
