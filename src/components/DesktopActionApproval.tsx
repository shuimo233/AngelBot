import { useEffect, useRef, useState } from 'react';
import {
  preflightPendingDesktopAction,
  type DesktopActionPreview,
} from '$lib/commands/message';
import { DESKTOP_CONTROL_ACTION_LABELS, WINDOW_IMAGE_CONFIRMATION_SUMMARY } from '$lib/tool-confirmation';
import { PENDING_ACTIONS_BLOCKED_MESSAGE } from '$lib/tool-action-result';
import './DesktopActionApproval.css';

type PreviewState =
  | { status: 'loading' }
  | { status: 'unavailable'; reason?: 'busy' | 'changed' }
  | { status: 'uncertain' }
  | { status: 'settled' }
  | { status: 'ready'; preview: DesktopActionPreview; scopeKey: string };

/** Local UI outcome only; durable confirmation results remain authoritative. */
export type DesktopApprovalResolution = 'recheck_required' | void;

/** ChatArea-local failed preview; survives only its live/persisted card swap. */
export interface DesktopApprovalRecheck {
  sessionId: string;
  messageId: string;
  callId: string;
  previewId: string;
  onRecheck: () => void;
}

function isReviewable(value: DesktopActionPreview, operation: DesktopActionPreview['operation'] | null): boolean {
  const image = value?.image;
  return typeof value?.previewId === 'string' && value.previewId.length > 0
    && value.operation === operation
    && (operation === 'choose'
      ? typeof value.option === 'string' && value.option.trim().length > 0 && value.option.trim() === value.option
        && value.option.length <= 80 && !/[\u0000-\u001f\u007f-\u009f]/.test(value.option)
      : value.option == null)
    && typeof value.appDisplayName === 'string' && value.appDisplayName.trim().length > 0
    && typeof value.executableName === 'string' && value.executableName.trim().length > 0
    && (operation === 'imageclick'
      ? !!image && typeof image.imageDataUrl === 'string' && image.imageDataUrl.startsWith('data:image/png;base64,')
        && image.imageDataUrl.length > 22 && image.imageDataUrl.length <= 2_796_230
        && [image.width, image.height, image.x, image.y].every(Number.isSafeInteger)
        && image.width > 0 && image.width <= 2048 && image.height > 0 && image.height <= 2048
        && image.x >= 0 && image.x < image.width && image.y >= 0 && image.y < image.height
      : image == null)
    && (operation === 'image' || operation === 'imageclick'
      ? value.windowTitle === null && value.controlName === null && value.text === null
      : typeof value.controlName === 'string' && value.controlName.trim().length > 0
        && (operation === 'draft' || operation === 'field'
          ? typeof value.text === 'string' && (operation === 'field' || value.text.length > 0)
          : value.text === null))
    && typeof value.expiresAt === 'number' && Number.isFinite(value.expiresAt) && value.expiresAt > Date.now();
}

/**
 * Only the caller's validated operation crosses this module's interface; the
 * remaining model-authored tool arguments are deliberately absent.
 * Only the backend's durable-step preflight can supply a reviewable target,
 * operation, exact text where applicable, and the short-lived approval ID.
 * Image-observation consent checks the configured app only, without capturing
 * pixels during preflight. Visual-click preflight instead rechecks an already
 * authorized observation and returns a transient image with the exact point.
 */
