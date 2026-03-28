import { useCallback, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useConfig, useConfigKdl, queryKeys } from "../api/queries";
import { post } from "../api/client";
import { CONFIG_GROUPS, type FieldDef, type GroupDef } from "./config-fields";

// -- Nested object helpers --

function getPath(obj: unknown, path: string): unknown {
  let cur = obj;
  for (const key of path.split(".")) {
    if (cur == null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[key];
  }
  return cur;
}

function setPath(obj: unknown, path: string, value: unknown): unknown {
  const clone = structuredClone(obj);
  const keys = path.split(".");
  let cur = clone as Record<string, unknown>;
  for (let i = 0; i < keys.length - 1; i++) {
    const k = keys[i]!;
    if (cur[k] == null || typeof cur[k] !== "object") cur[k] = {};
    cur = cur[k] as Record<string, unknown>;
  }
  cur[keys[keys.length - 1]!] = value;
  return clone;
}

// -- Duration formatting --

function formatDuration(secs: number): string {
  if (secs >= 3600 && secs % 3600 === 0) return `${secs / 3600}h`;
  if (secs >= 60 && secs % 60 === 0) return `${secs / 60}m`;
  return `${secs}s`;
}

function parseDuration(s: string): number | null {
  const m = /^(\d+(?:\.\d+)?)\s*(h|m|s)?$/.exec(s.trim());
  if (!m) return null;
  const n = parseFloat(m[1]!);
  switch (m[2]) {
    case "h": return Math.round(n * 3600);
    case "m": return Math.round(n * 60);
    default: return Math.round(n);
  }
}

// -- BoolGrid helpers --

type RoutingMap = Record<string, Record<string, boolean>>;

function routingToRows(
  routing: RoutingMap,
  columns: string[],
): [string, boolean[]][] {
  return Object.entries(routing).map(([name, vals]) => [
    name,
    columns.map((c) => !!(vals as Record<string, boolean>)[`to_${c}`]),
  ]);
}

function rowsToRouting(
  rows: [string, boolean[]][],
  columns: string[],
): RoutingMap {
  const out: RoutingMap = {};
  for (const [name, vals] of rows) {
    const entry: Record<string, boolean> = {};
    for (let i = 0; i < columns.length; i++) entry[`to_${columns[i]!}`] = !!vals[i];
    out[name] = entry;
  }
  return out;
}

// -- Field renderers --

function BoolField({
  value,
  onChange,
}: {
  value: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      className={`cfg-bool ${value ? "cfg-bool--on" : "cfg-bool--off"}`}
      onClick={() => onChange(!value)}
      type="button"
    >
      {value ? "Yes" : "No"}
    </button>
  );
}

function OptionalUintField({
  value,
  onChange,
}: {
  value: number | null;
  onChange: (v: number | null) => void;
}) {
  const [text, setText] = useState(value == null ? "" : String(value));
  return (
    <div className="cfg-optional-uint">
      <input
        type="text"
        value={text}
        placeholder="auto"
        onChange={(e) => {
          setText(e.target.value);
          const s = e.target.value.trim();
          if (s === "" || s === "auto") {
            onChange(null);
          } else {
            const n = parseInt(s, 10);
            if (!isNaN(n)) onChange(n);
          }
        }}
      />
    </div>
  );
}

function StringListField({
  value,
  onChange,
}: {
  value: string[];
  onChange: (v: string[]) => void;
}) {
  const [text, setText] = useState(value.join(", "));
  return (
    <input
      type="text"
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        onChange(
          e.target.value
            .split(",")
            .map((s) => s.trim())
            .filter((s) => s.length > 0),
        );
      }}
    />
  );
}

function StringListMapField({
  value,
  onChange,
}: {
  value: Record<string, string[]>;
  onChange: (v: Record<string, string[]>) => void;
}) {
  const entries = Object.entries(value);
  return (
    <div className="cfg-map">
      {entries.map(([key, vals]) => (
        <div key={key} className="cfg-map-row">
          <span className="cfg-map-key">{key}</span>
          <input
            type="text"
            value={vals.join(", ")}
            onChange={(e) => {
              const newVals = e.target.value
                .split(",")
                .map((s) => s.trim())
                .filter((s) => s.length > 0);
              onChange({ ...value, [key]: newVals });
            }}
          />
        </div>
      ))}
    </div>
  );
}

