"use client";

import { useBrowserController } from "@/lib/code-ui/controller";
import { browserInteractionRespondSupported } from "@/lib/code-ui/interactions";
import { useCodeUiStore } from "@/lib/code-ui/store";

import { InteractionsHost } from "./InteractionsHost";

/**
 * Wires the pending snapshot interaction to the browser controller lease.
 * Lives in the W2-08 ownership tree so the foundation page only mounts it.
 */
export function SessionInteractions() {
  const { snapshot } = useCodeUiStore();
  const controller = useBrowserController();
  const pending = snapshot?.interactions.find((interaction) => interaction.status === "pending");
  const respondEnabled = snapshot
    ? browserInteractionRespondSupported(snapshot.provider)
    : true;

  return (
    <InteractionsHost
      interaction={pending}
      respondEnabled={respondEnabled}
      onRespond={(interactionId, response) => controller.respond(interactionId, response)}
      onCancel={() => controller.cancel()}
    />
  );
}
