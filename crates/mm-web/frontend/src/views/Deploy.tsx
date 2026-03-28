import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useDeployStatus, useDeployData } from "../api/queries";
import { post } from "../api/client";
import { ApiError } from "../api/client";
import type {
  DeployModalData,
  DeploySignalFile,
  StaleSignalFile,
  ConflictGroup,
  DirectoryAggregate,
  LibrarySummary,
} from "../api/generated/types";

type Tab = "healthy" | "new" | "conflicts" | "leftover" | "stale";

function TabButton({
  tab,
  label,
  count,
  active,
  onClick,
}: {
  tab: Tab;
  label: string;
  count: number;
  active: boolean;
  onClick: (t: Tab) => void;
}) {
  return (
    <button
      className={`deploy-tab ${active ? "deploy-tab--active" : ""} ${count > 0 && tab !== "healthy" ? "deploy-tab--warn" : ""}`}
      onClick={() => onClick(tab)}
    >
      {label} ({count})
    </button>
  );
}

function HealthyTab({ files }: { files: DeploySignalFile[] }) {
  const show = files.slice(0, 200);
  return (
    <div className="deploy-panel">
      {show.map((f, i) => (
        <div key={i} className="deploy-file-row">
          <span className="deploy-file-lib">{f.library_name}</span>
          <span className="deploy-file-path">{f.deploy_path}</span>
        </div>
      ))}
      {files.length > 200 && (
        <div className="deploy-more">...and {files.length - 200} more</div>
      )}
    </div>
  );
}

function NewTab({ dirs }: { dirs: DirectoryAggregate[] }) {
  return (
    <div className="deploy-panel">
      {dirs.map((d) => (
        <div key={d.directory} className="deploy-dir-row">
          <span className="deploy-dir-name">{d.directory}</span>
          <span className="deploy-dir-count">
            {d.count} file{d.count !== 1 ? "s" : ""}
            {d.sidecar_count > 0 ? ` + ${d.sidecar_count} sidecar${d.sidecar_count !== 1 ? "s" : ""}` : ""}
          </span>
        </div>
      ))}
    </div>
  );
}

