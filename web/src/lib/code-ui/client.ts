import type {
  CodeUiAckResponse,
  CodeUiApiError,
  CodeUiControllerAttachResponse,
  CodeUiEventEnvelope,
  CodeUiInteractionResponse,
  CodeUiSessionSnapshot,
} from "./types";

const MAX_MESSAGE_BODY_BYTES = 256 * 1024;

export interface CodeUiEventStream {
  close(): void;
}

export interface CodeUiTransport {
  request<T>(path: string, init?: RequestInit): Promise<T>;
  events(
    onEvent: (event: CodeUiEventEnvelope) => void,
    onError: () => void,
  ): CodeUiEventStream;
}

export interface CodeUiClient {
  snapshot(): Promise<CodeUiSessionSnapshot>;
  attach(clientId: string): Promise<CodeUiControllerAttachResponse>;
  detach(clientId: string, token: string, keepalive?: boolean): Promise<void>;
  submit(text: string, token: string, commandId?: string): Promise<CodeUiAckResponse>;
  respond(
    interactionId: string,
    response: CodeUiInteractionResponse,
    token: string,
  ): Promise<CodeUiAckResponse>;
  cancel(token: string): Promise<CodeUiAckResponse>;
  observe(
    onEvent: (event: CodeUiEventEnvelope) => void,
    onError: () => void,
  ): CodeUiEventStream;
}

function headers(token?: string): HeadersInit {
  return {
    "content-type": "application/json",
    ...(token ? { "X-Code-Controller-Token": token } : {}),
  };
}

/** Named SSE event types emitted by the Rust `/api/code/events` stream. */
export const CODE_UI_SSE_EVENT_TYPES = [
  "session_updated",
  "status_changed",
  "controller_changed",
] as const;

export class FetchCodeUiTransport implements CodeUiTransport {
  /**
   * @param baseUrl Optional origin for loopback/integration tests. Production
   *   browser loads leave this empty so paths stay same-origin relative.
   */
  constructor(private readonly baseUrl = "") {}

  private url(path: string): string {
    return `${this.baseUrl}${path}`;
  }

  async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(this.url(path), { credentials: "same-origin", ...init });
    if (!response.ok) {
      let message = response.statusText;
      let code: string | undefined;
      try {
        const body = (await response.json()) as { error?: Partial<CodeUiApiError> };
        if (typeof body.error?.message === "string") message = body.error.message;
        if (typeof body.error?.code === "string") code = body.error.code;
      } catch {
        // The health endpoint and intermediary failures need not be JSON.
      }
      throw { status: response.status, message, code } satisfies CodeUiApiError;
    }
    return (await response.json()) as T;
  }

  events(onEvent: (event: CodeUiEventEnvelope) => void, onError: () => void): CodeUiEventStream {
    const source = new EventSource(this.url("/api/code/events"), { withCredentials: true });
    source.onmessage = (message) => onEvent(JSON.parse(message.data) as CodeUiEventEnvelope);
    for (const type of CODE_UI_SSE_EVENT_TYPES) {
      source.addEventListener(type, (message) =>
        onEvent(JSON.parse((message as MessageEvent<string>).data) as CodeUiEventEnvelope),
      );
    }
    source.onerror = onError;
    return { close: () => source.close() };
  }
}

export function createCodeUiClient(
  transport: CodeUiTransport = new FetchCodeUiTransport(),
): CodeUiClient {
  return {
    snapshot: () => transport.request("/api/code/session"),
    attach: (clientId) =>
      transport.request("/api/code/controller/attach", {
        method: "POST",
        headers: headers(),
        body: JSON.stringify({ clientId, kind: "browser" }),
      }),
    detach: async (clientId, token, keepalive = false) => {
      await transport.request("/api/code/controller/detach", {
        method: "POST",
        headers: headers(token),
        body: JSON.stringify({ clientId }),
        keepalive,
      });
    },
    submit: async (text, token, commandId) => {
      const body = JSON.stringify({ text, ...(commandId ? { commandId } : {}) });
      if (new TextEncoder().encode(body).byteLength > MAX_MESSAGE_BODY_BYTES) {
        throw {
          status: 413,
          code: "PAYLOAD_TOO_LARGE",
          message: "Message is too large. Reduce it to 256 KiB or less before submitting.",
        } satisfies CodeUiApiError;
      }
      return transport.request("/api/code/messages", {
        method: "POST",
        headers: headers(token),
        body,
      });
    },
    respond: (interactionId, response, token) =>
      transport.request(`/api/code/interactions/${encodeURIComponent(interactionId)}`, {
        method: "POST",
        headers: headers(token),
        body: JSON.stringify(response),
      }),
    cancel: (token) =>
      transport.request("/api/code/control/cancel", {
        method: "POST",
        headers: headers(token),
      }),
    observe: transport.events.bind(transport),
  };
}

export class MockCodeUiClient implements CodeUiClient {
  public readonly calls: Array<{ name: string; args: unknown[] }> = [];
  private listeners = new Set<(event: CodeUiEventEnvelope) => void>();

  constructor(
    public currentSnapshot: CodeUiSessionSnapshot,
    public controllerToken = "mock-controller-token",
    public leaseExpiresAt = new Date(Date.now() + 120_000).toISOString(),
  ) {}

  snapshot = async () => this.currentSnapshot;
  attach = async (clientId: string) => {
    this.calls.push({ name: "attach", args: [clientId] });
    return {
      controllerToken: this.controllerToken,
      leaseExpiresAt: this.leaseExpiresAt,
      controller: this.currentSnapshot.controller,
    };
  };
  detach = async (clientId: string, token: string, keepalive = false) => {
    this.calls.push({ name: "detach", args: [clientId, token, keepalive] });
  };
  submit = async (text: string, token: string, commandId?: string) => {
    this.calls.push({ name: "submit", args: [text, token, commandId] });
    return { accepted: true };
  };
  respond = async (id: string, response: CodeUiInteractionResponse, token: string) => {
    this.calls.push({ name: "respond", args: [id, response, token] });
    return { accepted: true };
  };
  cancel = async (token: string) => {
    this.calls.push({ name: "cancel", args: [token] });
    return { accepted: true };
  };
  observe = (onEvent: (event: CodeUiEventEnvelope) => void, _onError: () => void) => {
    void _onError;
    this.listeners.add(onEvent);
    return { close: () => this.listeners.delete(onEvent) };
  };
  emit(event: CodeUiEventEnvelope): void {
    this.currentSnapshot = event.data;
    this.listeners.forEach((listener) => listener(event));
  }
}
