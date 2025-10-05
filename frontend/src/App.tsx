import { useEffect, useMemo, useRef, useState } from 'react';
import { TopBar } from './components/TopBar';
import { TerminalPanel } from './components/TerminalPanel';
import { Sidebar } from './components/Sidebar';
import { StatusBar } from './components/StatusBar';

interface ManifestSegment {
  seq: number;
  path: string;
  first_ts: number;
  last_ts: number;
  lines: number;
  bytes_uncompressed?: number;
  bytes_gzip?: number;
}

interface ManifestCheckpoint {
  id: string;
  label?: string;
  seq: number;
  line_idx: number;
  ts: number;
  git?: string;
  branch?: string;
}

interface ManifestPayload {
  version: number;
  sid: string;
  created_at: string;
  updated_at: string;
  active_seq: number;
  segments: ManifestSegment[];
  checkpoints: ManifestCheckpoint[];
}

interface SessionPayload {
  sid: string;
  manifest: ManifestPayload;
}

interface TerminalLine {
  ts: number;
  type: string;
  text?: string;
  name?: string;
  stdout?: string;
  detail?: any;
  [key: string]: unknown;
}

async function fetchSessions(): Promise<SessionPayload[]> {
  const response = await fetch('/api/sessions');
  if (!response.ok) {
    const message = await response.text();
    throw new Error(message || `Failed to load sessions (${response.status})`);
  }
  const data = await response.json();
  return data.sessions ?? [];
}

async function fetchReplayLines(sid: string, seq: number, lineIdx: number, maxLines: number): Promise<TerminalLine[]> {
  const params = new URLSearchParams({
    seq: String(seq),
    line_idx: String(lineIdx),
    max_lines: String(maxLines),
  });
  const response = await fetch(`/api/sessions/${encodeURIComponent(sid)}/replay?${params.toString()}`);
  if (!response.ok) {
    const message = await response.text();
    throw new Error(message || `Failed to load replay (${response.status})`);
  }
  const data = await response.json();
  return data.lines ?? [];
}

