import ReactMarkdown, { defaultUrlTransform } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { vscDarkPlus } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { ReactNode } from 'react';

/** Local citations grant no host access; the active session resolves the path. */
export function workspaceFilePathFromHref(href?: string): string | null {
  if (!href) return null;
  try {
    const parsed = new URL(href);
    if (parsed.protocol !== 'angelbot-file:' || parsed.host || parsed.search || parsed.hash
      || parsed.pathname.startsWith('/')) return null;
    const path = decodeURIComponent(parsed.pathname);
    if (!path || /[\\:\x00-\x1f\x7f]/.test(path)) return null;
    if (path.split('/').some((segment) => !segment || segment === '.' || segment === '..')) return null;
    return path;
  } catch {
    return null;
  }
}

export function workspaceFilePath(value: unknown): string | null {
  if (typeof value !== 'string' || !value.trim()) return null;
  try {
    const encoded = value.replace(/\\/g, '/').split('/').map(encodeURIComponent).join('/');
    return workspaceFilePathFromHref(`angelbot-file:${encoded}`);
  } catch {
    return null;
  }
}

export function locateWorkspaceFile(path: string) {
  window.dispatchEvent(new CustomEvent('angelbot:locate-file', { detail: { path } }));
}

export function WorkspaceFileLink({ path, children }: { path: string; children: ReactNode }) {
  return <button type="button" className="message-file-link"
    title={`在工作台中定位 ${path}`} aria-label={`在工作台中定位 ${path}`}
    onClick={() => locateWorkspaceFile(path)}>{children}</button>;
}

/** One rendering contract for both live segments and persisted messages. */
export function MessageMarkdown({ content, filePreview = false, deliveredFiles }: {
  content: string; filePreview?: boolean; deliveredFiles?: readonly { path: string }[];
}) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      urlTransform={(url) => workspaceFilePathFromHref(url) ? url : defaultUrlTransform(url)}
      components={{
        code({ className, children, ...props }) {
          const match = /language-(\w+)/.exec(className || '');
          const code = String(children).replace(/\n$/, '');
          return match && !filePreview ? (
            <SyntaxHighlighter style={vscDarkPlus} language={match[1]} PreTag="div">{code}</SyntaxHighlighter>
          ) : <code className={className} {...props}>{children}</code>;
        },
        a({ href, children, ...props }) {
          const path = workspaceFilePathFromHref(href);
          if (!path) return filePreview ? <span>{children}</span> : <a href={href} {...props}>{children}</a>;
          if (deliveredFiles?.some(file => file.path === path)) return <span>{children}</span>;
          return <WorkspaceFileLink path={path}>{children}</WorkspaceFileLink>;
        },
        ...(filePreview && {
          // Opening a local document preview must not fetch embedded resources.
          img({ alt }) { return <span>{alt ? `图片：${alt}` : '图片'}</span>; },
          table({ children }) {
            return <div className="file-preview-table" role="region" aria-label="表格" tabIndex={0}><table>{children}</table></div>;
          },
        }),
      }}
    >
      {content}
    </ReactMarkdown>
  );
}
