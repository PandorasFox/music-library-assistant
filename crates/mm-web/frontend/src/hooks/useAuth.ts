import { useCallback, useState } from "react";
import { login as apiLogin } from "../api/queries";
import { post } from "../api/client";
import { witchSocket } from "../api/ws";

/**
 * Auth hook — cookie-based.
 *
 * Login sets an HttpOnly cookie server-side. JS can't read it, but the
 * browser sends it automatically on every request. We track auth state
 * locally as a boolean flipped on login/logout.
 */
export function useAuth() {
  // Optimistic: if we got this far in the app (past AuthGuard), we're authed.
  // The cookie handles actual auth. This flag just drives redirect logic.
  const [isAuthenticated, setIsAuthenticated] = useState(
    // On first load, check if we have a cookie by probing a lightweight endpoint.
    // For now, assume not authenticated — AuthGuard will redirect to login.
    false,
  );

  const login = useCallback(async (username: string, password: string) => {
    await apiLogin(username, password);
    // Cookie is now set by the server. Mark as authenticated.
    setIsAuthenticated(true);
    witchSocket.connect();
  }, []);

  const logout = useCallback(async () => {
    try {
      await post("/auth/logout", {});
    } catch {
      // Best-effort — cookie cleared server-side.
    }
    setIsAuthenticated(false);
    witchSocket.disconnect();
  }, []);

  // Allow external code to mark as authenticated (e.g., after verifying cookie works).
  const markAuthenticated = useCallback(() => {
    setIsAuthenticated(true);
  }, []);

  return { isAuthenticated, login, logout, markAuthenticated };
}
