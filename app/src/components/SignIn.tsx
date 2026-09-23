import { useState } from "react";
import { api } from "../api";
import type { AuthStatus } from "../types";
import { openExternal } from "./Html";

const NEW_TOKEN_URL =
  "https://github.com/settings/tokens/new?scopes=repo,read:org&description=PR%20to%20Go";

export function SignIn({ onSignedIn }: { onSignedIn: (s: AuthStatus) => void }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (f: () => Promise<AuthStatus>) => {
    setBusy(true);
    setError(null);
    try {
      onSignedIn(await f());
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="signin">
      <h1>PR to Go</h1>
      <p className="muted">Review pull requests anywhere, even without a connection.</p>
      <section className="card">
        <h2>Use the GitHub CLI</h2>
        <p>If you're signed in with <code>gh</code>, PR to Go can use the same account.</p>
        <button className="primary" disabled={busy} onClick={() => run(api.signInWithGh)}>
          Sign in with gh
        </button>
      </section>
      <section className="card">
        <h2>Or paste a personal access token</h2>
        <p>
          A classic token needs the <code>repo</code> scope.{" "}
          <a href={NEW_TOKEN_URL} onClick={(e) => (e.preventDefault(), openExternal(NEW_TOKEN_URL))}>
            Create one on GitHub
          </a>
          .
        </p>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run(() => api.signIn(token));
          }}
        >
          <input
            type="password"
            placeholder="ghp_… or github_pat_…"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            autoComplete="off"
          />
          <button type="submit" disabled={busy || !token.trim()}>
            Sign in
          </button>
        </form>
      </section>
      <p className="muted small">The token is kept in your system keychain and only used to talk to GitHub.</p>
      {error && <p className="error">{error}</p>}
    </main>
  );
}
