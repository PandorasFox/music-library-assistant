import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useDirectoryListing } from "../api/queries";
import type { DirectoryListingEntry } from "../api/generated/types";
import { DirConfigPanel } from "./DirConfig";

function formatDuration(ms: number): string {
  const totalSec = Math.round(ms / 1000);
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${min}:${sec.toString().padStart(2, "0")}`;
}

function FileEntry({ entry }: { entry: DirectoryListingEntry }) {
  const meta: string[] = [];
  if (entry.duration_ms != null) meta.push(formatDuration(entry.duration_ms));
  if (entry.bitrate_kbps != null) meta.push(`${entry.bitrate_kbps}kbps`);

  return (
    <div className="file-entry">
      <span className="file-name">{entry.name}</span>
      {meta.length > 0 && (
        <span className="file-meta">{meta.join(" \u00b7 ")}</span>
      )}
    </div>
  );
}

function DirEntry({
  entry,
  depth,
}: {
  entry: DirectoryListingEntry;
  depth: number;
}) {
  const [expanded, setExpanded] = useState(false);
  const [configOpen, setConfigOpen] = useState(false);
  const navigate = useNavigate();

  return (
    <div className="dir-entry">
      <div className="dir-header-row" style={{ paddingLeft: `${depth * 16 + 8}px` }}>
        <button
          className="dir-header"
          onClick={() => setExpanded(!expanded)}
        >
          <span className={`dir-arrow ${expanded ? "dir-arrow--open" : ""}`}>
            {"\u25b6"}
          </span>
          <span className="dir-name">{entry.name}</span>
          <span className="dir-chips">
            <span
              className={`dir-chip ${configOpen ? "dir-chip--active" : ""}`}
              onClick={(e) => {
                e.stopPropagation();
                setConfigOpen(!configOpen);
              }}
            >
              Config
            </span>
            <span
              className="dir-chip"
              onClick={(e) => {
                e.stopPropagation();
                navigate(`/tags?path=${encodeURIComponent(entry.path)}`);
              }}
            >
              Bulk Tags
            </span>
          </span>
          {entry.file_count > 0 && (
            <span className="dir-count">{entry.file_count}</span>
          )}
        </button>
      </div>
      {configOpen && (
        <DirConfigPanel path={entry.path} onClose={() => setConfigOpen(false)} />
      )}
      {expanded && <DirectoryLevel parent={entry.path} depth={depth + 1} />}
    </div>
  );
}

function DirectoryLevel({
  parent,
  depth,
}: {
  parent: string | null;
  depth: number;
}) {
  const { data, isLoading, error } = useDirectoryListing(parent);

  if (isLoading) {
    return (
      <div className="dir-loading" style={{ paddingLeft: `${depth * 16 + 24}px` }}>
        Loading...
      </div>
    );
  }

  if (error) {
    return (
      <div className="dir-error" style={{ paddingLeft: `${depth * 16 + 24}px` }}>
        {error.message}
      </div>
    );
  }

  if (!data || data.length === 0) {
    return (
      <div className="dir-empty" style={{ paddingLeft: `${depth * 16 + 24}px` }}>
        Empty
      </div>
    );
  }

  const dirs = data.filter((e) => e.is_dir);
  const files = data.filter((e) => !e.is_dir);

  return (
    <div className="dir-level">
      {dirs.map((entry) => (
        <DirEntry key={entry.path} entry={entry} depth={depth} />
      ))}
      {files.map((entry) => (
        <div key={entry.path} style={{ paddingLeft: `${depth * 16 + 24}px` }}>
          <FileEntry entry={entry} />
        </div>
      ))}
    </div>
  );
}

export function Files() {
  return (
    <div className="files-view">
      <DirectoryLevel parent={null} depth={0} />
    </div>
  );
}
