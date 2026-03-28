import { Outlet } from "react-router-dom";
import { NavBar } from "./NavBar";
import { StatusBar } from "./StatusBar";
import { useWitchStatus } from "../hooks/useWitchStatus";

export function Layout() {
  const { status, wsState } = useWitchStatus();

  return (
    <div className="app-shell">
      <header className="app-header">
        <span className="app-title">Music Magic</span>
        <NavBar />
      </header>
      <main className="app-content">
        <Outlet />
      </main>
      <StatusBar status={status} wsState={wsState} />
    </div>
  );
}
