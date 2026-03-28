import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { checkSetup, completeSetup } from "../api/queries";

export function Setup() {
  const navigate = useNavigate();
  const [root, setRoot] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    checkSetup()
      .then((res) => {
        if (!res.needs_setup) {
          navigate("/login");
          return;
        }
        if (res.suggested_root) {
          setRoot(res.suggested_root);
        }
      })
      .catch(() => {
        // Server not reachable — stay on setup page.
      });
  }, [navigate]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setLoading(true);
    try {
      await completeSetup(root, username, password);
      navigate("/login");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Setup failed");
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="login-container">
      <form className="login-form" onSubmit={handleSubmit}>
        <h2>First-Time Setup</h2>
        {error && <div className="form-error">{error}</div>}
        <label>
          Corpus Root
          <input
            type="text"
            value={root}
            onChange={(e) => setRoot(e.target.value)}
            placeholder="/path/to/corpus"
            autoFocus
          />
        </label>
        <label>
          Username
          <input
            type="text"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
        </label>
        <label>
          Password
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </label>
        <button type="submit" disabled={loading}>
          {loading ? "Setting up..." : "Complete Setup"}
        </button>
      </form>
    </div>
  );
}
