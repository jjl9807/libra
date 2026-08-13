import type { ThreadGraphView } from "@/lib/code-ui/thread-graph";

import { ThreadGraphPanel } from "./ThreadGraphPanel";

export function ThreadGraphHost({ view }: { view: ThreadGraphView }) {
  return <ThreadGraphPanel view={view} />;
}
