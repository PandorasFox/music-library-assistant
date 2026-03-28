import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useDirConfig, useConfig, type SourceDir } from "../api/queries";
import { post } from "../api/client";
import { ApiError } from "../api/client";

type TriState = boolean | null;

function TriSelect({
  value,
  onChange,
}: {
  value: TriState;
  onChange: (v: TriState) => void;
}) {
  return (
    <select
      className="cfg-enum"
      value={value == null ? "inherit" : String(value)}
      onChange={(e) => {
        const v = e.target.value;
        onChange(v === "inherit" ? null : v === "true");
      }}
    >
      <option value="inherit">(inherit)</option>
      <option value="true">Yes</option>
      <option value="false">No</option>
    </select>
  );
}

function SanctitySelect({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (v: string | null) => void;
}) {
  return (
    <select
      className="cfg-enum"
      value={value ?? "inherit"}
      onChange={(e) =>
        onChange(e.target.value === "inherit" ? null : e.target.value)
      }
    >
      <option value="inherit">(inherit)</option>
      <option value="DontTouch">Don't touch</option>
      <option value="ReplaceIfBetter">Replace if better</option>
      <option value="ReplaceAlways">Replace always</option>
    </select>
  );
}

interface DirConfigPanelProps {
  path: string;
  onClose: () => void;
}

export function DirConfigPanel({ path, onClose }: DirConfigPanelProps) {
  const { data: dirConfig, isLoading: loadingDir } = useDirConfig(path);
  const { data: fullConfig } = useConfig();
  const navigate = useNavigate();

  const [libraries, setLibraries] = useState<string | null>(null);
  const [canStashDupes, setCanStashDupes] = useState<TriState | undefined>(
    undefined,
  );
  const [interiorDupes, setInteriorDupes] = useState<TriState | undefined>(
    undefined,
  );
  const [enableAcoustid, setEnableAcoustid] = useState<TriState | undefined>(
    undefined,
  );
  const [coverArtSanctity, setCoverArtSanctity] = useState<
    string | null | undefined
  >(undefined);
  const [pathSchema, setPathSchema] = useState<string | null>(null);
  const [pinnedRelease, setPinnedRelease] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Resolve effective values: local state overrides loaded data
  const src = dirConfig ?? null;
  const effLibraries = libraries ?? src?.libraries.join(", ") ?? "";
  const effCanStash = canStashDupes !== undefined ? canStashDupes : (src?.can_stash_dupes ?? null);
  const effInterior = interiorDupes !== undefined ? interiorDupes : (src?.interior_dupes ?? null);
  const effAcoustid = enableAcoustid !== undefined ? enableAcoustid : (src?.enable_acoustid ?? null);
  const effSanctity = coverArtSanctity !== undefined ? coverArtSanctity : (src?.cover_art_sanctity ?? null);
  const effSchema = pathSchema ?? src?.path_schema ?? "";
  const effPinned = pinnedRelease ?? src?.pinned_release ?? "";

  async function handleSave() {
    if (!fullConfig) return;
    setSaving(true);
    setError(null);

    try {
      const oldDir: SourceDir = src ?? {
        path,
        libraries: [],
        can_stash_dupes: null,
        interior_dupes: null,
        path_schema: null,
        enable_acoustid: null,
        pinned_release: null,
        cover_art_sanctity: null,
      };

      const newDir: SourceDir = {
        path,
        libraries: effLibraries
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s.length > 0),
        can_stash_dupes: effCanStash,
        interior_dupes: effInterior,
        path_schema: effSchema || null,
        enable_acoustid: effAcoustid,
        pinned_release: effPinned || null,
        cover_art_sanctity: effSanctity,
      };

      // Build new config with updated source_dirs
      const newConfig = structuredClone(fullConfig);
      const sourceDirs = (newConfig as Record<string, unknown>)
        .source_dirs as SourceDir[];
      const idx = sourceDirs.findIndex((d) => d.path === path);
      if (idx >= 0) {
        sourceDirs[idx] = newDir;
      } else {
        sourceDirs.push(newDir);
      }

      // Start transaction (ignore AlreadyActive)
      try {
        await post("/tx/start", { label: "Dir config edit" });
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 409)) throw e;
      }

      await post("/tx/add", {
        key: { DirConfigEdit: { source_path: path } },
        decision: {
          label: `Dir config: ${path}`,
          mutations: [
            {
              ApplyDirConfigEdit: {
                source_path: path,
                old_dir: oldDir,
                new_dir: newDir,
                new_config: newConfig,
              },
            },
          ],
        },
      });

      navigate("/tx");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  if (loadingDir) {
    return <div className="dc-panel dc-panel--loading">Loading config...</div>;
  }

  return (
    <div className="dc-panel">
      <div className="dc-header">
        <span className="dc-title">Config: {path}</span>
        <button className="dc-close" onClick={onClose} type="button">
          ×
        </button>
      </div>
      {error && <div className="form-error">{error}</div>}

      <div className="dc-field">
        <label>Libraries</label>
        <input
          type="text"
          value={effLibraries}
          onChange={(e) => setLibraries(e.target.value)}
          placeholder="comma-separated library names"
        />
      </div>

      <div className="dc-field">
        <label>Can stash dupes</label>
        <TriSelect value={effCanStash} onChange={setCanStashDupes} />
      </div>

      <div className="dc-field">
        <label>Interior dupes</label>
        <TriSelect value={effInterior} onChange={setInteriorDupes} />
      </div>

      <div className="dc-field">
        <label>Enable AcoustID</label>
        <TriSelect value={effAcoustid} onChange={setEnableAcoustid} />
      </div>

      <div className="dc-field">
        <label>Cover art sanctity</label>
        <SanctitySelect value={effSanctity} onChange={setCoverArtSanctity} />
      </div>

      <div className="dc-field">
        <label>Path schema</label>
        <input
          type="text"
          value={effSchema}
          onChange={(e) => setPathSchema(e.target.value)}
          placeholder="$ARTIST/$ALBUM/$TITLE"
        />
      </div>

      <div className="dc-field">
        <label>Pinned release</label>
        <input
          type="text"
          value={effPinned}
          onChange={(e) => setPinnedRelease(e.target.value)}
          placeholder="MusicBrainz release ID"
        />
      </div>

      <div className="dc-buttons">
        <button onClick={handleSave} disabled={saving || !fullConfig}>
          {saving ? "Saving..." : "Stage"}
        </button>
        <button className="cfg-toolbar__discard" onClick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  );
}
