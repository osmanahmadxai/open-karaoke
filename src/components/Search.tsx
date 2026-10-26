import { useEffect, useRef, useState } from "react";
import { api, errorText, type SongMeta } from "../lib/api";
import { librarySources } from "../lib/library";
import { openSong } from "../lib/loader";
import { formatTime } from "../lib/music";
import { useStore } from "../lib/store";
import { Mic, Play, Search as SearchIcon } from "./Icons";

function Card({ m, instant = false }: { m: SongMeta; instant?: boolean }) {
  return (
    <button className="card" onClick={() => openSong(m)} title={`${m.title} — ${m.artist}`}>
      <div className="art">
        {m.artwork && <img src={m.artwork.replace("600x600", "300x300")} alt="" loading="lazy" />}
        {instant && (
          <span className="instant" title="Already separated: starts instantly">
            ⚡ Instant
          </span>
        )}
        <div className="play-badge">
          <Play />
        </div>
      </div>
      <div className="title">{m.title}</div>
      <div className="sub">
        {m.explicit && <span className="tag">E</span>}
        {m.artist} · {formatTime(m.duration)}
      </div>
    </button>
  );
}

export function Search() {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SongMeta[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const history = useStore((s) => s.history);
  const [instant, setInstant] = useState<Set<string>>(new Set());
  const input = useRef<HTMLInputElement>(null);
  const seq = useRef(0);

  useEffect(() => {
    input.current?.focus();
  }, []);

  // Mark songs whose separation is already on disk.
  useEffect(() => {
    const sources = librarySources([...history.map((h) => h.id), ...(results ?? []).map((r) => r.id)]);
    if (!sources.size) return;
    api
      .stemsCached([...sources.keys()])
      .then((ready) => setInstant(new Set(ready.map((id) => sources.get(id)!))))
      .catch(() => {});
  }, [history, results]);

  useEffect(() => {
    const q = query.trim();
    if (q.length < 2) {
      setResults(null);
      setError(null);
      return;
    }
    const id = ++seq.current;
    const t = setTimeout(async () => {
      setLoading(true);
      try {
        const r = await api.searchSongs(q);
        if (id === seq.current) {
          setResults(r);
          setError(null);
        }
      } catch (e) {
        if (id === seq.current) setError(errorText(e));
      } finally {
        if (id === seq.current) setLoading(false);
      }
    }, 350);
    return () => clearTimeout(t);
  }, [query]);

  const compact = query.trim().length >= 2;

  return (
    <div className="search-view">
      <div className={`search-hero ${compact ? "compact" : ""}`}>
        <h1>
          What are we
          <br />
          <em>singing</em> tonight?
        </h1>
        <div className="searchbox">
          <SearchIcon />
          <input
            ref={input}
            value={query}
            placeholder="Search any song or artist…"
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && results?.[0]) openSong(results[0]);
              if (e.key === "Escape") setQuery("");
            }}
            spellCheck={false}
          />
          {loading && <div className="spinner" style={{ marginRight: 10 }} />}
        </div>
      </div>

      {error && <div className="empty err">{error}</div>}

      {compact && results && !results.length && !loading && <div className="empty">No songs found for “{query.trim()}”.</div>}

      {compact && results && results.length > 0 && (
        <>
          <div className="section-title">Songs</div>
          <div className="results">
            {results.map((m) => (
              <Card key={m.id} m={m} instant={instant.has(m.id)} />
            ))}
          </div>
        </>
      )}

      {compact && !results && loading && (
        <div className="results" style={{ marginTop: 50 }}>
          {Array.from({ length: 10 }, (_, i) => (
            <div key={i} className="card">
              <div className="art skeleton" />
              <div className="skeleton" style={{ height: 12, borderRadius: 6, marginTop: 12, width: "80%" }} />
              <div className="skeleton" style={{ height: 10, borderRadius: 6, marginTop: 8, width: "50%" }} />
            </div>
          ))}
        </div>
      )}

      {!compact && (
        <button className="lab-cta" onClick={() => useStore.setState({ view: "lab" })}>
          <div className="lab-cta-icon">
            <Mic />
          </div>
          <div>
            <b>Voice Lab</b>
            <span>Test your mic, record a take and build your perfect autotune sound before you sing.</span>
          </div>
          <span className="lab-cta-go">Open →</span>
        </button>
      )}

      {!compact && history.length > 0 && (
        <>
          <div className="section-title">Sung recently</div>
          <div className="results">
            {history.map((m) => (
              <Card key={m.id} m={m} instant={instant.has(m.id)} />
            ))}
          </div>
        </>
      )}
    </div>
  );
}
