import type { CodeUiApiError } from "../types";

import type { UsageReadModel } from "./types";

export interface UsageTransport {
  request<T>(path: string, init?: RequestInit): Promise<T>;
}

export interface UsageQueryScope {
  sessionId?: string;
  /** Repository-scoped thread filter — distinct from runtime turnId. */
  threadId?: string;
  /** Runtime turn id for current-turn delta — never confuse with threadId. */
  turnId?: string;
}

export class FetchUsageTransport implements UsageTransport {
  constructor(private readonly baseUrl = "") {}

  async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      credentials: "same-origin",
      ...init,
    });
    if (!response.ok) {
      let message = response.statusText;
      let code: string | undefined;
      try {
        const body = (await response.json()) as { error?: Partial<CodeUiApiError> };
        if (typeof body.error?.message === "string") message = body.error.message;
        if (typeof body.error?.code === "string") code = body.error.code;
      } catch {
        // Non-JSON failures still surface statusText.
      }
      throw { status: response.status, message, code } satisfies CodeUiApiError;
    }
    return (await response.json()) as T;
  }
}

/**
 * Usage query HTTP is W3-01. The path is reserved so fixture/injected transports
 * and the future wire share one client surface.
 */
export function createUsageApi(transport: UsageTransport = new FetchUsageTransport()) {
  return {
    /**
     * Observe usage for the active session. Production returns 404 until W3-01
     * registers the route; SessionUsage treats that as empty state.
     */
    fetchReadModel(scope: UsageQueryScope = {}): Promise<UsageReadModel> {
      const params = new URLSearchParams();
      if (scope.sessionId) params.set("sessionId", scope.sessionId);
      if (scope.threadId) params.set("threadId", scope.threadId);
      if (scope.turnId) params.set("turnId", scope.turnId);
      const query = params.toString();
      return transport.request(`/api/code/usage${query ? `?${query}` : ""}`);
    },
  };
}

export type UsageApi = ReturnType<typeof createUsageApi>;

export function isAbsentUsageError(cause: unknown): boolean {
  if (!cause || typeof cause !== "object") return false;
  const status = (cause as { status?: number }).status;
  const code = (cause as { code?: string }).code;
  return status === 404 || code === "CODE_UI_UNAVAILABLE" || code === "UNSUPPORTED_OPERATION";
}
