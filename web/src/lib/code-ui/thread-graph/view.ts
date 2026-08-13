import type { CodeUiSessionSnapshot, CodeUiThreadGraph } from "../types";

export interface ThreadGraphView {
  threadId?: string;
  title?: string;
  selectedPlanId?: string;
  activeTaskId?: string;
  activeRunId?: string;
  nodes: Array<{
    depth: number;
    kind: string;
    id: string;
    label: string;
    tags: string[];
  }>;
  emptyReason?: string;
  truncatedReason?: string;
  loadError?: string;
}

export const INDEXED_THREAD_GRAPH_UNAVAILABLE =
  "Indexed thread version graph is unavailable; showing live plan/task/patchset heads only.";

export const INDEXED_THREAD_GRAPH_LOAD_FAILED =
  "Indexed thread version graph failed to load; showing live plan/task/patchset heads only.";

export function threadGraphCoversSnapshotHeads(
  snapshot?: CodeUiSessionSnapshot,
): snapshot is CodeUiSessionSnapshot & { threadGraph: CodeUiThreadGraph } {
  const graph = snapshot?.threadGraph;
  if (!snapshot || !graph) return false;
  if (!snapshot.threadId || graph.threadId !== snapshot.threadId) return false;

  const hasNode = (kind: string, id: string) =>
    graph.nodes.some((node) => node.kind === kind && node.id === id);
  const taggedId = (kind: string, tags: string[]) =>
    graph.nodes.find((node) => node.kind === kind && node.tags?.some((tag) => tags.includes(tag)))
      ?.id;
  const selectedPlanId =
    graph.selectedPlanId ?? taggedId("plan", ["selected", "running"]);
  const activeTaskId = graph.activeTaskId ?? taggedId("task", ["active", "running"]);

  if (selectedPlanId && !hasNode("plan", selectedPlanId)) return false;
  if (activeTaskId && !hasNode("task", activeTaskId)) return false;
  if (graph.activeRunId && !hasNode("run", graph.activeRunId)) return false;

  const livePlanIds = snapshot.plans
    .filter((plan) => plan.status === "selected" || plan.status === "running")
    .map((plan) => plan.id);
  if (livePlanIds.length > 0 && (!selectedPlanId || !livePlanIds.includes(selectedPlanId))) {
    return false;
  }
  const liveTaskIds = snapshot.tasks
    .filter((task) => task.status === "active" || task.status === "running")
    .map((task) => task.id);
  if (liveTaskIds.length > 0 && (!activeTaskId || !liveTaskIds.includes(activeTaskId))) {
    return false;
  }

  const plans = graph.truncated
    ? snapshot.plans.filter(
        (plan) =>
          plan.status === "selected" ||
          plan.status === "running" ||
          graph.selectedPlanId === plan.id,
      )
    : snapshot.plans;
  const tasks = graph.truncated
    ? snapshot.tasks.filter(
        (task) =>
          task.status === "active" ||
          task.status === "running" ||
          graph.activeTaskId === task.id,
      )
    : snapshot.tasks;
  const patchsets = graph.truncated
    ? snapshot.patchsets.slice(-1)
    : snapshot.patchsets;

  return (
    plans.every((plan) => hasNode("plan", plan.id)) &&
    tasks.every((task) => hasNode("task", task.id)) &&
    patchsets.every((patchset) => hasNode("patchset", patchset.id))
  );
}

export function threadGraphTruncationReason(graph: CodeUiThreadGraph): string | undefined {
  if (!graph.truncated) return undefined;
  const shown = graph.nodes.length;
  const total = graph.totalNodeCount ?? shown + (graph.omittedNodeCount ?? 0);
  return `Showing ${shown} of ${total} version-graph nodes; older lineage omitted. Active heads are preserved.`;
}

export function threadGraphView(
  snapshot?: CodeUiSessionSnapshot,
  options?: { loadError?: string },
): ThreadGraphView {
  const graph = snapshot?.threadGraph;
  if (!graph) {
    if (!snapshot?.threadId) {
      return { nodes: [], emptyReason: "No thread is attached to this session yet." };
    }
    const nodes = fallbackNodes(snapshot);
    return {
      threadId: snapshot.threadId,
      nodes,
      loadError: options?.loadError,
      emptyReason: options?.loadError
        ? undefined
        : nodes.length === 0
          ? "This thread has no Intent/Plan/Task/Run graph yet."
          : INDEXED_THREAD_GRAPH_UNAVAILABLE,
    };
  }
  return viewFromGraph(graph);
}

function viewFromGraph(graph: CodeUiThreadGraph): ThreadGraphView {
  return {
    threadId: graph.threadId,
    title: graph.title,
    selectedPlanId: graph.selectedPlanId,
    activeTaskId: graph.activeTaskId,
    activeRunId: graph.activeRunId,
    nodes: graph.nodes.map((node) => ({
      depth: node.depth,
      kind: node.kind,
      id: node.id,
      label: node.label,
      tags: node.tags ?? [],
    })),
    truncatedReason: threadGraphTruncationReason(graph),
    emptyReason:
      graph.nodes.length === 0 ? "This thread has no Intent/Plan/Task/Run graph yet." : undefined,
  };
}

function fallbackNodes(snapshot: CodeUiSessionSnapshot): ThreadGraphView["nodes"] {
  const nodes: ThreadGraphView["nodes"] = [];
  for (const plan of snapshot.plans) {
    nodes.push({
      depth: 1,
      kind: "plan",
      id: plan.id,
      label: plan.title ?? plan.summary ?? `Plan ${plan.id}`,
      tags: plan.status ? [plan.status] : [],
    });
  }
  for (const task of snapshot.tasks) {
    nodes.push({
      depth: 2,
      kind: "task",
      id: task.id,
      label: task.title ?? `Task ${task.id}`,
      tags: task.status ? [task.status] : [],
    });
  }
  for (const patchset of snapshot.patchsets) {
    nodes.push({
      depth: 4,
      kind: "patchset",
      id: patchset.id,
      label: `PatchSet ${patchset.id}`,
      tags: patchset.status ? [patchset.status] : [],
    });
  }
  return nodes;
}
