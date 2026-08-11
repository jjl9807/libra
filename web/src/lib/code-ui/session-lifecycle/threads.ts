import type { CodeUiApiError } from "../types";

export interface ThreadListItem {
  id: string;
  title?: string;
  archived: boolean;
  currentIntentId?: string;
  /**
   * Omitted until ThreadProjection persists a per-thread cwd. Clients must not
   * invent the live session cwd for foreign threads.
   */
  workingDir?: string;
  createdAt: string;
  updatedAt: string;
}

export interface ThreadListResponse {
  items: ThreadListItem[];
  nextOffset?: number;
}

export interface SessionResumeRequest {
  threadId: string;
}

export interface SessionLifecycleTransport {
  request<T>(path: string, init?: RequestInit): Promise<T>;
}

export class FetchSessionLifecycleTransport implements SessionLifecycleTransport {
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

export function createSessionLifecycleApi(
  transport: SessionLifecycleTransport = new FetchSessionLifecycleTransport(),
) {
  return {
    listThreads(limit = 50, offset = 0): Promise<ThreadListResponse> {
      const params = new URLSearchParams({
        limit: String(limit),
        offset: String(offset),
      });
      return transport.request(`/api/code/threads?${params.toString()}`);
    },
    /**
     * Leased browser resume. Production currently fail-closes with
     * `SESSION_RESUME_REQUIRES_RESTART` after proving the thread is loadable —
     * callers should surface the server message (CLI restart hint).
     */
    resumeSession(threadId: string, controllerToken: string): Promise<unknown> {
      return transport.request("/api/code/session/resume", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "X-Code-Controller-Token": controllerToken,
        },
        body: JSON.stringify({ threadId } satisfies SessionResumeRequest),
      });
    },
  };
}

export type SessionLifecycleApi = ReturnType<typeof createSessionLifecycleApi>;
