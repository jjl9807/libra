"use client";

import { useCallback, useMemo, useRef, useState } from "react";

import { useBrowserController } from "@/lib/code-ui/controller";
import { executionRepairView } from "@/lib/code-ui/execution-repair";
import { browserInteractionRespondSupported } from "@/lib/code-ui/interactions";
import { useCodeUiStore } from "@/lib/code-ui/store";
import type { CodeUiInteractionResponse } from "@/lib/code-ui/types";

import { ExecutionRepairHost } from "./ExecutionRepairHost";

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Request failed. Try again.";
}

/**
 * Projects plan execution + repair state from the live snapshot.
 * Continue/Cancel use the shared controller respond path — no local repair FSM.
 */
export function SessionExecutionRepair() {
  const { snapshot } = useCodeUiStore();
  const controller = useBrowserController();
  const view = useMemo(() => executionRepairView(snapshot), [snapshot]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const busyRef = useRef(false);

  const canRespond = Boolean(
    snapshot && browserInteractionRespondSupported(snapshot.provider),
  );

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
    <ExecutionRepairHost
      view={view}
      busy={busy}
      error={error}
      canRespond={canRespond}
      onContinue={(interactionId, response: CodeUiInteractionResponse) =>
        void run(async () => {
          await controller.respond(interactionId, response);
        })
      }
      onCancelRepair={(interactionId, response: CodeUiInteractionResponse) =>
        void run(async () => {
          await controller.respond(interactionId, response);
        })
      }
    />
  );
}
