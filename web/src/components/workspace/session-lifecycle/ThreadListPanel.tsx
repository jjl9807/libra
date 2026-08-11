"use client";

import type { ThreadListItem } from "../../../lib/code-ui/session-lifecycle";

export interface ThreadListPanelProps {
  items: ThreadListItem[];
  selectedThreadId?: string;
  busy?: boolean;
  error?: string;
  loading?: boolean;
  hasMore?: boolean;
  onRefresh(): void | Promise<void>;
  onLoadMore(): void | Promise<void>;
  onSelect(threadId: string): void;
}

export function ThreadListPanel({
  items,
  selectedThreadId,
  busy = false,
  error,
  loading = false,
  hasMore = false,
  onRefresh,
  onLoadMore,
  onSelect,
}: ThreadListPanelProps) {
  return (
    <section aria-label="Thread list panel">
      <h2>Threads</h2>
      <p>
        Repository-shared list from storage. Process resume is working-directory
        scoped — use the original session cwd with `libra code --resume`.
      </p>
      <button type="button" disabled={busy || loading} onClick={() => void onRefresh()}>
        Refresh threads
      </button>
      {loading && items.length === 0 ? <p>Loading threads…</p> : null}
      {items.length === 0 && !loading ? <p>No threads found.</p> : null}
      <ul aria-label="Thread list">
        {items.map((item) => {
          const selected = item.id === selectedThreadId;
          return (
            <li key={item.id}>
              <button
                type="button"
                aria-pressed={selected}
                disabled={busy}
                onClick={() => onSelect(item.id)}
              >
                {item.title?.trim() || item.id}
                {item.archived ? " (archived)" : ""}
              </button>
              <span>
                Updated {item.updatedAt}
                {selected ? " · selected" : ""}
              </span>
            </li>
          );
        })}
      </ul>
      {hasMore ? (
        <button type="button" disabled={busy || loading} onClick={() => void onLoadMore()}>
          Load more threads
        </button>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
    </section>
  );
}
