import {
  createHashRouter,
  Navigate,
  RouterProvider,
} from "react-router-dom";
import { useEffect, useState } from "react";
import { Layout } from "./components/Layout";
import { Login } from "./views/Login";
import { Setup } from "./views/Setup";
import { Health } from "./views/Health";
import { Files } from "./views/Files";
import { Search } from "./views/Search";
import { Config } from "./views/Config";
import { Transaction } from "./views/Transaction";
import { ExternalMatches } from "./views/ExternalMatches";
import { Deploy } from "./views/Deploy";
import { Resolve } from "./views/Resolve";
import { TagEditorRoute } from "./views/TagEditor";
import {
  TagCanonicity,
  CompoundSplit,
  MissingAlbum,
  DiscExtraction,
  ManualReview,
  DirectoryCluster,
  TagCanonicityPicker,
  CompoundPicker,
} from "./views/ComplexResolutions";
import { useAuth } from "./hooks/useAuth";
import { witchSocket } from "./api/ws";
import { get } from "./api/client";
import { useParams } from "react-router-dom";

function TagCanonicityWrapper() {
  const { tagName } = useParams<{ tagName: string }>();
  return <TagCanonicity tagName={tagName ?? ""} />;
}

function CompoundSplitWrapper({ safe }: { safe: boolean }) {
  const { tagName } = useParams<{ tagName: string }>();
  return <CompoundSplit tagName={tagName ?? ""} safe={safe} />;
}

function ManualReviewWrapper() {
  const { kind } = useParams<{ kind: string }>();
  return <ManualReview kind={kind ?? ""} />;
}

function AuthGuard({ children }: { children: React.ReactNode }) {
  const { isAuthenticated, markAuthenticated } = useAuth();
  const [checking, setChecking] = useState(true);

  // On first render, probe the server to see if our cookie is valid.
  useEffect(() => {
    if (isAuthenticated) {
      setChecking(false);
      return;
    }
    get<unknown>("/status")
      .then(() => {
        markAuthenticated();
        witchSocket.connect();
        setChecking(false);
      })
      .catch(() => {
        setChecking(false);
      });
  }, [isAuthenticated, markAuthenticated]);

  if (checking) return null;
  if (!isAuthenticated) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

const router = createHashRouter([
  {
    path: "/login",
    element: <Login />,
  },
  {
    path: "/setup",
    element: <Setup />,
  },
  {
    path: "/",
    element: (
      <AuthGuard>
        <Layout />
      </AuthGuard>
    ),
    children: [
      { index: true, element: <Health /> },
      { path: "files", element: <Files /> },
      { path: "search", element: <Search /> },
      { path: "config", element: <Config /> },
      { path: "external", element: <ExternalMatches /> },
      { path: "deploy", element: <Deploy /> },
      { path: "resolve/:type", element: <Resolve /> },
      { path: "tags", element: <TagEditorRoute /> },
      { path: "resolve/directory-clusters", element: <DirectoryCluster queryKey="directory-cluster-data" queryUrl="/queries/directory-cluster-data" title="Cross-Source Overlaps" /> },
      { path: "resolve/release-overlaps", element: <DirectoryCluster queryKey="release-overlap-data" queryUrl="/queries/release-overlap-data" title="Release Overlaps" /> },
      { path: "resolve/tag-canonicity-picker", element: <TagCanonicityPicker /> },
      { path: "resolve/compound-picker", element: <CompoundPicker /> },
      { path: "resolve/tag-canonicity/:tagName", element: <TagCanonicityWrapper /> },
      { path: "resolve/compound-split/:tagName", element: <CompoundSplitWrapper safe={true} /> },
      { path: "resolve/compound-review/:tagName", element: <CompoundSplitWrapper safe={false} /> },
      { path: "resolve/missing-album", element: <MissingAlbum /> },
      { path: "resolve/disc-extraction", element: <DiscExtraction /> },
      { path: "resolve/manual-review/:kind", element: <ManualReviewWrapper /> },
      { path: "tx", element: <Transaction /> },
    ],
  },
]);

export function App() {
  // WS connection is started by AuthGuard on successful session probe.
  // Cleanup on unmount.
  useEffect(() => {
    return () => witchSocket.disconnect();
  }, []);

  return <RouterProvider router={router} />;
}
