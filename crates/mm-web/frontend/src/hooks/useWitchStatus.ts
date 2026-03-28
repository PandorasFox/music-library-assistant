import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { WitchStatus } from "../api/generated/types";
import { witchSocket, type WsState } from "../api/ws";
import { queryKeys } from "../api/queries";

/**
 * WebSocket-fed WitchStatus hook.
 *
 * Subscribes to the WitchSocket singleton. On each StatusChanged event:
 * 1. Stores the latest WitchStatus in React state.
 * 2. Compares generation counters to the previous snapshot.
 * 3. Invalidates TanStack Query caches when generations change.
 */
export function useWitchStatus(): {
  status: WitchStatus | null;
  wsState: WsState;
} {
  const queryClient = useQueryClient();
  const [status, setStatus] = useState<WitchStatus | null>(null);
  const [wsState, setWsState] = useState<WsState>(witchSocket.state);
  const prevRef = useRef<{
    mutations_generation: number;
    computations_generation: number;
    error_generation: number;
    config_generation: number;
  } | null>(null);

  useEffect(() => {
    const unsubEvent = witchSocket.onEvent((event) => {
      if ("StatusChanged" in event) {
        const next = event.StatusChanged;
        setStatus(next);

        const prev = prevRef.current;
        if (prev) {
          const dataChanged =
            next.mutations_generation !== prev.mutations_generation ||
            next.computations_generation !== prev.computations_generation;

          if (dataChanged) {
            void queryClient.invalidateQueries({
              queryKey: queryKeys.insights,
            });
            void queryClient.invalidateQueries({
              queryKey: queryKeys.externalMatches,
            });
            void queryClient.invalidateQueries({
              queryKey: queryKeys.deployStatus,
            });
          }
        }

        prevRef.current = {
          mutations_generation: next.mutations_generation,
          computations_generation: next.computations_generation,
          error_generation: next.error_generation,
          config_generation: next.config_generation,
        };
      }
    });

    const unsubState = witchSocket.onStateChange(setWsState);

    return () => {
      unsubEvent();
      unsubState();
    };
  }, [queryClient]);

  return { status, wsState };
}
