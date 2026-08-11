import type { CodeUiClient, CodeUiEventStream } from "../client";

type DisconnectListener = () => void;

const disconnectListeners = new Set<DisconnectListener>();

/** Subscribe to SSE disconnects observed through a wrapped client. */
export function subscribeSseDisconnect(listener: DisconnectListener): () => void {
  disconnectListeners.add(listener);
  return () => {
    disconnectListeners.delete(listener);
  };
}

/**
 * Wrap a Code UI client so the store's EventSource error path also notifies
 * W2-15 listeners — without modifying W2-07 `store.tsx`.
 */
export function wrapClientForSseResilience(client: CodeUiClient): CodeUiClient {
  return {
    ...client,
    observe(onEvent, onError): CodeUiEventStream {
      return client.observe(onEvent, () => {
        disconnectListeners.forEach((listener) => listener());
        onError();
      });
    },
  };
}
