import { useEffect, useId, useMemo, useRef, useState, type RefObject } from 'react';
import type { Message } from '$types';
import './ConversationNavigation.css';

const INDEX_LIMIT = 50;
const previewText = (text: string, limit: number) => text.trimStart().slice(0, limit * 4).replace(/^\s{0,3}#{1,6}\s+/gm, '').replace(/\s+/g, ' ').trim().slice(0, limit);

/** A bounded view of visible user turns, not another history store or branch switch. */
export function conversationIndex(messages: Message[], sessionId?: string) {
  const entries: Array<{ id: string; title: string; reply: string; createdAt: number }> = [];
  let reply = '';
  if (!sessionId) return entries;
  for (let index = messages.length - 1; index >= 0 && entries.length < INDEX_LIMIT; index -= 1) {
    const message = messages[index];
    if (message.sessionId !== sessionId) continue;
    if (message.role === 'assistant' && !reply) {
      const text = message.content.trim() || message.timelineBlocks?.flatMap(block => block.kind === 'text' ? [block.content] : []).join('\n\n') || '';
      reply = previewText(text, 240);
    }
    if (message.role !== 'user') continue;
    const title = previewText(message.content, 180)
      || (message.textAttachments?.length ? `附件：${message.textAttachments.map(file => file.name).join('、')}` : '附件消息');
    entries.push({ id: message.id, title: title.slice(0, 180), reply, createdAt: message.createdAt });
    reply = '';
  }
  return entries.reverse();
}

export function ConversationNavigation({ messages, sessionId, containerRef, onNavigateAway, onReturnToLatest }: {
  messages: Message[];
  sessionId?: string;
  containerRef: RefObject<HTMLDivElement>;
  onNavigateAway: () => void;
  onReturnToLatest: () => void;
}) {
  const entries = useMemo(() => conversationIndex(messages, sessionId), [messages, sessionId]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [previewId, setPreviewId] = useState<string | null>(null);
  const [previewTop, setPreviewTop] = useState(8);
  const [focusId, setFocusId] = useState<string | null>(null);
  const [showLatest, setShowLatest] = useState(false);
  const navRef = useRef<HTMLElement>(null);
  const tooltipId = useId();
  const preview = entries.find(entry => entry.id === previewId);
  const tabStop = entries.find(entry => entry.id === focusId)?.id ?? entries.find(entry => entry.id === activeId)?.id ?? entries[entries.length - 1]?.id;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let frame: number | null = null;
    const anchors = [...container.querySelectorAll<HTMLElement>('[data-conversation-message-id]')];
    const ids = new Set(entries.map(entry => entry.id));
    const targets = anchors.filter(node => ids.has(node.dataset.conversationMessageId ?? ''));
    const update = () => {
      frame = null;
      setShowLatest(container.scrollHeight - container.scrollTop - container.clientHeight >= 96);
      const top = container.getBoundingClientRect().top + 40;
      let current: string | null = null;
      for (const target of targets) {
        if (target.getBoundingClientRect().top > top) break;
        current = target.dataset.conversationMessageId ?? null;
      }
      setActiveId(current);
    };
    const schedule = () => { if (frame === null) frame = requestAnimationFrame(update); };
    container.addEventListener('scroll', schedule, { passive: true });
    window.addEventListener('resize', schedule);
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(schedule);
    observer?.observe(container);
    if (container.lastElementChild) observer?.observe(container.lastElementChild);
    update();
    return () => {
      container.removeEventListener('scroll', schedule);
      window.removeEventListener('resize', schedule);
      observer?.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, [containerRef, entries]);

  const reveal = (id: string, button: HTMLButtonElement) => {
    const container = containerRef.current;
    if (!container) return;
    setPreviewTop(Math.max(8, Math.min(button.getBoundingClientRect().top - container.getBoundingClientRect().top - 16, container.clientHeight - 176)));
    setPreviewId(id);
  };
  const jump = (id: string) => {
    const container = containerRef.current;
    const target = [...(container?.querySelectorAll<HTMLElement>('[data-conversation-message-id]') ?? [])]
      .find(node => node.dataset.conversationMessageId === id);
    if (!container || !target) return;
    onNavigateAway();
    container.scrollTop = Math.max(0, container.scrollTop + target.getBoundingClientRect().top - container.getBoundingClientRect().top - 20);
    setActiveId(id);
    setShowLatest(container.scrollHeight - container.scrollTop - container.clientHeight >= 96);
    setPreviewId(null);
  };

  return <>
    {entries.length > 0 && <nav ref={navRef} className="conversation-index" aria-label="对话定位" onMouseLeave={() => setPreviewId(null)} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null)) { setPreviewId(null); setFocusId(null); }
    }}>
      <ol style={{ maxHeight: entries.length * 14 }}>
        {entries.map((entry, index) => <li key={entry.id}><button type="button" data-index-id={entry.id}
          aria-label={`定位第 ${index + 1} 条：${entry.title}`} aria-current={activeId === entry.id ? 'location' : undefined}
          aria-describedby={previewId === entry.id ? tooltipId : undefined} tabIndex={tabStop === entry.id ? 0 : -1}
          onMouseEnter={event => reveal(entry.id, event.currentTarget)} onFocus={event => { setFocusId(entry.id); reveal(entry.id, event.currentTarget); }}
          onClick={() => jump(entry.id)} onKeyDown={event => {
            if (event.key === 'Escape') { setPreviewId(null); return; }
            const next = event.key === 'ArrowDown' ? Math.min(entries.length - 1, index + 1)
              : event.key === 'ArrowUp' ? Math.max(0, index - 1) : event.key === 'Home' ? 0 : event.key === 'End' ? entries.length - 1 : null;
            if (next === null) return;
            event.preventDefault();
            navRef.current?.querySelectorAll<HTMLButtonElement>('button')[next]?.focus();
          }}><span aria-hidden="true" /></button></li>)}
      </ol>
    </nav>}
    {preview && <div className="conversation-index-preview" role="tooltip" id={tooltipId} style={{ top: previewTop }}>
      <strong>{preview.title}</strong><p>{preview.reply || '暂无文字回复'}</p>
      <small>{entries.indexOf(preview) + 1} / {entries.length}<span>{new Date(preview.createdAt < 10_000_000_000 ? preview.createdAt * 1000 : preview.createdAt).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span></small>
    </div>}
    {showLatest && messages.length > 0 && <button type="button" className="conversation-latest" aria-label="回到最底部" title="回到最底部" onClick={() => {
      onReturnToLatest(); setShowLatest(false); setPreviewId(null); setActiveId(entries[entries.length - 1]?.id ?? null);
    }}><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M12 4v16m-6-6 6 6 6-6" /></svg></button>}
  </>;
}