function ConflictsTab({ conflicts }: { conflicts: ConflictGroup[] }) {
  if (conflicts.length === 0)
    return <div className="deploy-panel deploy-empty">No conflicts</div>;

  return (
    <div className="deploy-panel">
      {conflicts.map((c, i) => (
        <div key={i} className="deploy-conflict">
          <div className="deploy-conflict__path">{c.deploy_path}</div>
          <div className="deploy-conflict__files">
            {c.conflicting_files.map(([path], j) => (
              <div key={j} className="deploy-conflict__file">{path}</div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function LeftoverTab({ dirs }: { dirs: DirectoryAggregate[] }) {
  return (
    <div className="deploy-panel">
      {dirs.map((d) => (
        <div key={d.directory} className="deploy-dir-row">
          <span className="deploy-dir-name">{d.directory}</span>
          <span className="deploy-dir-count">
            {d.count} file{d.count !== 1 ? "s" : ""}
          </span>
        </div>
      ))}
    </div>
  );
}

function StaleTab({ stale }: { stale: StaleSignalFile[] }) {
  const show = stale.slice(0, 200);
  return (
    <div className="deploy-panel">
      {show.map((s, i) => (
        <div key={i} className="deploy-stale-row">
          <span className="deploy-stale-from">{s.library_path}</span>
          <span className="deploy-stale-arrow">&rarr;</span>
          <span className="deploy-stale-to">{s.expected_path}</span>
        </div>
      ))}
      {stale.length > 200 && (
        <div className="deploy-more">...and {stale.length - 200} more</div>
      )}
    </div>
  );
}

function LibrarySummaries({ summaries }: { summaries: LibrarySummary[] }) {
  if (summaries.length === 0) return null;
  return (
    <div className="deploy-summaries">
      {summaries.map((s) => (
        <div key={s.library_name} className="deploy-lib-summary">
          <span className="deploy-lib-name">{s.library_name}</span>
          <span className="deploy-lib-stats">
            {s.healthy_count} healthy, {s.new_count} new, {s.leftover_count} leftover, {s.stale_count} stale
          </span>
        </div>
      ))}
    </div>
  );
}

function DeployPreview({ data }: { data: DeployModalData }) {
  const navigate = useNavigate();
  const totalOps =
    data.new.length +
    data.leftover.length +
    data.stale.length +
    data.sidecars.length;

  const initialTab: Tab = data.new.length > 0 ? "new" : "healthy";
  const [activeTab, setActiveTab] = useState<Tab>(initialTab);
  const [staging, setStaging] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  async function handleStageDeploy() {
    setStaging(true);
    setMessage(null);
    try {
      // Start transaction
      try {
        await post("/tx/start", { label: "Deploy" });
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 409)) throw e;
      }

      // Build deploy mutations from the data
      // The server has the full data — we just signal intent.
      // We POST the full deploy data so the server can build mutations.
      // Actually, we need to build the mutations client-side from the data.
      // HardLink for new files, LibraryMove for stale, StashLeftovers for leftovers.
      const mutations: unknown[] = [];

      for (const f of data.leftover) {
        mutations.push({
          StashLeftovers: { path: `libraries/${f.library_path}` },
        });
      }
      for (const f of data.stale) {
        mutations.push({
          LibraryMove: {
            source: `libraries/${f.library_path}`,
            destination: `libraries/${f.expected_path}`,
          },
        });
      }
      for (const f of data.new) {
        if (f.deploy_path && f.library_name) {
          mutations.push({
            HardLink: {
              source: f.corpus_path,
              destination: `libraries/${f.library_name}/${f.deploy_path}`,
            },
          });
        }
      }

      if (mutations.length > 0) {
        await post("/tx/add", {
          key: "Deploy",
          decision: {
            label: `Deploy ${mutations.length} operations`,
            mutations,
          },
        });
      }

      // Sidecar deploy as separate decision
      const sidecarMutations: unknown[] = [];
      for (const s of data.sidecars) {
        sidecarMutations.push({
          HardLink: {
            source: s.corpus_image_path,
            destination: `libraries/${s.library_name}/${s.library_album_dir}/${s.filename}`,
          },
        });
      }
      if (sidecarMutations.length > 0) {
        await post("/tx/add", {
          key: "DeploySidecars",
          decision: {
            label: `Deploy ${sidecarMutations.length} sidecars`,
            mutations: sidecarMutations,
          },
        });
      }

      navigate("/tx");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setStaging(false);
    }
  }

  return (
    <div>
      <div className="deploy-header">
        <LibrarySummaries summaries={data.per_library} />
        {totalOps > 0 && (
          <button onClick={handleStageDeploy} disabled={staging}>
            {staging ? "Staging..." : `Stage Deploy (${totalOps} ops)`}
          </button>
        )}
      </div>

      {message && <div className="cfg-message cfg-message--error">{message}</div>}

      <div className="deploy-tabs">
        <TabButton tab="healthy" label="Healthy" count={data.healthy.length} active={activeTab === "healthy"} onClick={setActiveTab} />
        <TabButton tab="new" label="New" count={data.new.length} active={activeTab === "new"} onClick={setActiveTab} />
        <TabButton tab="conflicts" label="Conflicts" count={data.conflicts.length} active={activeTab === "conflicts"} onClick={setActiveTab} />
        <TabButton tab="leftover" label="Leftover" count={data.leftover.length} active={activeTab === "leftover"} onClick={setActiveTab} />
        <TabButton tab="stale" label="Stale" count={data.stale.length} active={activeTab === "stale"} onClick={setActiveTab} />
      </div>

      {activeTab === "healthy" && <HealthyTab files={data.healthy} />}
      {activeTab === "new" && <NewTab dirs={data.new_by_dir} />}
      {activeTab === "conflicts" && <ConflictsTab conflicts={data.conflicts} />}
      {activeTab === "leftover" && <LeftoverTab dirs={data.leftover_by_dir} />}
      {activeTab === "stale" && <StaleTab stale={data.stale} />}
    </div>
  );
}

export function Deploy() {
  const { data: status, isLoading: loadingStatus } = useDeployStatus();
  const { data: deployData, isLoading: loadingData } = useDeployData();

  if (loadingStatus) return <div className="view-loading">Loading deploy status...</div>;

  if (status && !status.needs_action) {
    return (
      <div className="deploy-view">
        <div className="deploy-uptodate">
          Libraries up to date
          {status.library_file_counts.length > 0 && (
            <div className="deploy-lib-counts">
              {status.library_file_counts.map(([name, count]) => (
                <span key={name}>
                  {name}: {count.toLocaleString()}
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
    );
  }

  if (loadingData) return <div className="view-loading">Loading deploy data...</div>;
  if (!deployData) return <div className="view-error">No deploy data</div>;

  return (
    <div className="deploy-view">
      <DeployPreview data={deployData} />
    </div>
  );
}
