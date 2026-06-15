import { useLocation, useNavigate } from "react-router-dom";
import { useWitchStatus } from "../hooks/useWitchStatus";

const TABS = [
  { path: "/", label: "Health" },
  { path: "/files", label: "Files" },
  { path: "/search", label: "Search" },
  { path: "/external", label: "External" },
  { path: "/genres", label: "Genres" },
  { path: "/promote-genres", label: "Promote" },
  { path: "/deploy", label: "Deploy" },
  { path: "/config", label: "Config" },
] as const;

export function NavBar() {
  const location = useLocation();
  const navigate = useNavigate();
  const { status } = useWitchStatus();
  const hasTx = !!status?.transaction;

  return (
    <nav className="navbar">
      {TABS.map((tab) => (
        <button
          key={tab.path}
          className={`nav-tab ${location.pathname === tab.path ? "nav-tab--active" : ""}`}
          onClick={() => navigate(tab.path)}
        >
          {tab.label}
        </button>
      ))}
      {hasTx && (
        <button
          className={`nav-tab nav-tab--tx ${location.pathname === "/tx" ? "nav-tab--active" : ""}`}
          onClick={() => navigate("/tx")}
        >
          TX ({status!.transaction!.decision_count})
        </button>
      )}
    </nav>
  );
}
