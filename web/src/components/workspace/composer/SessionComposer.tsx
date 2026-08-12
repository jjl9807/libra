"use client";

import { FormEvent, useCallback, useRef, useState } from "react";

import { useBrowserController } from "@/lib/code-ui/controller";
import { useCodeUiStore } from "@/lib/code-ui/store";

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Failed to submit message. Try again.";
}

function newCommandId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `cmd-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/**
 * Capability-gated message composer for `--web-only` sessions.
 * First submit attaches the browser controller via withLease; do not gate on
 * projected canWrite (same pattern as cancel).
 */
export function SessionComposer() {
  const { snapshot } = useCodeUiStore();
  const controller = useBrowserController();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const busyRef = useRef(false);
  /** Retained across retries when commandIdempotency is advertised. */
  const commandIdRef = useRef<string | undefined>(undefined);

  const messageInputEnabled = Boolean(snapshot?.capabilities.messageInput);
  const commandIdempotency = Boolean(snapshot?.capabilities.commandIdempotency);
  const canSubmit = Boolean(snapshot && messageInputEnabled && !busy);
  const transcript = snapshot?.transcript ?? [];

  const onSubmit = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmed = text.trim();
      if (!canSubmit || !trimmed || busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      setError(undefined);
      if (commandIdempotency && !commandIdRef.current) {
        commandIdRef.current = newCommandId();
      }
      const commandId = commandIdempotency ? commandIdRef.current : undefined;
      try {
        await controller.submit(trimmed, commandId);
        commandIdRef.current = undefined;
        setText("");
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    },
    [canSubmit, commandIdempotency, controller, text],
  );

  if (!snapshot) {
    return null;
  }

  return (
    <section aria-label="Message composer" style={{ marginTop: "1.5rem" }}>
      {transcript.length > 0 ? (
        <ol
          aria-label="Transcript"
          style={{ listStyle: "none", padding: 0, margin: "0 0 1rem", maxHeight: 240, overflow: "auto" }}
        >
          {transcript.map((entry) => (
            <li key={entry.id} style={{ marginBottom: "0.5rem" }}>
              <strong>{entry.kind}</strong>
              {entry.title ? `: ${entry.title}` : null}
              {entry.content ? (
                <div style={{ whiteSpace: "pre-wrap" }}>{entry.content}</div>
              ) : null}
            </li>
          ))}
        </ol>
      ) : null}
      {messageInputEnabled ? (
        <form onSubmit={onSubmit}>
          <label style={{ display: "block", marginBottom: "0.5rem" }}>
            Message
            <textarea
              aria-label="Session message"
              value={text}
              onChange={(event) => {
                commandIdRef.current = undefined;
                setText(event.target.value);
              }}
              rows={3}
              disabled={busy}
              style={{ display: "block", width: "100%", marginTop: "0.25rem" }}
            />
          </label>
          <button type="submit" disabled={!canSubmit || text.trim().length === 0}>
            {busy ? "Sending…" : "Send"}
          </button>
          {error ? (
            <p role="alert" style={{ color: "crimson" }}>
              {error}
            </p>
          ) : null}
        </form>
      ) : (
        <p>Message input is not available for this session.</p>
      )}
    </section>
  );
}
