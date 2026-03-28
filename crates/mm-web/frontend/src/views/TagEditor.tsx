import { useCallback, useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { get, post, ApiError } from "../api/client";

interface TagEntry {
  name: string;
  values: string[];
}

interface FileTagState {
  inode: number;
  path: string;
  tags: TagEntry[];
  originalTags: TagEntry[];
}

function tagsToEntries(raw: [string, string][]): TagEntry[] {
  const map = new Map<string, string[]>();
  for (const [name, value] of raw) {
    const key = name.toUpperCase();
    const existing = map.get(key);
    if (existing) {
      existing.push(value);
    } else {
      map.set(key, [value]);
    }
  }
  return Array.from(map, ([name, values]) => ({ name, values }));
}

function diffTags(
  inode: number,
  original: TagEntry[],
  current: TagEntry[],
): unknown[] {
  const ops: unknown[] = [];
  const origMap = new Map(original.map((t) => [t.name, t.values]));
  const curMap = new Map(current.map((t) => [t.name, t.values]));

  // Dropped tags
  for (const [name, vals] of origMap) {
    if (!curMap.has(name)) {
      for (const v of vals) {
        ops.push({ inode, tag_name: name, old_value: v, new_value: null });
      }
    }
  }

  // Added or changed tags
  for (const [name, curVals] of curMap) {
    const origVals = origMap.get(name) ?? [];
    // Simple approach: if values differ, drop all old + add all new
    if (JSON.stringify(origVals) !== JSON.stringify(curVals)) {
      for (const v of origVals) {
        ops.push({ inode, tag_name: name, old_value: v, new_value: null });
      }
      for (const v of curVals) {
        ops.push({ inode, tag_name: name, old_value: null, new_value: v });
      }
    }
  }

  return ops;
}

function TagRow({
  entry,
  onChange,
  onRemove,
}: {
  entry: TagEntry;
  onChange: (name: string, values: string[]) => void;
  onRemove: (name: string) => void;
}) {
  return (
    <div className="te-tag">
      <span className="te-tag__name">{entry.name}</span>
      <div className="te-tag__values">
        {entry.values.map((v, i) => (
          <input
            key={i}
            type="text"
            value={v}
            onChange={(e) => {
              const newVals = [...entry.values];
              newVals[i] = e.target.value;
              onChange(entry.name, newVals);
            }}
          />
        ))}
      </div>
      <button
        className="te-tag__add"
        onClick={() => onChange(entry.name, [...entry.values, ""])}
        title="Add value"
        type="button"
      >
        +
      </button>
      <button
        className="te-tag__remove"
        onClick={() => onRemove(entry.name)}
        title="Remove tag"
        type="button"
      >
        ×
      </button>
    </div>
  );
}

function FileEditor({
  file,
  onChange,
}: {
  file: FileTagState;
  onChange: (updated: FileTagState) => void;
}) {
  const [newTagName, setNewTagName] = useState("");

  const hasChanges =
    JSON.stringify(file.tags) !== JSON.stringify(file.originalTags);

  function handleTagChange(name: string, values: string[]) {
    const updated = file.tags.map((t) =>
      t.name === name ? { ...t, values } : t,
    );
    onChange({ ...file, tags: updated });
  }

  function handleTagRemove(name: string) {
    onChange({ ...file, tags: file.tags.filter((t) => t.name !== name) });
  }

  function handleAddTag() {
    const name = newTagName.trim().toUpperCase();
    if (!name) return;
    if (file.tags.some((t) => t.name === name)) return;
    onChange({ ...file, tags: [...file.tags, { name, values: [""] }] });
    setNewTagName("");
  }

  return (
    <div className={`te-file ${hasChanges ? "te-file--edited" : ""}`}>
      <div className="te-file__path">{file.path}</div>
      <div className="te-tags">
        {file.tags.map((entry) => (
          <TagRow
            key={entry.name}
            entry={entry}
            onChange={handleTagChange}
            onRemove={handleTagRemove}
          />
        ))}
      </div>
      <div className="te-add-row">
        <input
          type="text"
          value={newTagName}
          onChange={(e) => setNewTagName(e.target.value)}
          placeholder="New tag name"
          onKeyDown={(e) => e.key === "Enter" && handleAddTag()}
        />
        <button onClick={handleAddTag} type="button">
          Add Tag
        </button>
      </div>
    </div>
  );
}

interface TagEditorProps {
  inodes: number[];
  label?: string;
}

export function TagEditor({ inodes, label }: TagEditorProps) {
  const navigate = useNavigate();
  const [files, setFiles] = useState<FileTagState[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [staging, setStaging] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    async function load() {
      setLoading(true);
      setError(null);
      try {
        const tagData = await post<[number, [string, string][]][]>(
          "/queries/file-tag-values",
          { inodes, zone: "Corpus" },
        );
        const fileStates: FileTagState[] = tagData.map(([inode, raw]) => {
          const tags = tagsToEntries(raw);
          return {
            inode,
            path: `inode:${inode}`,
            tags,
            originalTags: structuredClone(tags),
          };
        });
        setFiles(fileStates);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    }
    void load();
  }, [inodes]);

  const handleFileChange = useCallback(
    (updated: FileTagState) => {
      setFiles((prev) =>
        prev.map((f) => (f.inode === updated.inode ? updated : f)),
      );
    },
    [],
  );

  async function handleSave() {
    const allOps: unknown[] = [];
    for (const file of files) {
      const ops = diffTags(file.inode, file.originalTags, file.tags);
      allOps.push(...ops);
    }
    if (allOps.length === 0) {
      setMessage("No changes to save.");
      return;
    }

    setStaging(true);
    setMessage(null);
    try {
      try {
        await post("/tx/start", { label: label ?? "Tag edit (web)" });
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 409)) throw e;
      }
      await post("/tx/add", {
        key: { TagEdit: { key_item: `web-${Date.now()}` } },
        decision: {
          label: `Tag edit: ${files.length} file(s), ${allOps.length} ops`,
          mutations: [{ ApplyTagOps: { ops: allOps, zone: "Corpus" } }],
        },
      });
      navigate("/tx");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setStaging(false);
    }
  }

  function handleRevert() {
    setFiles((prev) =>
      prev.map((f) => ({ ...f, tags: structuredClone(f.originalTags) })),
    );
    setMessage(null);
  }

  const totalChanges = files.reduce(
    (sum, f) =>
      sum +
      (JSON.stringify(f.tags) !== JSON.stringify(f.originalTags) ? 1 : 0),
    0,
  );

  if (loading) return <div className="view-loading">Loading tags...</div>;
  if (error) return <div className="view-error">{error}</div>;

  return (
    <div className="te-view">
      <div className="te-toolbar">
        <span>
          {files.length} file{files.length !== 1 ? "s" : ""}
          {totalChanges > 0 ? ` (${totalChanges} modified)` : ""}
        </span>
        <button onClick={handleSave} disabled={staging || totalChanges === 0}>
          {staging ? "Staging..." : "Stage Changes"}
        </button>
        <button
          className="cfg-toolbar__discard"
          onClick={handleRevert}
          disabled={totalChanges === 0}
        >
          Revert
        </button>
      </div>
      {message && (
        <div className={`cfg-message ${message.startsWith("No changes") ? "cfg-message--ok" : "cfg-message--error"}`}>
          {message}
        </div>
      )}
      {files.map((file) => (
        <FileEditor key={file.inode} file={file} onChange={handleFileChange} />
      ))}
    </div>
  );
}

