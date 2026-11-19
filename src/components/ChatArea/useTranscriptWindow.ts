import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { getMessagePage, type MessagePage, type MessagePageCursor } from '$lib/commands/message';
import type { Message } from '$types';

export const TRANSCRIPT_PAGE_SIZE = 80;

type PageBounds = Pick<MessagePage, 'hasOlder' | 'hasNewer'> & { firstId?: string; lastId?: string };
type HistoryWindow = { sessionId: string; messages: Message[]; bounds: PageBounds };
type ScrollRequest = { id: string; focus: boolean; top?: number };

export function messagePageBounds(page: MessagePage): PageBounds {
  return { firstId: page.messages[0]?.id, lastId: page.messages.at(-1)?.id, hasOlder: page.hasOlder, hasNewer: page.hasNewer };
}

export function findTranscriptRow(container: HTMLElement, id: string) {
  return [...container.querySelectorAll<HTMLElement>('[data-conversation-message-id]')]
    .find((row) => row.dataset.conversationMessageId === id);
}

export function focusTranscriptRow(row: HTMLElement) {
  if (!row.hasAttribute('tabindex')) {
    row.tabIndex = -1;
    row.addEventListener('blur', () => row.removeAttribute('tabindex'), { once: true });
  }
  row.focus({ preventScroll: true });
}

function joinPages(first: Message[], second: Message[]) {
  const seen = new Set(first.map(({ id }) => id));
  return [...first, ...second.filter(({ id }) => !seen.has(id))];
}

