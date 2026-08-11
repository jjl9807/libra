"use client";

import { useCallback, useMemo, useRef, useState } from "react";

import { useBrowserController } from "@/lib/code-ui/controller";
import { browserInteractionRespondSupported } from "@/lib/code-ui/interactions";
import { useCodeUiStore } from "@/lib/code-ui/store";
import type { CodeUiInteractionResponse } from "@/lib/code-ui/types";
import { workflowView } from "@/lib/code-ui/workflow";

import { WorkflowHost } from "./WorkflowHost";

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Request failed. Try again.";
}

/**
 * Snapshot-driven IntentSpec / Plan / network-policy gates.
 * No local workflow FSM — wait for SSE/snapshot after respond/cancel.
 */
export function SessionWorkflow() {
  const { snapshot } = useCodeUiStore();
  const controller = useBrowserController();
  const view = useMemo(() => workflowView(snapshot), [snapshot]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const busyRef = useRef(false);

  const canRespond = Boolean(
    snapshot && browserInteractionRespondSupported(snapshot.provider),
  );
  // Cancel turn fail-closed when the projected controller cannot write.
  const canCancel = Boolean(snapshot?.controller.canWrite);

  const run = useCallback(async (operation: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(undefined);
    try {
      await operation();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  return (
    <WorkflowHost
      view={view}
      respondEnabled={canRespond}
      cancelEnabled={canCancel}
      busy={busy}
      error={error}
      onRespond={(interactionId, response: CodeUiInteractionResponse) =>
        void run(async () => {
          await controller.respond(interactionId, response);
        })
      }
      onCancelTurn={() =>
        void run(async () => {
          await controller.cancel();
        })
      }
    />
  );
}