// -- Aggregate (bulk) tag editor for a directory --

interface AggregateTag {
  name: string;
  uniform_value: string | null;
  presence: number;
  value_inodes: [string, number[]][];
}

interface BulkTagAggregate {
  file_count: number;
  inodes: number[];
  dir_label: string;
  tags: AggregateTag[];
}

interface BulkEditState {
  name: string;
  // null = unchanged, string = new uniform value to apply
  editedValue: string | null;
  original: AggregateTag;
}

function BulkTagRow({
  state,
  onChange,
}: {
  state: BulkEditState;
  onChange: (edited: string | null) => void;
}) {
  const tag = state.original;
  const isUniform = tag.uniform_value != null;
  const isEdited = state.editedValue != null;
  const displayValue = state.editedValue ?? tag.uniform_value ?? "";
  const [expanded, setExpanded] = useState(false);

  return (
    <div className={`te-tag ${isEdited ? "te-tag--edited" : ""}`}>
      <span className="te-tag__name">{tag.name}</span>
      <div className="te-tag__values" style={{ flex: 1 }}>
        {isUniform || isEdited ? (
          <input
            type="text"
            value={displayValue}
            onChange={(e) => onChange(e.target.value)}
          />
        ) : (
          <div>
            <button
              className="bulk-various"
              onClick={() => setExpanded(!expanded)}
              type="button"
            >
              Various ({tag.value_inodes.length} values, {tag.presence}/{state.original.presence} files)
            </button>
            {expanded && (
              <div className="bulk-variants">
                {tag.value_inodes.map(([val, inodes]) => (
                  <div
                    key={val}
                    className="bulk-variant"
                    onClick={() => { onChange(val); setExpanded(false); }}
                  >
                    <span className="bulk-variant__val">"{val}"</span>
                    <span className="bulk-variant__count">{inodes.length} files</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
      {isEdited && (
        <button
          className="te-tag__remove"
          onClick={() => onChange(null)}
          title="Revert"
          type="button"
        >
          ↩
        </button>
      )}
    </div>
  );
}

function DirTagEditor({ dirPath }: { dirPath: string }) {
  const navigate = useNavigate();
  const [data, setData] = useState<BulkTagAggregate | null>(null);
  const [edits, setEdits] = useState<Map<string, string>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [staging, setStaging] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    async function load() {
      setLoading(true);
      try {
        const agg = await get<BulkTagAggregate>(
          `/queries/bulk-tag-aggregate?rel_path=${encodeURIComponent(dirPath)}`,
        );
        setData(agg);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    }
    void load();
  }, [dirPath]);

  function handleEdit(tagName: string, value: string | null) {
    setEdits((prev) => {
      const next = new Map(prev);
      if (value === null) {
        next.delete(tagName);
      } else {
        next.set(tagName, value);
      }
      return next;
    });
    setMessage(null);
  }

  async function handleSave() {
    if (!data || edits.size === 0) return;
    setStaging(true);
    setMessage(null);

    try {
      const ops: unknown[] = [];
      for (const [tagName, newValue] of edits) {
        const tag = data.tags.find((t) => t.name === tagName);
        if (!tag) continue;

        // For each distinct old value group, create replace ops
        for (const [oldVal, inodes] of tag.value_inodes) {
          if (oldVal === newValue) continue;
          for (const inode of inodes) {
            ops.push({
              inode,
              tag_name: tagName,
              old_value: oldVal,
              new_value: newValue,
            });
          }
        }

        // For inodes that don't have this tag at all, add it
        if (tag.presence < data.file_count) {
          const coveredInodes = new Set(tag.value_inodes.flatMap(([, inodes]) => inodes));
          for (const inode of data.inodes) {
            if (!coveredInodes.has(inode)) {
              ops.push({
                inode,
                tag_name: tagName,
                old_value: null,
                new_value: newValue,
              });
            }
          }
        }
      }

      if (ops.length === 0) {
        setMessage("No effective changes.");
        setStaging(false);
        return;
      }

      try {
        await post("/tx/start", { label: `Bulk tag edit: ${dirPath}` });
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 409)) throw e;
      }
      await post("/tx/add", {
        key: { TagEdit: { key_item: `bulk-${dirPath}` } },
        decision: {
          label: `Bulk tag edit: ${data.dir_label} (${edits.size} tags, ${ops.length} ops)`,
          mutations: [{ ApplyTagOps: { ops, zone: "Corpus" } }],
        },
      });
      navigate("/tx");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setStaging(false);
    }
  }

  if (loading) return <div className="view-loading">Loading tags...</div>;
  if (error) return <div className="view-error">{error}</div>;
  if (!data) return null;

  const states: BulkEditState[] = data.tags.map((tag) => ({
    name: tag.name,
    editedValue: edits.get(tag.name) ?? null,
    original: tag,
  }));

  return (
    <div className="te-view">
      <div className="te-toolbar">
        <span>
          {data.dir_label} — {data.file_count} files
          {edits.size > 0 ? ` (${edits.size} tags edited)` : ""}
        </span>
        <button onClick={handleSave} disabled={staging || edits.size === 0}>
          {staging ? "Staging..." : "Stage Changes"}
        </button>
        <button
          className="cfg-toolbar__discard"
          onClick={() => { setEdits(new Map()); setMessage(null); }}
          disabled={edits.size === 0}
        >
          Revert
        </button>
      </div>
      {message && (
        <div className={`cfg-message ${message.startsWith("No effective") ? "cfg-message--ok" : "cfg-message--error"}`}>
          {message}
        </div>
      )}
      <div className="te-file">
        <div className="te-tags">
          {states.map((s) => (
            <BulkTagRow
              key={s.name}
              state={s}
              onChange={(v) => handleEdit(s.name, v)}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

// Standalone route wrapper — supports ?inodes=1,2,3 or ?path=some/dir
export function TagEditorRoute() {
  const [searchParams] = useSearchParams();
  const inodesParam = searchParams.get("inodes") ?? "";
  const pathParam = searchParams.get("path");

  if (pathParam) {
    return <DirTagEditor dirPath={pathParam} />;
  }

  const inodes = inodesParam
    .split(",")
    .map(Number)
    .filter((n) => !isNaN(n) && n > 0);

  if (inodes.length === 0) {
    return <div className="view-placeholder">No inodes or path specified</div>;
  }

  return <TagEditor inodes={inodes} />;
}
