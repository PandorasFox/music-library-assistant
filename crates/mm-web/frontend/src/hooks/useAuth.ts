import { useCallback, useSyncExternalStore } from "react";
import { getToken, setToken, clearToken } from "../api/client";
import { login as apiLogin } from "../api/queries";
import { witchSocket } from "../api/ws";

// Simple external store for token presence (avoids full state management).
let tokenSnapshot = getToken() !== null;
const subscribers = new Set<() => void>();

function notify(): void {
  tokenSnapshot = getToken() !== null;
  for (const fn of subscribers) fn();
}

function subscribe(cb: () => void): () => void {
  subscribers.add(cb);
  return () => subscribers.delete(cb);
}

export function useAuth() {
  const isAuthenticated = useSyncExternalStore(
    subscribe,
    () => tokenSnapshot,
  );

  const login = useCallback(async (username: string, password: string) => {
    const token = await apiLogin(username, password);
    setToken(token);
    notify();
    witchSocket.connect();
  }, []);

  const logout = useCallback(() => {
    clearToken();
    notify();
    witchSocket.disconnect();
  }, []);

  return { isAuthenticated, login, logout };
}