export function DesktopActionApproval({
  sessionId,
  messageId,
  callId,
  operation,
  layout,
  recheck,
  approvalBlocked = false,
  onResolve,
}: {
  sessionId?: string;
  messageId?: string;
  callId: string;
  operation: DesktopActionPreview['operation'] | null;
  layout: 'compact' | 'stream';
  recheck?: DesktopApprovalRecheck;
  approvalBlocked?: boolean;
  onResolve: (decision: 'approved' | 'rejected', previewId?: string) => DesktopApprovalResolution | Promise<DesktopApprovalResolution>;
}) {
  const scopeKey = JSON.stringify([sessionId, messageId, callId, operation]);
  const recheckPreviewId = recheck?.sessionId === sessionId && recheck?.messageId === messageId
    && recheck?.callId === callId ? recheck.previewId : undefined;
  const currentScope = useRef<string | null>(scopeKey);
  currentScope.current = scopeKey;
  const resolvingScope = useRef<string | null>(null);
  const blockedPreview = useRef<{ scopeKey: string; previewId: string } | null>(null);
  const [resolvingKey, setResolvingKey] = useState<string | null>(null);
  const [state, setState] = useState<PreviewState>({ status: 'loading' });
  const [retry, setRetry] = useState(0);
  const [expired, setExpired] = useState(false);
  const [loadedImageKey, setLoadedImageKey] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    currentScope.current = scopeKey;
    setState({ status: 'loading' });
    setExpired(false);
    setLoadedImageKey(null);
    if (recheckPreviewId) blockedPreview.current = { scopeKey, previewId: recheckPreviewId };
    // Retain this card's invalidation even if ChatArea later reports another
    // failed call. Only an explicit recheck clears the matching local preview.
    if (approvalBlocked || !sessionId || !messageId || !operation || blockedPreview.current?.scopeKey === scopeKey) {
      setState({ status: 'unavailable' });
      return () => { current = false; currentScope.current = null; };
    }
    void preflightPendingDesktopAction({ sessionId, messageId, callId })
      .then((preview) => {
        if (!current) return;
        setState(isReviewable(preview, operation) ? { status: 'ready', preview, scopeKey } : { status: 'unavailable' });
      })
      .catch((error: unknown) => {
        if (!current) return;
        const message = error instanceof Error ? error.message : typeof error === 'string' ? error : '';
        const reason = message.startsWith('TARGET_BUSY:') ? 'busy'
          : message.startsWith('TARGET_CHANGED:') ? 'changed' : undefined;
        setState({ status: 'unavailable', reason });
      });
    return () => { current = false; currentScope.current = null; };
  }, [sessionId, messageId, callId, operation, retry, scopeKey, recheckPreviewId, approvalBlocked]);

  useEffect(() => {
    if (state.status !== 'ready') return;
    const remaining = state.preview.expiresAt - Date.now();
    if (remaining <= 0) {
      setExpired(true);
      return;
    }
    const timer = window.setTimeout(() => { setExpired(true); if (operation === 'imageclick') setState({ status: 'unavailable' }); }, remaining);
    return () => window.clearTimeout(timer);
  }, [state, operation]);

  const preview = state.status === 'ready' && state.scopeKey === scopeKey && !recheckPreviewId
    && !expired && operation && isReviewable(state.preview, operation)
    ? state.preview
    : null;
  const resolving = resolvingKey === scopeKey;
  const imageReady = operation !== 'imageclick' || !!preview
    && loadedImageKey === JSON.stringify([scopeKey, preview.previewId]);
  const resolve = async (decision: 'approved' | 'rejected') => {
    if (decision === 'approved' && approvalBlocked) return;
    if (resolvingScope.current === scopeKey) return;
    if (decision === 'approved' && !imageReady) return;
    if (decision === 'approved' && (!preview || preview.expiresAt <= Date.now())) {
      setExpired(true);
      return;
    }
    resolvingScope.current = scopeKey;
    setResolvingKey(scopeKey);
    try {
      const outcome = await onResolve(decision, decision === 'approved' ? preview?.previewId : undefined);
      if (currentScope.current !== scopeKey) return;
      if (operation === 'imageclick' && outcome !== 'recheck_required') setState({ status: 'settled' });
      if (decision === 'approved' && outcome === 'recheck_required') {
        if (preview) blockedPreview.current = { scopeKey, previewId: preview.previewId };
        setState((current) => current.status === 'ready' && current.scopeKey === scopeKey
          && current.preview.previewId === preview?.previewId ? { status: 'unavailable' } : current);
      }
    } catch {
      // A rejected transport promise is not proof the request was unexecuted.
      if (currentScope.current === scopeKey) setState({ status: 'uncertain' });
    } finally {
      if (resolvingScope.current === scopeKey) resolvingScope.current = null;
      if (currentScope.current === scopeKey) setResolvingKey(null);
    }
  };
  const retryPreflight = () => {
    if (blockedPreview.current?.scopeKey === scopeKey) {
      if (recheckPreviewId === blockedPreview.current.previewId) recheck?.onRecheck();
      blockedPreview.current = null;
    }
    setRetry((value) => value + 1);
  };
  const compact = layout === 'compact';
  const isTextWrite = operation === 'draft' || operation === 'field';
  const isClear = preview?.operation === 'field' && preview.text === '';
  const isImage = operation === 'image';
  const isVisualClick = operation === 'imageclick';
  const actionLabel = isVisualClick ? '单次窗口点击' : operation && !isTextWrite && !isImage ? DESKTOP_CONTROL_ACTION_LABELS[operation] : null;
  const summaryClass = compact ? 'agent-run-confirmation-summary' : 'content-block-tool-confirmation-summary';
  const actionsClass = compact ? 'agent-run-badge-actions' : 'content-block-tool-actions';
  const approveClass = compact ? 'agent-inline-btn agent-inline-btn--approve' : 'content-block-tool-btn content-block-tool-btn--approve';
  const rejectClass = compact ? 'agent-inline-btn agent-inline-btn--reject' : 'content-block-tool-btn content-block-tool-btn--reject';

  return (
    <div className={compact ? 'agent-run-text-confirmation' : 'content-block-tool-confirmation'}>
      <div className={summaryClass}>
        {actionLabel && <strong>待确认操作：{actionLabel}</strong>}
        {isClear && <strong>待确认操作：清空输入框</strong>}
        {preview && isVisualClick ? (
          <>
            已核对目标应用：{preview.appDisplayName}（{preview.executableName}）。
            将在下图标记的位置左键点击一次；可能切换到该窗口，也可能触发发送、删除或购买，请核对当前页面及后果。
            窗口、画面或输入状态变化时拒绝点击，不自动重试；回执只表示输入已派发，不代表业务完成。
          </>
        ) : preview && isImage ? (
          <>
            已核对授权应用：{preview.appDisplayName}（{preview.executableName}）。
            允许后才检查该应用当前唯一可用的主窗口；窗口不可用或目标不明确时停止，不截取图像。
            {WINDOW_IMAGE_CONFIRMATION_SUMMARY}
          </>
        ) : preview ? (
          <>
            已核对目标：{preview.appDisplayName}（{preview.executableName}）
            {preview.windowTitle ? ` · ${preview.windowTitle}` : !isTextWrite ? ' · 未命名窗口' : ''}
            {preview.controlName ? ` · ${preview.controlName}` : !isTextWrite ? ' · 未命名控件' : ''}。
            {operation === 'choose' && <>将选择选项“{preview.option}”：本次批准包含展开、精确选择、状态核验及收起。只接受该下拉框中唯一可见的匹配选项；无法核对则停止，不猜测或重试。</>}
            {operation === 'draft'
              ? '仅填写草稿，不点击发送；请先核对当前会话和收件对象，应用可能自动保存或同步。'
              : operation === 'field'
                ? '将修改这一个输入框，不点击提交；请核对目标窗口与内容，应用可能自动保存、同步或响应字段变化。'
                : <>{`将${actionLabel}，可能触发发送、删除等后果；控件名称不代表安全或授权，请核对当前页面及后果。`}{operation === 'invoke'
                  ? '成功回执仅表示操作请求已发出，不代表目标已完成。'
                  : operation === 'scrollup' || operation === 'scrolldown'
                    ? '成功回执仅确认滚动状态，仍需检查任务结果，不代表目标已完成；请重新观察页面后再发起下一步操作。'
                  : '成功回执仅确认控件状态，仍需检查任务结果，不代表目标已完成。'}</>}
            {(operation === 'check' || operation === 'uncheck')
              && '已达到目标状态则不重复操作；最多切换一次，当前为半选状态或无法确认目标状态时停止，请先核对应用再决定下一步。'}
          </>
        ) : !operation ? '请求动作缺失、无效或已停用；不能允许本次操作。' : isImage
          ? '正在核对授权应用与窗口图像观察范围；核对完成前不能允许。' : !isTextWrite
          ? '正在核对目标窗口与待操作控件；核对完成前不能允许。'
          : '正在核对目标窗口与待填写内容；核对完成前不能允许。'}
        {preview && isVisualClick && preview.image && (
          <figure className="desktop-image-click-preview">
            <div className="desktop-image-click-frame">
              <img key={preview.previewId} src={preview.image.imageDataUrl} alt="本次待点击的授权窗口" draggable={false} width={preview.image.width} height={preview.image.height}
                onLoad={(event) => {
                  if (currentScope.current !== scopeKey) return;
                  if (event.currentTarget.naturalWidth !== preview.image!.width || event.currentTarget.naturalHeight !== preview.image!.height) {
                    setState({ status: 'unavailable' });
                    return;
                  }
                  setLoadedImageKey(JSON.stringify([scopeKey, preview.previewId]));
                }}
                onError={() => { if (currentScope.current === scopeKey) setState({ status: 'unavailable' }); }} />
              <span className="desktop-image-click-point" aria-label={`点击位置 ${preview.image.x}, ${preview.image.y}`}
                style={{ left: `${(preview.image.x + 0.5) / preview.image.width * 100}%`, top: `${(preview.image.y + 0.5) / preview.image.height * 100}%` }} />
            </div>
            <figcaption>原图坐标（{preview.image.x}, {preview.image.y}）；仅本次确认，不保存截图。{!imageReady && '图像尚未加载完成，暂不能批准。'}</figcaption>
          </figure>
        )}
        {preview && isClear ? (
          <span className={compact ? 'agent-run-text-warning' : 'content-block-tool-confirmation-warning'}>
            将删除这个输入框中的全部内容；不会读取或保留原内容，无法通过 AngelBot 自动恢复。目标应用可能立即保存或同步这次清空。
          </span>
        ) : preview && isTextWrite && (compact ? (
          <span className="agent-run-text-preview">
            <span>将填写的内容</span>
            <span>{preview.text}</span>
          </span>
        ) : (
          <div className="content-block-tool-confirmation-preview">
            <strong>将填写的内容</strong>
            <pre>{preview.text}</pre>
          </div>
        ))}
        {approvalBlocked ? <span role="status">{PENDING_ACTIONS_BLOCKED_MESSAGE}</span> : state.status === 'settled' ? <span role="status">本次决定已提交，请查看主对话中的执行结果；不会再次派发。</span> : state.status === 'uncertain' ? (
          <span className={compact ? 'agent-run-text-warning' : 'content-block-tool-confirmation-warning'} role="alert">
            无法确认请求是否已处理；请先检查目标应用与对话结果，勿直接重试本次批准。
          </span>
        ) : state.status === 'unavailable' || expired ? (
          <span className={compact ? 'agent-run-text-warning' : 'content-block-tool-confirmation-warning'} role="alert">
            {expired ? '目标预检已过期，请重新核对。'
              : state.status === 'unavailable' && state.reason === 'busy'
                ? '另一个电脑操作正在进行。请等待当前操作完成后，手动重新核对；本次操作不会自动重试。'
              : state.status === 'unavailable' && state.reason === 'changed'
                ? '目标窗口或控件已变化。请重新观察并核对当前页面后，手动重新核对；本次操作不会自动重试。'
              : !operation
              ? '请求动作缺失、无效或已停用。请拒绝本次操作，重新发起带有明确动作的请求。'
              : isImage
                ? '无法核对当前应用或窗口图像观察授权。请检查受信任应用是否启用、程序是否可用及观察范围是否仍有效；否则拒绝本次观察。'
              : isVisualClick
                ? '无法显示或核对待点击的窗口图像。请重新观察并核对；图像加载失败或尺寸不符时不能批准。'
              : operation === 'draft'
              ? '无法核对目标窗口或草稿内容。请检查受信任应用及输入框设置，并确保目标窗口已打开；否则拒绝本次操作。'
              : operation === 'field'
                ? '无法核对目标窗口或待填写内容。请重新观察受信任应用并确认输入框仍可用；否则拒绝本次操作。'
                : '无法核对目标窗口或待操作控件。请重新观察受信任应用并确认控件仍可用；否则拒绝本次操作。'}
            {operation && <button type="button" disabled={resolving} onClick={retryPreflight}>重新核对</button>}
          </span>
        ) : null}
      </div>
      <div className={actionsClass}>
        <button
          type="button"
          className={approveClass}
          disabled={approvalBlocked || !preview || !imageReady || resolving}
          onClick={() => { void resolve('approved'); }}
        >
          {isImage ? '允许发送图像一次' : isClear ? '允许清空一次' : actionLabel ? `允许${actionLabel}一次` : compact ? '允许一次' : '允许'}
        </button>
        <button type="button" className={rejectClass} disabled={resolving || state.status === 'settled'} onClick={() => { void resolve('rejected'); }}>
          拒绝
        </button>
      </div>
    </div>
  );
}
