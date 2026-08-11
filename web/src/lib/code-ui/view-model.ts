import { phaseForSnapshot, type CodeUiPhase } from "./phases";
import type { CodeUiSessionSnapshot } from "./types";

export interface CodeUiShellViewModel {
  phase: CodeUiPhase;
  title: string;
  canWrite: boolean;
  pendingInteractionCount: number;
  isStreaming: boolean;
}

export function toShellViewModel(snapshot?: CodeUiSessionSnapshot): CodeUiShellViewModel {
  const phase = phaseForSnapshot(snapshot);
  return {
    phase,
    title: snapshot?.provider.model ?? snapshot?.provider.provider ?? "Libra Code",
    canWrite: Boolean(snapshot?.controller.canWrite && snapshot.capabilities.messageInput),
    pendingInteractionCount:
      snapshot?.interactions.filter((interaction) => interaction.status === "pending").length ?? 0,
    isStreaming: Boolean(snapshot?.transcript.some((entry) => entry.streaming)),
  };
}