export function useTranscriptWindow({ sessionId, messages, tail, visible, captureOwner, onTailPage }: {
  sessionId?: string;
  messages: Message[];
  tail: PageBounds | null;
  visible: (message: Message) => boolean;
  captureOwner: () => () => boolean;
  onTailPage: (page: MessagePage) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [history, setHistory] = useState<HistoryWindow | null>(null);
  const [paging, setPaging] = useState(false);
  const [pagingError, setPagingError] = useState<string | null>(null);
  const requestRef = useRef<symbol | null>(null);
  const pendingScroll = useRef<ScrollRequest | null>(null);
  const focusFrame = useRef<number | null>(null);
  const followLatest = useRef(true);
  const historicalPage = history?.sessionId === sessionId ? history : null;
  const bounds = historicalPage?.bounds ?? tail;
  const windowMessages = historicalPage?.messages ?? messages;
  const renderedMessages = useMemo(() => windowMessages.filter(visible), [windowMessages, visible]);
  const hasNewer = Boolean(historicalPage);
  const hasOlder = Boolean(bounds?.hasOlder);
  const hasMoreNewer = Boolean(historicalPage && (bounds?.hasNewer || (tail?.lastId && tail.lastId !== bounds?.lastId)));

  const cancelFocus = useCallback(() => {
    if (focusFrame.current !== null) cancelAnimationFrame(focusFrame.current);
    focusFrame.current = null;
  }, []);

  useLayoutEffect(() => {
    followLatest.current = true;
    setHistory(null);
    setPaging(false);
    setPagingError(null);
    return () => { requestRef.current = null; pendingScroll.current = null; cancelFocus(); };
  }, [sessionId, cancelFocus]);

  const scrollToRow = useCallback((container: HTMLElement, row: HTMLElement, request: ScrollRequest) => {
    container.scrollTop += row.getBoundingClientRect().top - (request.top ?? container.getBoundingClientRect().top + 20);
    if (request.focus) {
      focusFrame.current = requestAnimationFrame(() => {
        focusFrame.current = null;
        if (!containerRef.current?.contains(row)) return;
        focusTranscriptRow(row);
      });
    }
  }, []);

  useLayoutEffect(() => {
    const request = pendingScroll.current;
    pendingScroll.current = null;
    const container = containerRef.current;
    if (!container || !request) return;
    const row = findTranscriptRow(container, request.id);
    if (row) scrollToRow(container, row, request);
  }, [renderedMessages, scrollToRow]);

  const updateFollowLatest = useCallback(() => {
    const container = containerRef.current;
    if (container) followLatest.current = !hasNewer && container.scrollHeight - container.scrollTop - container.clientHeight < 96;
  }, [hasNewer]);

  const scrollToLatestIfFollowing = useCallback(() => {
    const container = containerRef.current;
    if (container && followLatest.current && !hasNewer) container.scrollTop = container.scrollHeight;
  }, [hasNewer]);

  const returnToLatest = useCallback(() => {
    requestRef.current = null;
    pendingScroll.current = null;
    cancelFocus();
    setPaging(false);
    setPagingError(null);
    followLatest.current = true;
    if (hasNewer) setHistory(null);
    else scrollToLatestIfFollowing();
  }, [cancelFocus, hasNewer, scrollToLatestIfFollowing]);

  const preserveReadingWindow = (previous: Message[], previousBounds: PageBounds) => {
    if (!sessionId || followLatest.current || historicalPage) return;
    const container = containerRef.current;
    if (container && !pendingScroll.current) {
      const top = container.getBoundingClientRect().top;
      const anchor = [...container.querySelectorAll<HTMLElement>('[data-conversation-message-id]')]
        .find(row => row.getBoundingClientRect().bottom >= top);
      if (anchor?.dataset.conversationMessageId) pendingScroll.current = {
        id: anchor.dataset.conversationMessageId, focus: false, top: anchor.getBoundingClientRect().top,
      };
    }
    setHistory(current => current ?? { sessionId, messages: previous, bounds: previousBounds });
  };

  const readPage = async (cursor: MessagePageCursor, apply: (page: MessagePage) => boolean) => {
    if (!sessionId || requestRef.current) return false;
    const owner = captureOwner();
    const request = Symbol();
    requestRef.current = request;
    setPaging(true);
    setPagingError(null);
    try {
      const page = await getMessagePage(sessionId, cursor);
      return owner() && requestRef.current === request && apply(page);
    } catch {
      if (owner() && requestRef.current === request) setPagingError('暂时无法加载这段对话，请重试。');
      return false;
    } finally {
      if (requestRef.current === request) {
        requestRef.current = null;
        setPaging(false);
      }
    }
  };

  const jumpToMessage = (id: string, focus: boolean, preferTail = false) => {
    const container = containerRef.current;
    if (!container || !sessionId) return false;
    const row = findTranscriptRow(container, id);
    const request = { id, focus };
    if (historicalPage && (preferTail || !row) && messages.some(message => message.id === id && visible(message))) {
      returnToLatest();
      followLatest.current = false;
      pendingScroll.current = request;
      return true;
    }
    if (row) {
      followLatest.current = false;
      cancelFocus();
      scrollToRow(container, row, request);
      return true;
    }
    return readPage({ aroundId: id }, page => {
      if (!page.messages.some(message => message.id === id && visible(message))) return false;
      followLatest.current = false;
      cancelFocus();
      pendingScroll.current = request;
      // Keep the newest page live while the reader inspects an older window.
      setHistory({ sessionId, messages: page.messages, bounds: messagePageBounds(page) });
      return true;
    });
  };

  const loadAdjacent = (direction: 'older' | 'newer', keyboard: boolean, button: HTMLButtonElement) => {
    const older = direction === 'older';
    const id = older ? bounds?.firstId : bounds?.lastId;
    if (!sessionId || !bounds || !id || (older ? !hasOlder : !hasMoreNewer)) return;
    followLatest.current = false;
    void readPage(older ? { beforeId: id } : { newerId: id }, page => {
      if (page.reset) {
        returnToLatest();
        onTailPage(page);
        return true;
      }
      const container = containerRef.current;
      const anchorId = (older ? renderedMessages[0] : renderedMessages.at(-1))?.id;
      const anchor = container && anchorId ? findTranscriptRow(container, anchorId) : undefined;
      followLatest.current = false;
      cancelFocus();
      if (anchor && anchorId) pendingScroll.current = { id: anchorId, top: anchor.getBoundingClientRect().top,
        focus: document.activeElement === button && (keyboard || !(older ? page.hasOlder : page.hasNewer)) };
      const nextBounds = messagePageBounds(page);
      setHistory({ sessionId,
        messages: older ? joinPages(page.messages, windowMessages) : joinPages(windowMessages, page.messages),
        bounds: {
          firstId: (older ? nextBounds.firstId : bounds.firstId) ?? bounds.firstId,
          lastId: (older ? bounds.lastId : nextBounds.lastId) ?? bounds.lastId,
          hasOlder: older ? page.hasOlder : bounds.hasOlder,
          hasNewer: older ? bounds.hasNewer : page.hasNewer,
        },
      });
      return true;
    });
  };

  const updateHistoricalMessage = (saved: Message, hydrated: Message) => {
    if (!historicalPage?.messages.includes(saved)) return false;
    setHistory(current => current?.sessionId === sessionId && current?.messages.includes(saved) ? {
      ...current,
      messages: current.messages.map(message => message === saved ? hydrated : message),
    } : current);
    return true;
  };

  return { containerRef, windowMessages, renderedMessages, hasOlder, hasNewer, hasMoreNewer, preserveReadingWindow,
    paging, pagingError, loadAdjacent, jumpToMessage, returnToLatest, updateHistoricalMessage, updateFollowLatest, scrollToLatestIfFollowing };
}
