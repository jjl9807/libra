"use client";

import type { ResumeAffordance, ThreadListItem } from "../../../lib/code-ui/session-lifecycle";

import { ResumeCancelPanel } from "./ResumeCancelPanel";
import { ThreadListPanel } from "./ThreadListPanel";

export interface SessionLifecycleHostProps {
  items: ThreadListItem[];
  selectedThreadId?: string;
  currentThreadId?: string;
  phaseLabel: string;
  affordance: ResumeAffordance;
  selectionError?: string;
  listError?: string;
  cancelError?: string;
  resumeHint?: string;
  busy?: boolean;
  loading?: boolean;
  hasMore?: boolean;
  canCancel?: boolean;
  onRefreshThreads(): void | Promise<void>;
  onLoadMoreThreads(): void | Promise<void>;
  onSelectThread(threadId: string): void;
  onCancelTurn(): void | Promise<void>;
  onResumeIntent(): void | Promise<void>;
}

export function SessionLifecycleHost({
  items,
  selectedThreadId,
  currentThreadId,
  phaseLabel,
  affordance,
  selectionError,
  listError,
  cancelError,
  resumeHint,
  busy,
  loading,
  hasMore,
  canCancel,
  onRefreshThreads,
  onLoadMoreThreads,
  onSelectThread,
  onCancelTurn,
  onResumeIntent,
}: SessionLifecycleHostProps) {
  return (
    <div aria-label="Session lifecycle workspace">
      <ThreadListPanel
        items={items}
        selectedThreadId={selectedThreadId}
        busy={busy}
        error={listError}
        loading={loading}
        hasMore={hasMore}
        onRefresh={onRefreshThreads}
        onLoadMore={onLoadMoreThreads}
        onSelect={onSelectThread}
      />
      <ResumeCancelPanel
        currentThreadId={currentThreadId}
        selectedThreadId={selectedThreadId}
        phaseLabel={phaseLabel}
        affordance={affordance}
        selectionError={selectionError}
        cancelError={cancelError}
        resumeHint={resumeHint}
        busy={busy}
        canCancel={canCancel}
        onCancel={onCancelTurn}
        onResumeIntent={onResumeIntent}
      />
    </div>
  );
}

export { ResumeCancelPanel } from "./ResumeCancelPanel";
export { ThreadListPanel } from "./ThreadListPanel";
