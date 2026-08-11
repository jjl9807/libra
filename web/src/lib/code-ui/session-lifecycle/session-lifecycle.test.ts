import { describe, expect, it } from "vitest";

import {
  awaitingSessionFixture,
  busySessionFixture,
  canSelectThreadForResume,
  createSessionLifecycleApi,
  resumeAffordance,
  terminalSessionFixture,
  threadListFixture,
} from ".";
import { sessionFixture } from "../fixtures";

describe("session-lifecycle helpers", () => {
  it("classifies terminal and awaiting resume affordances", () => {
    expect(resumeAffordance(terminalSessionFixture()).kind).toBe("ready");
    expect(resumeAffordance(awaitingSessionFixture()).kind).toBe("ready");
    expect(resumeAffordance(busySessionFixture()).kind).toBe("deferred");
    expect(resumeAffordance(sessionFixture()).kind).toBe("unsupported");
    expect(
      resumeAffordance(
        terminalSessionFixture({
          status: "indeterminate_side_effect",
        }),
      ).kind,
    ).toBe("unsupported");
  });

  it("validates thread selection against the current session", () => {
    const ready = resumeAffordance(terminalSessionFixture());
    expect(canSelectThreadForResume(ready, "")).toMatch(/select a thread/i);
    expect(
      canSelectThreadForResume(ready, "thread-fixture", "thread-fixture"),
    ).toMatch(/already the active/i);
    expect(canSelectThreadForResume(ready, "thread-fixture-2", "thread-fixture")).toBeUndefined();
  });

  it("lists threads through the domain API", async () => {
    const response = threadListFixture();
    const api = createSessionLifecycleApi({
      async request<T>(path: string): Promise<T> {
        expect(path).toBe("/api/code/threads?limit=50&offset=0");
        return response as T;
      },
    });
    await expect(api.listThreads()).resolves.toEqual(response);
  });
});
