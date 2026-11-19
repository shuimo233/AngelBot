import { Fragment, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { WorkDirEntry } from '$lib/commands/file';

const PAGE_SIZE = 80;

export function pathForChildLevel(path: string | undefined, parentPath = '') {
  if (!path) return undefined;
  const prefix = parentPath ? `${parentPath}/` : '';
  return path.startsWith(prefix) ? prefix + path.slice(prefix.length).split('/', 1)[0] : undefined;
}

export function FileTreePage({ entries, requiredEntryPath, renderEntry }: {
  entries: WorkDirEntry[];
  requiredEntryPath?: string;
  renderEntry: (entry: WorkDirEntry) => ReactNode;
}) {
  const [limit, setLimit] = useState(PAGE_SIZE);
  const moreRow = useRef<HTMLLIElement>(null);
  const focusAfterMore = useRef<Element | null>(null);
  const requiredIndex = requiredEntryPath ? entries.findIndex(({ path }) => path === requiredEntryPath) : -1;
  const shown = entries.slice(0, limit);
  if (requiredIndex >= limit) shown.push(entries[requiredIndex]);

  useLayoutEffect(() => {
    const previousRow = focusAfterMore.current;
    focusAfterMore.current = null;
    previousRow?.nextElementSibling?.querySelector<HTMLButtonElement>('button')?.focus();
  }, [limit]);

  return <>
    {shown.map((entry) => <Fragment key={entry.path}>{renderEntry(entry)}</Fragment>)}
    {shown.length < entries.length && <li className="file-tree-more" ref={moreRow}>
      <button type="button" onClick={(event) => {
        if (event.detail === 0 || entries.length <= limit + PAGE_SIZE) {
          const lastRegularRow = requiredIndex >= limit
            ? moreRow.current?.previousElementSibling?.previousElementSibling
            : moreRow.current?.previousElementSibling;
          focusAfterMore.current = lastRegularRow ?? null;
        }
        setLimit((count) => count + PAGE_SIZE);
      }}>显示更多</button>
    </li>}
  </>;
}

export function matchingLoadedFiles(root: WorkDirEntry[], directories: ReadonlyMap<string, WorkDirEntry[]>, query: string) {
  const normalized = query.trim().toLocaleLowerCase().replace(/\\/g, '/');
  if (!normalized) return [];
  const results = new Map<string, WorkDirEntry>();
  for (const entries of [root, ...directories.values()]) {
    for (const entry of entries) {
      if (!entry.isDirectory && (entry.path.toLocaleLowerCase().includes(normalized)
        || entry.name.toLocaleLowerCase().includes(normalized))) results.set(entry.path, entry);
    }
  }
  return [...results.values()].sort((left, right) => left.path.localeCompare(right.path));
}
