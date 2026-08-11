"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  type PropsWithChildren,
} from "react";

import { useCodeUiStore } from "./store";
import type { CodeUiInteractionResponse } from "./types";

export interface BrowserController {
  clientId: string;
  token?: string;
  ensureLease(): Promise<string>;
  submit(text: string, commandId?: string): Promise<void>;
  respond(interactionId: string, response: CodeUiInteractionResponse): Promise<void>;
  cancel(): Promise<void>;
}
export type ControllerExtension = (controller: BrowserController) => void | (() => void);

const BrowserControllerContext = createContext<BrowserController | undefined>(undefined);

const BROWSER_CLIENT_ID_STORAGE_KEY = "libra.code-ui.browser-client-id";

function browserClientId(): string {
  if (typeof window !== "undefined") {
    try {
      const existing = window.sessionStorage.getItem(BROWSER_CLIENT_ID_STORAGE_KEY);
      if (existing) return existing;
    } catch {
      // Private mode or blocked storage — fall through to an ephemeral id.
    }
  }
  const id =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `browser-${Math.random().toString(36).slice(2)}`;
  if (typeof window !== "undefined") {
    try {
      window.sessionStorage.setItem(BROWSER_CLIENT_ID_STORAGE_KEY, id);
    } catch {
      // Same as above: continue without persistence.
    }
  }
  return id;
}

export function BrowserControllerProvider({
  children,
  clientId: injectedClientId,
  extensions = [],
}: PropsWithChildren<{ clientId?: string; extensions?: ControllerExtension[] }>) {
  const { client } = useCodeUiStore();
  const fallbackClientId = useRef<string | undefined>(undefined);
  const clientId = injectedClientId ?? (fallbackClientId.current ??= browserClientId());
  const token = useRef<string | undefined>(undefined);
  const leaseExpiresAt = useRef<number | undefined>(undefined);

  const clearToken = useCallback(() => {
    token.current = undefined;
    leaseExpiresAt.current = undefined;
  }, []);
  const ensureLease = useCallback(async () => {
    if (token.current && leaseExpiresAt.current !== undefined && Date.now() >= leaseExpiresAt.current) {
      clearToken();
    }
    if (!token.current) {
      const lease = await client.attach(clientId);
      token.current = lease.controllerToken;
      const expiresAt = Date.parse(lease.leaseExpiresAt);
      leaseExpiresAt.current = Number.isFinite(expiresAt) ? expiresAt : undefined;
    }
    return token.current;
  }, [clearToken, client, clientId]);
  const withLease = useCallback(
    async <T,>(operation: (lease: string) => Promise<T>): Promise<T> => {
      try {
        return await operation(await ensureLease());
      } catch (cause) {
        const code = (cause as { code?: string }).code;
        const staleLeaseConflict =
          code === "CONTROLLER_CONFLICT" &&
          leaseExpiresAt.current !== undefined &&
          Date.now() >= leaseExpiresAt.current;
        if (
          code !== "MISSING_CONTROLLER_TOKEN" &&
          code !== "INVALID_CONTROLLER_TOKEN" &&
          !staleLeaseConflict
        ) {
          throw cause;
        }
        clearToken();
        return operation(await ensureLease());
      }
    },
    [clearToken, ensureLease],
  );

  const controller = useMemo<BrowserController>(
    () => ({
      clientId,
      get token() {
        return token.current;
      },
      ensureLease,
      submit: async (text, commandId) => {
        await withLease((lease) => client.submit(text, lease, commandId));
      },
      respond: async (interactionId, response) => {
        await withLease((lease) => client.respond(interactionId, response, lease));
      },
      cancel: async () => {
        await withLease((lease) => client.cancel(lease));
      },
    }),
    [client, clientId, ensureLease, withLease],
  );

  useEffect(() => {
    const cleanups = extensions
      .map((extension) => extension(controller))
      .filter((cleanup): cleanup is () => void => typeof cleanup === "function");
    return () => cleanups.forEach((cleanup) => cleanup());
  }, [controller, extensions]);

  useEffect(() => {
    const detach = () => {
      if (!token.current) return;
      void client.detach(clientId, token.current, true);
      clearToken();
    };
    window.addEventListener("beforeunload", detach);
    return () => {
      window.removeEventListener("beforeunload", detach);
      detach();
    };
  }, [client, clearToken, clientId]);

  return (
    <BrowserControllerContext.Provider value={controller}>
      {children}
    </BrowserControllerContext.Provider>
  );
}

export function useBrowserController(): BrowserController {
  const value = useContext(BrowserControllerContext);
  if (!value) throw new Error("useBrowserController must be used inside BrowserControllerProvider");
  return value;
}
