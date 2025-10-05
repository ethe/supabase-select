interface TopBarProps {
  sessions: string[];
  selectedSession: string;
  onSessionChange: (sid: string) => void;
  checkpoints: Array<{ value: string; label: string }>;
  selectedCheckpoint: string;
  onCheckpointChange: (id: string) => void;
  onOpen: () => void;
  onReplay1x: () => void;
  onReplay4x: () => void;
  isReplaying: boolean;
}

export function TopBar({
  sessions,
  selectedSession,
  onSessionChange,
  checkpoints,
  selectedCheckpoint,
  onCheckpointChange,
  onOpen,
  onReplay1x,
  onReplay4x,
  isReplaying,
}: TopBarProps) {
  return (
    <div className="flex items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4 py-2">
      {/* Traffic lights */}
      <div className="flex gap-2">
        <div className="h-3 w-3 rounded-full bg-red-500"></div>
        <div className="h-3 w-3 rounded-full bg-yellow-500"></div>
        <div className="h-3 w-3 rounded-full bg-green-500"></div>
      </div>

      {/* Controls */}
      <div className="flex items-center gap-3">
        <label className="flex items-center gap-2 text-zinc-400">
          <span className="text-sm">Session</span>
          <select
            value={selectedSession}
            onChange={(e) => onSessionChange(e.target.value)}
            className="rounded border border-zinc-700 bg-zinc-900 px-2 py-1 text-sm text-zinc-200"
          >
            {sessions.map((sid) => (
              <option key={sid} value={sid}>
                {sid}
              </option>
            ))}
          </select>
        </label>

        <label className="flex items-center gap-2 text-zinc-400">
          <span className="text-sm">Checkpoint</span>
          <select
            value={selectedCheckpoint}
            onChange={(e) => onCheckpointChange(e.target.value)}
            className="rounded border border-zinc-700 bg-zinc-900 px-2 py-1 text-sm text-zinc-200"
          >
            {checkpoints.map((cp) => (
              <option key={cp.value} value={cp.value}>
                {cp.label}
              </option>
            ))}
          </select>
        </label>

        <button
          onClick={onOpen}
          disabled={isReplaying}
          className="rounded bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
        >
          Open
        </button>
        <button
          onClick={onReplay1x}
          disabled={isReplaying}
          className="rounded bg-green-600 px-3 py-1 text-sm text-white hover:bg-green-700 disabled:opacity-50"
        >
          Replay x1
        </button>
        <button
          onClick={onReplay4x}
          disabled={isReplaying}
          className="rounded bg-green-600 px-3 py-1 text-sm text-white hover:bg-green-700 disabled:opacity-50"
        >
          x4
        </button>
      </div>

      <div className="w-20"></div> {/* Spacer for balance */}
    </div>
  );
}
