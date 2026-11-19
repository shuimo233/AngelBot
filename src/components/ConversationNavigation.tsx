import { useCallback, useEffect, useId, useMemo, useRef, useState, type RefObject } from 'react';
import type { Message } from '$types';
import type { ConversationSearchHit, ConversationSearchPage, searchConversationPage } from '$lib/commands/message';
import { Button, Dialog, Input } from '../ui';
import { completedReplyDeliveries, type DeliveredFile } from './CompletedFileTask';
import { workspaceFilePathFromHref } from './MessageMarkdown';
import { findTranscriptRow, focusTranscriptRow } from './ChatArea/useTranscriptWindow';
import './ConversationNavigation.css';

const INDEX_LIMIT = 50;
const SEARCH_RESULT_PAGE_SIZE = 20;
const SEARCH_TYPING_PAUSE_MS = 150;
type SearchPageState = ConversationSearchPage & { scope: string; pending: boolean; error?: string };
const previewText = (text: string, limit: number) => text.trimStart().slice(0, limit * 4).replace(/^\s{0,3}#{1,6}\s+/gm, '').replace(/\s+/g, ' ').trim().slice(0, limit);
const messageDate = (createdAt: number) => new Date(createdAt < 10_000_000_000 ? createdAt * 1000 : createdAt).toLocaleString(
  undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' },
);

function matchFileCitation(texts: string[], query: RegExp) {
  for (const text of texts) {
    for (const citation of text.matchAll(/(?:\]\(\s*<?|<)(angelbot-file:[^\s<>()]+)/gi)) {
      const path = workspaceFilePathFromHref(citation[1]);
      if (path && query.test(path)) return path;
    }
  }
}

function searchMessage(message: Message, literal: RegExp): ConversationSearchHit | undefined {
  const filenames = [...(message.textAttachments?.map(file => file.name) ?? []), ...(message.taskFacts?.modifiedFiles ?? [])];
  const texts = [message.content, ...(message.timelineBlocks?.flatMap(block => block.kind === 'text' ? [block.content] : []) ?? [])];
  const filename = filenames.find(name => literal.test(name)) ?? matchFileCitation(texts, literal);
  const match = texts.find(text => literal.test(text));
  if (!filename && !match) return undefined;
  const at = match?.search(literal) ?? 0;
  const start = Math.max(0, at - 28);
  const text = filename ?? `${start ? '…' : ''}${previewText(match?.slice(start) ?? '', 156)}`;
  return { id: message.id, text, role: message.role === 'user' ? 'user' : 'assistant', createdAt: message.createdAt };
}

function searchConversation(messages: Message[], sessionId: string | undefined, query: string, limit: number) {
  const results: Array<NonNullable<ReturnType<typeof searchMessage>>> = [];
  const needle = query.trim();
  if (!sessionId || !needle) return results;
  const literal = new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'iu');
  const firstQuestion = messages.findIndex(message => message.sessionId === sessionId && message.role === 'user');
  let replyMatch: ReturnType<typeof searchMessage>;
  // Keep one extra match so we can offer another batch without searching past it.
  for (let index = messages.length - 1; index >= 0 && results.length <= limit; index -= 1) {
    const message = messages[index];
    if (message.sessionId !== sessionId || message.role === 'tool') continue;
    if (message.role === 'assistant' && firstQuestion !== -1 && index > firstQuestion) {
      replyMatch ??= searchMessage(message, literal);
      continue;
    }
    const result = searchMessage(message, literal) ?? replyMatch;
    if (result) results.push(result);
    replyMatch = undefined;
  }
  return results;
}

function isGenericFollowup(text: string) {
  if (text.trim().length > 24) return false;
  const compact = text.replace(/[\s\p{P}]+/gu, '').toLowerCase();
  return /^(?:好的?|可以的?|行|同意|接受|确认|没问题|就这样|按你说的做|按建议做|yes|okay|ok|continue|goahead)$/.test(compact)
    || /^(?:(?:好(?:的)?|可以|那就|先))?(?:继续(?:推进|优化|完善|做|吧)?|推进|优化|完善)(?:吧)?$/.test(compact);
}

type DeliveryIndex = ReadonlyMap<string, readonly DeliveredFile[]>;

function belongsToTurn(reply: Message, questionId: string, byId: ReadonlyMap<string, Message>) {
  const seen = new Set<string>();
  let current: Message | undefined = reply;
  while (current?.role === 'assistant' && current.parentId && !seen.has(current.id)) {
    if (current.parentId === questionId) return true;
    seen.add(current.id);
    current = byId.get(current.parentId);
  }
  return false;
}

