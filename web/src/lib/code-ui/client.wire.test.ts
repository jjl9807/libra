/**
 * Loopback wire smoke for FetchCodeUiTransport against a protocol-faithful
 * stand-in for the Rust `/api/code` router (paths, controller token header,
 * named SSE events). Live axum coverage remains W3-02 / Playwright W3-15.
 */
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  CODE_UI_SSE_EVENT_TYPES,
  createCodeUiClient,
  FetchCodeUiTransport,
} from "./client";
import { sessionFixture, sseFixture } from "./fixtures";
import type { CodeUiEventEnvelope, CodeUiSessionSnapshot } from "./types";

type SseClient = {
  write(event: string, data: unknown): void;
  end(): void;
};

class NodeEventSource {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;

  readyState = NodeEventSource.CONNECTING;
  onmessage: ((event: MessageEvent<string>) => void) | null = null;
  onerror: (() => void) | null = null;

  private readonly listeners = new Map<string, Set<(event: MessageEvent<string>) => void>>();
  private readonly abort = new AbortController();

  constructor(url: string, _init?: EventSourceInit) {
    void _init;
    void this.connect(url);
  }

  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void {
    const set = this.listeners.get(type) ?? new Set();
    set.add(listener);
    this.listeners.set(type, set);
  }

  close(): void {
    this.abort.abort();
    this.readyState = NodeEventSource.CLOSED;
  }

  private dispatch(type: string, data: string): void {
    const event = { data } as MessageEvent<string>;
    if (type === "message") {
      this.onmessage?.(event);
      return;
    }
    this.listeners.get(type)?.forEach((listener) => listener(event));
  }

  private async connect(url: string): Promise<void> {
    try {
      const response = await fetch(url, {
        headers: { accept: "text/event-stream" },
        signal: this.abort.signal,
      });
      if (!response.ok || !response.body) {
        this.onerror?.();
        return;
      }
      this.readyState = NodeEventSource.OPEN;
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let split = buffer.indexOf("\n\n");
        while (split >= 0) {
          const frame = buffer.slice(0, split);
          buffer = buffer.slice(split + 2);
          let eventType = "message";
          const dataLines: string[] = [];
          for (const line of frame.split("\n")) {
            if (line.startsWith("event:")) eventType = line.slice(6).trim();
            else if (line.startsWith("data:")) dataLines.push(line.slice(5).trimStart());
          }
          if (dataLines.length > 0) this.dispatch(eventType, dataLines.join("\n"));
          split = buffer.indexOf("\n\n");
        }
      }
    } catch (error) {
      if ((error as { name?: string }).name === "AbortError") return;
      this.onerror?.();
    } finally {
      if (this.readyState !== NodeEventSource.CLOSED) {
        this.readyState = NodeEventSource.CLOSED;
      }
    }
  }
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

function json(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

async function startWireServer(): Promise<{
  baseUrl: string;
  server: Server;
  seen: {
    paths: string[];
    methods: string[];
    controllerTokens: Array<string | undefined>;
    attachBodies: unknown[];
  };
  pushSse: (event: CodeUiEventEnvelope) => void;
}> {
  const snapshot = sessionFixture({
    controller: { kind: "browser", canWrite: true, loopbackOnly: true, ownerLabel: "browser-1" },
  });
  const seen = {
    paths: [] as string[],
    methods: [] as string[],
    controllerTokens: [] as Array<string | undefined>,
    attachBodies: [] as unknown[],
  };
  const sseClients = new Set<SseClient>();

  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    seen.paths.push(url.pathname);
    seen.methods.push(req.method ?? "");
    seen.controllerTokens.push(
      typeof req.headers["x-code-controller-token"] === "string"
        ? req.headers["x-code-controller-token"]
        : undefined,
    );

    if (req.method === "GET" && url.pathname === "/api/code/session") {
      json(res, 200, snapshot);
      return;
    }

    if (req.method === "POST" && url.pathname === "/api/code/controller/attach") {
      const body = JSON.parse(await readBody(req)) as unknown;
      seen.attachBodies.push(body);
      json(res, 200, {
        controllerToken: "wire-controller-token",
        leaseExpiresAt: "2026-07-15T00:02:00.000Z",
        controller: snapshot.controller,
      });
      return;
    }

    if (req.method === "POST" && url.pathname === "/api/code/control/cancel") {
      json(res, 200, { accepted: true });
      return;
    }

    if (req.method === "GET" && url.pathname === "/api/code/events") {
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      const client: SseClient = {
        write(event, data) {
          res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
        },
        end() {
          res.end();
        },
      };
      sseClients.add(client);
      client.write("session_updated", sseFixture(snapshot, { seq: 1 }));
      req.on("close", () => {
        sseClients.delete(client);
      });
      return;
    }

    json(res, 404, { error: { code: "NOT_FOUND", message: `no route for ${url.pathname}` } });
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("expected TCP listen address");
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    server,
    seen,
    pushSse(event) {
      for (const client of sseClients) {
        client.write(event.type, event);
      }
    },
  };
}

describe("FetchCodeUiTransport loopback wire smoke", () => {
  let server: Server | undefined;
  const previousEventSource = globalThis.EventSource;

  beforeEach(() => {
    (globalThis as { EventSource: typeof EventSource }).EventSource =
      NodeEventSource as unknown as typeof EventSource;
  });

  afterEach(async () => {
    (globalThis as { EventSource: typeof EventSource }).EventSource = previousEventSource;
    await new Promise<void>((resolve, reject) => {
      if (!server) {
        resolve();
        return;
      }
      server.close((error) => (error ? reject(error) : resolve()));
      server = undefined;
    });
  });

  it("loads snapshot, attaches with browser kind, sends controller token, and observes named SSE", async () => {
    const wire = await startWireServer();
    server = wire.server;
    const client = createCodeUiClient(new FetchCodeUiTransport(wire.baseUrl));

    const snapshot = await client.snapshot();
    expect(snapshot.sessionId).toBe("session-fixture");

    const attach = await client.attach("browser-1");
    expect(attach.controllerToken).toBe("wire-controller-token");
    expect(wire.seen.attachBodies[0]).toEqual({ clientId: "browser-1", kind: "browser" });

    await client.cancel(attach.controllerToken);
    expect(wire.seen.controllerTokens.at(-1)).toBe("wire-controller-token");

    const received: CodeUiEventEnvelope[] = [];
    const stream = client.observe((event) => received.push(event), () => {
      throw new Error("SSE transport error");
    });

    await viWaitFor(() => received.length >= 1);
    expect(received[0]?.type).toBe("session_updated");
    expect(CODE_UI_SSE_EVENT_TYPES).toContain(received[0]?.type);

    const updated: CodeUiSessionSnapshot = {
      ...snapshot,
      status: "thinking",
      updatedAt: "2026-07-15T00:00:01.000Z",
    };
    wire.pushSse(sseFixture(updated, { seq: 2, type: "status_changed", at: updated.updatedAt }));
    await viWaitFor(() => received.some((event) => event.seq === 2));
    expect(received.find((event) => event.seq === 2)?.type).toBe("status_changed");

    stream.close();
    expect(wire.seen.paths).toEqual([
      "/api/code/session",
      "/api/code/controller/attach",
      "/api/code/control/cancel",
      "/api/code/events",
    ]);
    expect(wire.seen.methods).toEqual(["GET", "POST", "POST", "GET"]);
  });
});

async function viWaitFor(predicate: () => boolean, timeoutMs = 2_000): Promise<void> {
  const started = Date.now();
  while (!predicate()) {
    if (Date.now() - started > timeoutMs) {
      throw new Error("timed out waiting for wire condition");
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}