function BoolGridField({
  value,
  columns,
  onChange,
}: {
  value: RoutingMap;
  columns: string[];
  onChange: (v: RoutingMap) => void;
}) {
  const rows = routingToRows(value, columns);
  return (
    <table className="cfg-grid">
      <thead>
        <tr>
          <th />
          {columns.map((c) => (
            <th key={c}>{c}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map(([name, vals], ri) => (
          <tr key={name}>
            <td className="cfg-grid-label">{name}</td>
            {vals.map((v, ci) => (
              <td key={ci}>
                <input
                  type="checkbox"
                  checked={v}
                  onChange={(e) => {
                    const newRows = rows.map(
                      ([n, vs]) => [n, [...vs]] as [string, boolean[]],
                    );
                    newRows[ri]![1][ci] = e.target.checked;
                    onChange(rowsToRouting(newRows, columns));
                  }}
                />
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function DurationField({
  value,
  onChange,
}: {
  value: number;
  onChange: (v: number) => void;
}) {
  const [text, setText] = useState(formatDuration(value));
  return (
    <input
      type="text"
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        const n = parseDuration(e.target.value);
        if (n != null) onChange(n);
      }}
    />
  );
}

function EnumField({
  value,
  options,
  onChange,
}: {
  value: string;
  options: string[];
  onChange: (v: string) => void;
}) {
  return (
    <select
      className="cfg-enum"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      {options.map((o) => (
        <option key={o} value={o}>
          {o}
        </option>
      ))}
    </select>
  );
}

// -- Generic field component --

function ConfigField({
  field,
  value,
  isEdited,
  onChange,
}: {
  field: FieldDef;
  value: unknown;
  isEdited: boolean;
  onChange: (v: unknown) => void;
}) {
  const cls = `cfg-field ${isEdited ? "cfg-field--edited" : ""}`;

  return (
    <div className={cls}>
      <div className="cfg-field__header">
        <span className="cfg-field__label">
          {field.label}
          {field.restartRequired && (
            <span className="cfg-field__restart" title="Requires restart">
              *
            </span>
          )}
        </span>
        {field.description && (
          <span className="cfg-field__desc">{field.description}</span>
        )}
      </div>
      <div className="cfg-field__input">
        {field.type === "bool" && (
          <BoolField
            value={value as boolean}
            onChange={onChange}
          />
        )}
        {field.type === "float" && (
          <input
            type="number"
            step="any"
            value={value as number}
            onChange={(e) => onChange(parseFloat(e.target.value))}
          />
        )}
        {(field.type === "uint" || field.type === "int") && (
          <input
            type="number"
            step="1"
            value={value as number}
            onChange={(e) => onChange(parseInt(e.target.value, 10))}
          />
        )}
        {field.type === "optional-uint" && (
          <OptionalUintField
            value={value as number | null}
            onChange={onChange}
          />
        )}
        {field.type === "string" && (
          <input
            type="text"
            value={(value as string) ?? ""}
            onChange={(e) => onChange(e.target.value)}
          />
        )}
        {(field.type === "string-list" || field.type === "string-set") && (
          <StringListField
            value={(value as string[]) ?? []}
            onChange={onChange}
          />
        )}
        {field.type === "string-list-map" && (
          <StringListMapField
            value={(value as Record<string, string[]>) ?? {}}
            onChange={onChange}
          />
        )}
        {field.type === "duration" && (
          <DurationField
            value={(value as number) ?? 0}
            onChange={onChange}
          />
        )}
        {field.type === "enum" && (
          <EnumField
            value={(value as string) ?? field.options![0]!}
            options={field.options!}
            onChange={onChange}
          />
        )}
        {field.type === "bool-grid" && (
          <BoolGridField
            value={(value as RoutingMap) ?? {}}
            columns={field.boolGridColumns!}
            onChange={onChange}
          />
        )}
      </div>
    </div>
  );
}

// -- Config group component --

function ConfigGroup({
  group,
  opinions,
  edits,
  onEdit,
}: {
  group: GroupDef;
  opinions: unknown;
  edits: Map<string, unknown>;
  onEdit: (path: string, value: unknown) => void;
}) {
  const [collapsed, setCollapsed] = useState(group.collapsed);

  return (
    <section className="cfg-group">
      <button
        className="cfg-group__header"
        onClick={() => setCollapsed(!collapsed)}
        type="button"
      >
        <span className={`dir-arrow ${collapsed ? "" : "dir-arrow--open"}`}>
          {"\u25b6"}
        </span>
        {group.name}
      </button>
      {!collapsed && (
        <div className="cfg-group__fields">
          {group.fields.map((field) => {
            const current = edits.has(field.path)
              ? edits.get(field.path)
              : getPath(opinions, field.path);
            return (
              <ConfigField
                key={field.path}
                field={field}
                value={current}
                isEdited={edits.has(field.path)}
                onChange={(v) => onEdit(field.path, v)}
              />
            );
          })}
        </div>
      )}
    </section>
  );
}

// -- Main config editor --

export function Config() {
  const { data: config, isLoading, error } = useConfig();
  const { data: kdl } = useConfigKdl();
  const queryClient = useQueryClient();

  const [edits, setEdits] = useState<Map<string, unknown>>(new Map());
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const opinions = useMemo(
    () =>
      config && typeof config === "object"
        ? (config as Record<string, unknown>).opinions
        : null,
    [config],
  );

  const handleEdit = useCallback((path: string, value: unknown) => {
    setEdits((prev) => {
      const next = new Map(prev);
      next.set(path, value);
      return next;
    });
    setMessage(null);
  }, []);

  const handleSave = useCallback(async () => {
    if (!config || edits.size === 0) return;
    setSaving(true);
    setMessage(null);

    try {
      // Build new config by applying edits to the opinions subtree
      let newConfig = structuredClone(config);
      for (const [path, value] of edits) {
        newConfig = setPath(newConfig, `opinions.${path}`, value) as Record<
          string,
          unknown
        >;
      }

      await post("/tx/start", { label: "Config edit (web)" });
      await post("/tx/add", {
        key: "ConfigEdit",
        decision: {
          label: "Config edit",
          mutations: [
            {
              ApplyConfigEdits: {
                original_kdl: kdl ?? "",
                old_config: config,
                new_config: newConfig,
              },
            },
          ],
        },
      });
      await post("/tx/confirm", {});

      setEdits(new Map());
      setMessage("Config saved.");
      void queryClient.invalidateQueries({ queryKey: queryKeys.config });
      void queryClient.invalidateQueries({ queryKey: queryKeys.configKdl });
    } catch (err) {
      setMessage(
        `Save failed: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      setSaving(false);
    }
  }, [config, kdl, edits, queryClient]);

  const handleDiscard = useCallback(() => {
    setEdits(new Map());
    setMessage(null);
  }, []);

  if (isLoading) return <div className="view-loading">Loading config...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!opinions) return <div className="view-error">No config loaded</div>;

  return (
    <div className="config-view">
      {edits.size > 0 && (
        <div className="cfg-toolbar">
          <span className="cfg-toolbar__count">
            {edits.size} field{edits.size !== 1 ? "s" : ""} edited
          </span>
          <button onClick={handleSave} disabled={saving}>
            {saving ? "Saving..." : "Save"}
          </button>
          <button
            className="cfg-toolbar__discard"
            onClick={handleDiscard}
            disabled={saving}
          >
            Discard
          </button>
        </div>
      )}
      {message && (
        <div
          className={`cfg-message ${message.startsWith("Save failed") ? "cfg-message--error" : "cfg-message--ok"}`}
        >
          {message}
        </div>
      )}
      {CONFIG_GROUPS.map((group) => (
        <ConfigGroup
          key={group.name}
          group={group}
          opinions={opinions}
          edits={edits}
          onEdit={handleEdit}
        />
      ))}
    </div>
  );
}
