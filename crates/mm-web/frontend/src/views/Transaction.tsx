import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTxDetails, queryKeys, type DecisionDetail } from "../api/queries";
import { post } from "../api/client";
import { useWitchStatus } from "../hooks/useWitchStatus";

function DecisionKeyLabel({ decisionKey }: { decisionKey: unknown }): string {
  if (typeof decisionKey === "string") return decisionKey;
  if (typeof decisionKey === "object" && decisionKey !== null) {
    const entries = Object.entries(decisionKey as Record<string, unknown>);
    if (entries.length === 1) {
      const [variant, data] = entries[0]!;
      if (typeof data === "object" && data !== null) {
        const vals = Object.values(data as Record<string, unknown>);
        if (vals.length > 0) {
          return `${variant}: ${vals.map(String).join(", ")}`;
        }
      }
      return variant;
    }
  }
  return JSON.stringify(decisionKey);
}

function MutationSummary({ mutations }: { mutations: unknown[] }) {
  return (
    <div className="tx-mutations">
      {mutations.map((m, i) => {
        const label =
          typeof m === "object" && m !== null
            ? Object.keys(m as Record<string, unknown>)[0] ?? "Unknown"
            : String(m);
        return (
          <span key={i} className="tx-mutation-badge">
            {label}
          </span>
        );
      })}
    </div>
  );
}

function DecisionRow({ detail }: { detail: DecisionDetail }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="tx-decision">
      <button
        className="tx-decision__header"
        onClick={() => setExpanded(!expanded)}
        type="button"
      >
        <span className="dir-arrow dir-arrow--open"
          style={{ transform: expanded ? "rotate(90deg)" : "none" }}
        >
          {"\u25b6"}
        </span>
        <span className="tx-decision__label">{detail.label}</span>
        <span className="tx-decision__key">
          {DecisionKeyLabel({ decisionKey: detail.key })}
        </span>
      </button>
      {expanded && (
        <div className="tx-decision__body">
          <MutationSummary mutations={detail.mutations} />
          <pre className="tx-decision__json">
            {JSON.stringify(detail.mutations, null, 2)}
          </pre>
        </div>
      )}
    </div>
  );
}

export function Transaction() {
  const { status } = useWitchStatus();
  const queryClient = useQueryClient();
  const hasTransaction = !!status?.transaction;

  const { data: details, isLoading, error, refetch } = useTxDetails();
  const [acting, setActing] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  async function handleConfirm() {
    setActing(true);
    setMessage(null);
    try {
      await post("/tx/confirm", {});
      setMessage("Transaction confirmed.");
      void queryClient.invalidateQueries({ queryKey: queryKeys.insights });
      void queryClient.invalidateQueries({ queryKey: queryKeys.config });
      void refetch();
    } catch (err) {
      setMessage(`Confirm failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setActing(false);
    }
  }

  async function handleDiscard() {
    setActing(true);
    setMessage(null);
    try {
      await post("/tx/discard", {});
      setMessage("Transaction discarded.");
      void refetch();
    } catch (err) {
      setMessage(`Discard failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setActing(false);
    }
  }

  if (!hasTransaction) {
    return (
      <div className="tx-view">
        <div className="view-placeholder">No active transaction</div>
        {message && <div className="cfg-message cfg-message--ok">{message}</div>}
      </div>
    );
  }

  const snapshot = status!.transaction!;

  return (
    <div className="tx-view">
      <div className="tx-header">
        <h2>{snapshot.label}</h2>
        <span className="tx-counts">
          {snapshot.decision_count} decision{snapshot.decision_count !== 1 ? "s" : ""},{" "}
          {snapshot.mutation_count} mutation{snapshot.mutation_count !== 1 ? "s" : ""}
        </span>
      </div>

      {message && (
        <div
          className={`cfg-message ${message.startsWith("Transaction confirmed") || message.startsWith("Transaction discarded") ? "cfg-message--ok" : "cfg-message--error"}`}
        >
          {message}
        </div>
      )}

      <div className="tx-actions">
        <button onClick={handleConfirm} disabled={acting}>
          {acting ? "..." : "Confirm"}
        </button>
        <button
          className="cfg-toolbar__discard"
          onClick={handleDiscard}
          disabled={acting}
        >
          Discard
        </button>
      </div>

      {isLoading && <div className="view-loading">Loading details...</div>}
      {error && <div className="view-error">{error.message}</div>}

      {details && Array.isArray(details) && (
        <div className="tx-decisions">
          {details.map((d, i) => (
            <DecisionRow key={i} detail={d} />
          ))}
        </div>
      )}
    </div>
  );
}
