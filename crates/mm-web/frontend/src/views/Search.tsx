import { useState, useDeferredValue } from "react";
import { useSearchCorpus } from "../api/queries";
import type { SearchResult } from "../api/generated/types";

function ResultRow({ result }: { result: SearchResult }) {
  const tags = [result.artist, result.album, result.title]
    .filter(Boolean)
    .join(" \u2013 ");

  return (
    <div className="search-result">
      <span className="search-result__path">{result.path}</span>
      {tags && <span className="search-result__tags">{tags}</span>}
    </div>
  );
}

export function Search() {
  const [input, setInput] = useState("");
  const query = useDeferredValue(input);
  const { data, isLoading, error } = useSearchCorpus(query);

  return (
    <div className="search-view">
      <div className="search-bar">
        <input
          type="text"
          className="search-input"
          placeholder="Search corpus (path, artist, album, title)..."
          value={input}
          onChange={(e) => setInput(e.target.value)}
          autoFocus
        />
      </div>
      <div className="search-results">
        {isLoading && query && (
          <div className="view-loading">Searching...</div>
        )}
        {error && <div className="view-error">{error.message}</div>}
        {data && (
          <>
            <div className="search-summary">
              {data.length} result{data.length !== 1 ? "s" : ""}
            </div>
            {data.map((r) => (
              <ResultRow key={r.inode} result={r} />
            ))}
          </>
        )}
        {!query && (
          <div className="view-placeholder">Type to search</div>
        )}
      </div>
    </div>
  );
}