function turnHint(question: Message, following: readonly Message[], byId: ReadonlyMap<string, Message>, deliveries: DeliveryIndex): string | undefined {
  // The index groups by visible order, but a short title may only borrow
  // evidence from a linked visible reply. Its chain may include hidden tool rows.
  const linked = following.filter(reply => belongsToTurn(reply, question.id, byId));
  for (const reply of linked) {
    const file = deliveries.get(reply.id)?.[0];
    if (file) return compactHint(file.path.slice(file.path.lastIndexOf('/') + 1));
  }
  for (const reply of linked) {
    for (const goal of [reply.taskRun?.goal, reply.taskFacts?.goal]) {
      const value = goal?.trim() ?? '';
      if (value.length >= 4 && !isGenericFollowup(value) && /[\p{L}\p{N}]/u.test(value)) return compactHint(value);
    }
  }
}

function compactHint(text: string) {
  const normalized = previewText(text, 43);
  return normalized.length <= 42 ? normalized : `${normalized.slice(0, 41)}…`;
}

/** A bounded view of visible user turns, not another history store or branch switch. */
export function conversationIndex(messages: Message[], sessionId?: string, evidenceMessages: readonly Message[] = messages, deliveries?: DeliveryIndex) {
  const entries: Array<{ id: string; title: string; reply: string; createdAt: number; hint?: string }> = [];
  const following: Message[] = [];
  let reply = '';
  if (!sessionId) return entries;
  let evidenceById: Map<string, Message> | undefined;
  for (let index = messages.length - 1; index >= 0 && entries.length < INDEX_LIMIT; index -= 1) {
    const message = messages[index];
    if (message.sessionId !== sessionId) continue;
    if (message.role === 'assistant') {
      following.push(message);
      if (!reply) {
        const text = message.content.trim() || message.timelineBlocks?.flatMap(block => block.kind === 'text' ? [block.content] : []).join('\n\n') || '';
        reply = previewText(text, 240);
      }
    }
    if (message.role !== 'user') continue;
    const title = previewText(message.content, 180)
      || (message.textAttachments?.length ? `附件：${message.textAttachments.map(file => file.name).join('、')}` : '附件消息');
    let hint: string | undefined;
    if (following.length && isGenericFollowup(message.content)) {
      evidenceById ??= new Map(evidenceMessages.filter(item => item.sessionId === sessionId).map(item => [item.id, item]));
      deliveries ??= completedReplyDeliveries(evidenceMessages, sessionId);
      hint = turnHint(message, following, evidenceById, deliveries);
    }
    entries.push({ id: message.id, title: title.slice(0, 180), reply, createdAt: message.createdAt, hint });
    reply = '';
    following.length = 0;
  }
  return entries.reverse();
}

