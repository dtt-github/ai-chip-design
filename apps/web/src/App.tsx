import { useEffect, useRef, useState } from "react";
import { BrowserRouter, Link, Route, Routes, useNavigate } from "react-router";
import type { User } from "oidc-client-ts";
import { currentUser, displayName, login, logout, userManager } from "./auth";
import { ProjectsPage } from "./pages/Projects";
import { ReportPage } from "./pages/Report";
import { WorkspacePage } from "./pages/Workspace";

export function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/auth/callback" element={<AuthCallback />} />
        <Route path="*" element={<SignedIn />} />
      </Routes>
    </BrowserRouter>
  );
}

function AuthCallback() {
  const navigate = useNavigate();
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false);
  useEffect(() => {
    // StrictMode runs effects twice; the authorization code is single-use.
    if (started.current) return;
    started.current = true;
    userManager
      .signinRedirectCallback()
      .then((u) => {
        const returnTo = (u.state as { returnTo?: string } | undefined)?.returnTo ?? "/";
        navigate(returnTo.startsWith("/auth/") ? "/" : returnTo, { replace: true });
      })
      .catch((e: Error) => setError(e.message));
  }, [navigate]);
  return (
    <div className="center">
      {error ? (
        <div className="card">
          <p className="error">Sign-in failed: {error}</p>
          <button onClick={() => login("/")}>Try again</button>
        </div>
      ) : (
        <p className="muted">Signing in…</p>
      )}
    </div>
  );
}

function SignedIn() {
  const [user, setUser] = useState<User | null | undefined>(undefined);
  useEffect(() => {
    currentUser().then(setUser);
  }, []);

  if (user === undefined) return null;
  if (user === null) {
    return (
      <div className="center">
        <div className="card welcome">
          <h1>Wafersmith</h1>
          <p className="muted">Open, browser-based chip design on the IHP SG13G2 open PDK.</p>
          <button className="primary" onClick={() => login()}>
            Sign in
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className="shell">
      <header className="topbar">
        <Link to="/" className="brand">
          Wafersmith
        </Link>
        <span className="spacer" />
        <span className="muted">{displayName(user)}</span>
        <button className="ghost" onClick={() => logout()}>
          Sign out
        </button>
      </header>
      <Routes>
        <Route path="/" element={<ProjectsPage />} />
        <Route path="/p/:id" element={<WorkspacePage />} />
        <Route path="/p/:id/report" element={<ReportPage />} />
        <Route path="*" element={<p className="pad">Not found.</p>} />
      </Routes>
    </div>
  );
}
