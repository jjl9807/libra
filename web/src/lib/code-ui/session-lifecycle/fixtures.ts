import { lifecycleFixture, sessionFixture } from "../fixtures";
import type { CodeUiSessionSnapshot } from "../types";

import type { ThreadListItem, ThreadListResponse } from "./threads";

const now = "2026-07-15T00:00:00.000Z";

export function threadItemFixture(
  overrides: Partial<ThreadListItem> = {},
): ThreadListItem {
  return {
    id: "thread-fixture",
    title: "Prior investigation",
    archived: false,
    currentIntentId: "intent-1",
    createdAt: now,
    updatedAt: now,
    ...overrides,
  };
}

export function threadListFixture(
  overrides: Partial<ThreadListResponse> = {},
): ThreadListResponse {
  return {
    items: [
      threadItemFixture(),
      threadItemFixture({
        id: "thread-fixture-2",
        title: "Earlier turn",
        updatedAt: "2026-07-15T00:01:00.000Z",
      }),
    ],
    nextOffset: undefined,
    ...overrides,
  };
}

export function terminalSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return lifecycleFixture({
    status: "completed",
    capabilities: {
      ...sessionFixture().capabilities,
      providerSessionResume: true,
    },
    ...overrides,
  });
}

export function awaitingSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return lifecycleFixture({
    status: "awaiting_interaction",
    capabilities: {
      ...sessionFixture().capabilities,
      providerSessionResume: true,
    },
    ...overrides,
  });
}

export function busySessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return lifecycleFixture({
    status: "thinking",
    capabilities: {
      ...sessionFixture().capabilities,
      providerSessionResume: true,
    },
    ...overrides,
  });
}