export function ConversationNavigation({ messages, renderedMessages, sessionId, evidenceMessages, replyDeliveries, containerRef, onJumpToMessage, onReturnToLatest, showReturnToLatest, loadSearchPage }: {
  messages: Message[];
  renderedMessages: Message[];
  sessionId?: string;
  evidenceMessages?: readonly Message[];
  replyDeliveries?: DeliveryIndex;
  containerRef: RefObject<HTMLDivElement>;
  onJumpToMessage: (id: string, focus: boolean) => boolean | Promise<boolean>;
  onReturnToLatest: () => void;
  showReturnToLatest: boolean;
  loadSearchPage?: typeof searchConversationPage;
}) {
  const entries = useMemo(() => conversationIndex(messages, sessionId, evidenceMessages, replyDeliveries), [messages, sessionId, evidenceMessages, replyDeliveries]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [previewId, setPreviewId] = useState<string | null>(null);
  const [previewTop, setPreviewTop] = useState(8);
  const [focusId, setFocusId] = useState<string | null>(null);
  const [showLatest, setShowLatest] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [resultLimit, setResultLimit] = useState(SEARCH_RESULT_PAGE_SIZE);
  const resultListRef = useRef<HTMLUListElement>(null);
  const focusNextResult = useRef<number | null>(null);
  const [searchPage, setSearchPage] = useState<SearchPageState | null>(null);
  const searchRequest = useRef<symbol | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [openingResult, setOpeningResult] = useState(false);
  const searchVisit = useRef(0);
  const focusAfterSearch = useRef<string | null>(null);
  const navRef = useRef<HTMLElement>(null);
  const tooltipId = useId();
  const needle = query.trim();
  const searchScope = loadSearchPage && searchOpen && sessionId && needle ? JSON.stringify([sessionId, needle]) : '';
  const remote = searchPage?.scope === searchScope ? searchPage : null;
  const localMatches = useMemo(() => loadSearchPage ? [] : searchConversation(messages, sessionId, needle, resultLimit),
    [loadSearchPage, messages, sessionId, needle, resultLimit]);
  const results = loadSearchPage ? remote?.results ?? [] : localMatches.slice(0, resultLimit);
  const hasEarlierMatches = loadSearchPage ? Boolean(remote?.nextBeforeId) : localMatches.length > resultLimit;
  const searchPending = Boolean(searchScope && (!remote || remote.pending));
  useEffect(() => {
    const index = focusNextResult.current;
    if (index === null) return;
    focusNextResult.current = null;
    resultListRef.current?.children.item(index)?.querySelector('button')?.focus();
  }, [results.length]);
  const requestSearchPage = useCallback(async (previous?: SearchPageState) => {
    if (!searchScope || !sessionId || !loadSearchPage || searchRequest.current) return;
    const beforeId = previous?.nextBeforeId;
    if (previous && !beforeId) return;
    const ticket = Symbol();
    searchRequest.current = ticket;
    const pending = { scope: searchScope, results: previous?.results ?? [], nextBeforeId: beforeId ?? null, pending: true };
    setSearchPage(pending);
    try {
      const page = await loadSearchPage({ sessionId, query: needle, ...(beforeId ? { beforeId } : {}) });
      if (searchRequest.current !== ticket) return;
      const seen = new Set(pending.results.map(hit => hit.id));
      const added = page.results.filter(hit => !seen.has(hit.id));
      if (previous && added.length) focusNextResult.current = pending.results.length;
      setSearchPage({ scope: searchScope, results: [...pending.results, ...added],
        nextBeforeId: page.nextBeforeId === beforeId ? null : page.nextBeforeId, pending: false });
    } catch {
      if (searchRequest.current === ticket) setSearchPage({ ...pending, pending: false,
        error: beforeId ? '暂时无法加载更早匹配，请重试。' : '暂时无法搜索对话，请重试。' });
    } finally {
      if (searchRequest.current === ticket) searchRequest.current = null;
    }
  }, [searchScope, sessionId, loadSearchPage, needle]);
  useEffect(() => {
    if (!searchScope) return;
    const timer = window.setTimeout(() => void requestSearchPage(), SEARCH_TYPING_PAUSE_MS);
    return () => { window.clearTimeout(timer); searchRequest.current = null; };
  }, [searchScope, requestSearchPage]);
  const preview = entries.find(entry => entry.id === previewId);
  const tabStop = entries.find(entry => entry.id === focusId)?.id ?? entries.find(entry => entry.id === activeId)?.id ?? entries[entries.length - 1]?.id;

  useEffect(() => {
    if (searchOpen || !focusAfterSearch.current) return;
    const frame = requestAnimationFrame(() => {
      const id = focusAfterSearch.current;
      const container = containerRef.current;
      const row = id && container ? findTranscriptRow(container, id) : undefined;
      if (!row) return;
      focusAfterSearch.current = null;
      // The dialog restores its trigger on close. A result selection is an
      // intentional transfer of focus into the newly mounted message instead.
      focusTranscriptRow(row);
    });
    return () => cancelAnimationFrame(frame);
  }, [searchOpen, renderedMessages, containerRef]);

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
  }, [containerRef, entries, renderedMessages]);

  const reveal = (id: string, button: HTMLButtonElement) => {
    const container = containerRef.current;
    if (!container) return;
    setPreviewTop(Math.max(8, Math.min(button.getBoundingClientRect().top - container.getBoundingClientRect().top - 16, container.clientHeight - 176)));
    setPreviewId(id);
  };
  const markJump = (id: string) => {
    setActiveId(id);
    const container = containerRef.current;
    if (container) setShowLatest(container.scrollHeight - container.scrollTop - container.clientHeight >= 96);
    setPreviewId(null);
  };
  const updateQuery = (value: string) => {
    setQuery(value);
    if (value.trim() === needle) return;
    searchRequest.current = null;
    setSearchPage(null);
    setSearchError(null);
    setResultLimit(SEARCH_RESULT_PAGE_SIZE);
    focusNextResult.current = null;
    if (resultListRef.current) resultListRef.current.scrollTop = 0;
  };
  const closeSearch = () => {
    searchVisit.current += 1;
    setSearchOpen(false);
    updateQuery('');
    setSearchError(null);
    setOpeningResult(false);
  };
  const jumpToSearchResult = async (id: string) => {
    if (openingResult) return;
    const visit = searchVisit.current;
    setOpeningResult(true);
    setSearchError(null);
    const found = await onJumpToMessage(id, true);
    if (visit !== searchVisit.current) return;
    if (found) { focusAfterSearch.current = id; markJump(id); closeSearch(); }
    else {
      setOpeningResult(false);
      setSearchError('暂时无法打开这条消息，它可能不在当前分支。');
    }
  };

  return <>
    {(entries.length > 0 || loadSearchPage) && <Button type="button" variant="ghost" className="conversation-search-trigger" aria-label="搜索当前对话"
      aria-haspopup="dialog" title="搜索当前对话" onClick={() => { setPreviewId(null); setSearchOpen(true); }}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><circle cx="10.5" cy="10.5" r="5.5" /><path d="m15 15 4 4" /></svg>
    </Button>}
    {entries.length > 0 && <nav ref={navRef} className="conversation-index" aria-label="对话定位" onMouseLeave={() => setPreviewId(null)} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null)) { setPreviewId(null); setFocusId(null); }
    }}>
      <ol style={{ maxHeight: entries.length * 14 }}>
        {entries.map((entry, index) => <li key={entry.id}><button type="button" data-index-id={entry.id}
          aria-label={`定位第 ${index + 1} 条：${entry.title}${entry.hint ? ` · ${entry.hint}` : ''}`} aria-current={activeId === entry.id ? 'location' : undefined}
          aria-describedby={previewId === entry.id ? tooltipId : undefined} tabIndex={tabStop === entry.id ? 0 : -1}
          onMouseEnter={event => reveal(entry.id, event.currentTarget)} onFocus={event => { setFocusId(entry.id); reveal(entry.id, event.currentTarget); }}
          onClick={() => { void Promise.resolve(onJumpToMessage(entry.id, false)).then(found => { if (found) markJump(entry.id); }); }} onKeyDown={event => {
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
      <strong>{preview.title}{preview.hint && <span className="conversation-index-hint"> · {preview.hint}</span>}</strong><p>{preview.reply || '暂无文字回复'}</p>
      <small>{entries.indexOf(preview) + 1} / {entries.length}<span>{messageDate(preview.createdAt)}</span></small>
    </div>}
    {(showLatest || showReturnToLatest) && messages.length > 0 && <button type="button" className="conversation-latest" aria-label="回到最底部" title="回到最底部" onClick={() => {
      onReturnToLatest(); setShowLatest(false); setPreviewId(null); setActiveId(entries[entries.length - 1]?.id ?? null);
    }}><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M12 4v16m-6-6 6 6 6-6" /></svg></button>}
    <Dialog open={searchOpen} title="搜索当前对话" onClose={closeSearch} className="conversation-search-dialog"
      initialFocusSelector="input">
      <form onSubmit={event => {
        event.preventDefault();
        if (results[0]) jumpToSearchResult(results[0].id);
      }}>
        <Input type="search" aria-label="关键词或文件名" placeholder="关键词或文件名" value={query} maxLength={200}
          autoComplete="off" onKeyDown={event => {
            if (event.key === 'Enter' && event.nativeEvent.isComposing) event.preventDefault();
          }} onChange={event => updateQuery(event.target.value)} />
      </form>
      {needle && <p className="conversation-search-status" role="status">
        {searchError ?? remote?.error ?? (openingResult ? '正在打开消息…' : searchPending ? (results.length ? '正在加载更早匹配…' : '正在搜索对话…')
          : results.length === 0 ? '当前对话没有匹配的消息'
          : hasEarlierMatches ? `显示最近 ${results.length} 条匹配` : `${results.length} 条匹配`)}
      </p>}
      {results.length > 0 && <ul className="conversation-search-results" ref={resultListRef}>
        {results.map(result => <li key={result.id}>
          <button type="button" disabled={openingResult} onClick={() => jumpToSearchResult(result.id)}>
            <span>{result.text}</span>
            <small>{result.role === 'user' ? '你' : 'AngelBot'} · {messageDate(result.createdAt)}</small>
          </button>
        </li>)}
      </ul>}
      <div className="conversation-search-actions">
        {hasEarlierMatches && <Button type="button" variant="secondary" className="conversation-search-more" disabled={openingResult || searchPending} onClick={() => {
          if (remote) void requestSearchPage(remote);
          else {
            focusNextResult.current = resultLimit;
            setResultLimit(resultLimit + SEARCH_RESULT_PAGE_SIZE);
          }
        }}>查看更多匹配</Button>}
        {remote?.error && !remote.results.length && <Button type="button" variant="secondary" className="conversation-search-more"
          onClick={() => void requestSearchPage()}>重试搜索</Button>}
        <Button type="button" variant="secondary" onClick={closeSearch}>关闭</Button>
      </div>
    </Dialog>
  </>;
}