export default function App() {
  const [sessions, setSessions] = useState<SessionPayload[]>([]);
  const [selectedSession, setSelectedSession] = useState<string>('');
  const [selectedCheckpoint, setSelectedCheckpoint] = useState<string>('latest');
  const [terminalLines, setTerminalLines] = useState<TerminalLine[]>([]);
  const [isReplaying, setIsReplaying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const replayTimeoutRef = useRef<NodeJS.Timeout | null>(null);

  useEffect(() => {
    let mounted = true;

    const load = async () => {
      try {
        setLoading(true);
        const result = await fetchSessions();
        if (mounted) {
          setSessions(result);
          setError(null);
        }
      } catch (err) {
        if (mounted) {
          setError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (mounted) {
          setLoading(false);
        }
      }
    };

    load();
    const timer = setInterval(load, 15_000);

    return () => {
      mounted = false;
      clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    if (replayTimeoutRef.current) {
      clearTimeout(replayTimeoutRef.current);
      replayTimeoutRef.current = null;
    }
  }, [selectedSession, selectedCheckpoint]);

  useEffect(() => {
    if (sessions.length === 0) {
      setSelectedSession('');
      return;
    }
    setSelectedSession((current) => {
      if (current && sessions.some((s) => s.sid === current)) {
        return current;
      }
      return sessions[0].sid;
    });
  }, [sessions]);

  useEffect(() => {
    setSelectedCheckpoint('latest');
    setTerminalLines([]);
  }, [selectedSession]);

  const currentSession = useMemo(() => sessions.find((s) => s.sid === selectedSession) ?? null, [sessions, selectedSession]);

  const sessionIds = useMemo(() => sessions.map((s) => s.sid), [sessions]);

  const checkpointOptions = useMemo(() => {
    const cps = currentSession?.manifest.checkpoints ?? [];
    const options = cps.map((cp) => ({ value: cp.id, label: cp.label || cp.id }));
    return [{ value: 'latest', label: 'latest' }, ...options];
  }, [currentSession]);

  const checkpoints = currentSession?.manifest.checkpoints ?? [];
  const segments = currentSession?.manifest.segments ?? [];

  const getTarget = () => {
    if (!currentSession) {
      return { seq: 1, lineIdx: 0 };
    }
    if (selectedCheckpoint === 'latest') {
      const cps = currentSession.manifest.checkpoints;
      if (cps.length > 0) {
        const last = cps[cps.length - 1];
        return { seq: last.seq, lineIdx: last.line_idx };
      }
      const lastSeg = currentSession.manifest.segments[currentSession.manifest.segments.length - 1];
      return { seq: lastSeg.seq, lineIdx: Math.max(0, lastSeg.lines - 1) };
    }
    const cp = currentSession.manifest.checkpoints.find((c) => c.id === selectedCheckpoint);
    if (cp) {
      return { seq: cp.seq, lineIdx: cp.line_idx };
    }
    return { seq: 1, lineIdx: 0 };
  };

  const loadLines = async () => {
    if (!currentSession) return [] as TerminalLine[];
    const { seq, lineIdx } = getTarget();
    return fetchReplayLines(currentSession.sid, seq, lineIdx, 5000);
  };

  const handleOpen = async () => {
    if (!currentSession) return;
    if (replayTimeoutRef.current) {
      clearTimeout(replayTimeoutRef.current);
      replayTimeoutRef.current = null;
    }
    setIsReplaying(false);
    try {
      const lines = await loadLines();
      setTerminalLines(lines);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleReplay = async (speed: number) => {
    if (!currentSession) return;
    if (replayTimeoutRef.current) {
      clearTimeout(replayTimeoutRef.current);
      replayTimeoutRef.current = null;
    }
    try {
      const lines = await loadLines();
      setTerminalLines([]);
      setIsReplaying(true);

      const interval = speed === 1 ? 200 : 50;
      let index = 0;

      const play = () => {
        if (index >= lines.length) {
          setIsReplaying(false);
          replayTimeoutRef.current = null;
          return;
        }
        setTerminalLines((prev) => {
          const next = [...prev, lines[index]];
          if (next.length > 5000) {
            return next.slice(-5000);
          }
          return next;
        });
        index += 1;
        replayTimeoutRef.current = setTimeout(play, interval);
      };

      play();
      setError(null);
    } catch (err) {
      setIsReplaying(false);
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    return () => {
      if (replayTimeoutRef.current) {
        clearTimeout(replayTimeoutRef.current);
      }
    };
  }, []);

  const lastUpdate = currentSession?.manifest.updated_at
    ? new Date(currentSession.manifest.updated_at).toLocaleString()
    : 'n/a';

  return (
    <div className="flex h-screen flex-col bg-zinc-950 text-zinc-100">
      <TopBar
        sessions={sessionIds}
        selectedSession={selectedSession}
        onSessionChange={(sid) => setSelectedSession(sid)}
        checkpoints={checkpointOptions}
        selectedCheckpoint={selectedCheckpoint}
        onCheckpointChange={(id) => setSelectedCheckpoint(id)}
        onOpen={handleOpen}
        onReplay1x={() => handleReplay(1)}
        onReplay4x={() => handleReplay(4)}
        isReplaying={isReplaying}
      />

      {error && (
        <div className="bg-red-900 px-4 py-2 text-xs text-red-200">
          {error}
        </div>
      )}
      {loading && (
        <div className="bg-zinc-900 px-4 py-2 text-xs text-zinc-400">Loading sessions…</div>
      )}

      <div className="grid flex-1 grid-cols-[280px_1fr] gap-4 p-4">
        <Sidebar
          sessions={sessionIds}
          selectedSession={selectedSession}
          onSessionSelect={(sid) => setSelectedSession(sid)}
          checkpoints={checkpoints.map((cp) => ({
            ...cp,
            label: cp.label || cp.id,
            git: cp.git || '',
          }))}
          selectedCheckpoint={selectedCheckpoint}
          onCheckpointSelect={(id) => setSelectedCheckpoint(id)}
          segments={segments.map((segment) => ({
            ...segment,
            gzip_bytes: segment.bytes_gzip ?? 0,
          }))}
        />
        <TerminalPanel lines={terminalLines} />
      </div>

      <StatusBar
        segmentCount={segments.length}
        checkpointCount={checkpoints.length}
        lastUpdate={lastUpdate}
      />
    </div>
  );
}
