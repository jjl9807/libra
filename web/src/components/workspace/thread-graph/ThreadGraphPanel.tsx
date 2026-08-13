import type { ThreadGraphView } from "@/lib/code-ui/thread-graph";

export function ThreadGraphPanel({ view }: { view: ThreadGraphView }) {
  return (
    <section aria-label="Thread version graph">
      <h2>Thread version graph</h2>
      {view.title ? <p>{view.title}</p> : null}
      {view.threadId ? <p>Thread {view.threadId}</p> : null}
      {view.loadError ? <p>{view.loadError}</p> : null}
      {view.truncatedReason ? <p>{view.truncatedReason}</p> : null}
      {view.emptyReason ? <p>{view.emptyReason}</p> : null}
      <ol>
        {view.nodes.map((node) => (
          <li key={`${node.kind}:${node.id}`} style={{ marginLeft: `${node.depth * 1.25}rem` }}>
            <strong>{node.kind}</strong> {node.label}
            {node.tags.length > 0 ? <span> ({node.tags.join(", ")})</span> : null}
          </li>
        ))}
      </ol>
    </section>
  );
}
