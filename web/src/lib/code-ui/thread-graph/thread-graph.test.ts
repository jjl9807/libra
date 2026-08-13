import { describe, expect, it } from "vitest";

import { sessionFixture } from "../fixtures";

import {
  INDEXED_THREAD_GRAPH_LOAD_FAILED,
  INDEXED_THREAD_GRAPH_UNAVAILABLE,
  threadGraphCoversSnapshotHeads,
  threadGraphView,
} from "./view";

describe("threadGraphView", () => {
  it("renders snapshot threadGraph nodes with depth and tags", () => {
    const view = threadGraphView(
      sessionFixture({
        threadId: "thread-1",
        threadGraph: {
          threadId: "thread-1",
          title: "Demo thread",
          selectedPlanId: "plan-1",
          activeTaskId: "task-1",
          nodes: [
            { depth: 0, kind: "intent", id: "intent-1", label: "Intent 1", tags: ["current"] },
            { depth: 1, kind: "plan", id: "plan-1", label: "Plan 1", tags: ["selected"] },
            { depth: 2, kind: "task", id: "task-1", label: "Active task", tags: ["active"] },
          ],
        },
      }),
    );
    expect(view.title).toBe("Demo thread");
    expect(view.nodes).toHaveLength(3);
    expect(view.nodes[0]).toMatchObject({ kind: "intent", depth: 0, tags: ["current"] });
    expect(view.emptyReason).toBeUndefined();
    expect(view.truncatedReason).toBeUndefined();
  });

  it("explains truncation when the indexed graph is capped", () => {
    const view = threadGraphView(
      sessionFixture({
        threadId: "thread-1",
        threadGraph: {
          threadId: "thread-1",
          nodes: [{ depth: 2, kind: "task", id: "task-active", label: "Active task", tags: ["active"] }],
          truncated: true,
          omittedNodeCount: 12,
          totalNodeCount: 268,
        },
      }),
    );
    expect(view.truncatedReason).toMatch(/Showing 1 of 268/);
    expect(view.truncatedReason).toMatch(/Active heads are preserved/);
  });

  it("falls back to plans/tasks when threadGraph is absent", () => {
    const view = threadGraphView(
      sessionFixture({
        threadId: "thread-2",
        plans: [
          {
            id: "plan-a",
            title: "Execution",
            status: "selected",
            steps: [],
            updatedAt: "2026-07-15T00:00:00.000Z",
          },
        ],
      }),
    );
    expect(view.nodes).toEqual([
      expect.objectContaining({ kind: "plan", id: "plan-a", label: "Execution" }),
    ]);
    expect(view.emptyReason).toBe(INDEXED_THREAD_GRAPH_UNAVAILABLE);
  });

  it("surfaces loader failures separately from a missing indexed graph", () => {
    const view = threadGraphView(
      sessionFixture({
        threadId: "thread-2",
        plans: [
          {
            id: "plan-a",
            title: "Execution",
            status: "selected",
            steps: [],
            updatedAt: "2026-07-15T00:00:00.000Z",
          },
        ],
      }),
      { loadError: `${INDEXED_THREAD_GRAPH_LOAD_FAILED} (THREAD_GRAPH_UNAVAILABLE)` },
    );
    expect(view.nodes).toEqual([
      expect.objectContaining({ kind: "plan", id: "plan-a", label: "Execution" }),
    ]);
    expect(view.loadError).toContain("THREAD_GRAPH_UNAVAILABLE");
    expect(view.emptyReason).toBeUndefined();
  });

  it("covers live snapshot heads and ignores truncated historical plans", () => {
    const graph = {
      threadId: "thread-1",
      selectedPlanId: "plan-1",
      activeTaskId: "task-1",
      nodes: [
        { depth: 1, kind: "plan", id: "plan-1", label: "Plan 1" },
        { depth: 2, kind: "task", id: "task-1", label: "Task 1" },
        { depth: 4, kind: "patchset", id: "patch-1", label: "PatchSet 1" },
      ],
      truncated: true,
      omittedNodeCount: 12,
      totalNodeCount: 268,
    };
    expect(
      threadGraphCoversSnapshotHeads(
        sessionFixture({
          threadId: "thread-1",
          plans: [
            {
              id: "plan-1",
              status: "selected",
              steps: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
            {
              id: "plan-historical",
              status: "completed",
              steps: [],
              updatedAt: "2026-07-14T00:00:00.000Z",
            },
          ],
          tasks: [
            {
              id: "task-1",
              status: "active",
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          patchsets: [
            {
              id: "patch-1",
              status: "ready",
              changes: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          threadGraph: graph,
        }),
      ),
    ).toBe(true);
    expect(
      threadGraphCoversSnapshotHeads(
        sessionFixture({
          threadId: "thread-1",
          plans: [
            {
              id: "plan-1",
              status: "selected",
              steps: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          tasks: [
            {
              id: "task-1",
              status: "active",
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          patchsets: [
            {
              id: "patch-1",
              status: "ready",
              changes: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          threadGraph: {
            threadId: "thread-1",
            nodes: [
              { depth: 1, kind: "plan", id: "plan-1", label: "Plan 1", tags: ["selected"] },
              { depth: 2, kind: "task", id: "task-1", label: "Task 1", tags: ["active"] },
              { depth: 4, kind: "patchset", id: "patch-1", label: "PatchSet 1" },
            ],
          },
        }),
      ),
    ).toBe(true);

    expect(
      threadGraphCoversSnapshotHeads(
        sessionFixture({
          threadId: "thread-1",
          plans: [
            {
              id: "plan-2",
              status: "selected",
              steps: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          threadGraph: graph,
        }),
      ),
    ).toBe(false);
    expect(
      threadGraphCoversSnapshotHeads(
        sessionFixture({
          threadId: "thread-1",
          plans: [
            {
              id: "plan-1",
              status: "completed",
              steps: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
            {
              id: "plan-2",
              status: "selected",
              steps: [],
              updatedAt: "2026-07-15T00:00:01.000Z",
            },
          ],
          tasks: [
            {
              id: "task-1",
              status: "active",
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          patchsets: [
            {
              id: "patch-1",
              status: "ready",
              changes: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
          threadGraph: {
            ...graph,
            nodes: [
              ...graph.nodes,
              { depth: 1, kind: "plan", id: "plan-2", label: "Plan 2" },
            ],
          },
        }),
      ),
    ).toBe(false);
  });

  it("explains an empty session without a thread", () => {
    const view = threadGraphView(sessionFixture());
    expect(view.nodes).toEqual([]);
    expect(view.emptyReason).toMatch(/no thread/i);
  });
});
