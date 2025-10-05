interface Segment {
  seq: number;
  path: string;
  first_ts: number;
  last_ts: number;
  lines: number;
  gzip_bytes: number;
}

interface Checkpoint {
  id: string;
  label: string;
  seq: number;
  line_idx: number;
  git: string;
  ts: number;
}

interface SidebarProps {
  sessions: string[];
  selectedSession: string;
  onSessionSelect: (sid: string) => void;
  checkpoints: Checkpoint[];
  selectedCheckpoint: string;
  onCheckpointSelect: (id: string) => void;
  segments: Segment[];
}

export function Sidebar({
  sessions,
  selectedSession,
  onSessionSelect,
  checkpoints,
  selectedCheckpoint,
  onCheckpointSelect,
  segments,
}: SidebarProps) {
  return (
    <div className="flex h-[64vh] flex-col gap-4 overflow-y-auto border border-zinc-800 bg-zinc-950 p-4">
      {/* Sessions List */}
      <div>
        <h3 className="mb-2 text-sm text-zinc-400">Sessions</h3>
        <div className="space-y-1">
          {sessions.map((sid) => (
            <div
              key={sid}
              onClick={() => onSessionSelect(sid)}
              className={`cursor-pointer rounded px-2 py-1 text-xs ${
                sid === selectedSession
                  ? 'bg-blue-900 text-blue-200'
                  : 'text-zinc-400 hover:bg-zinc-800'
              }`}
            >
              {sid}
            </div>
          ))}
        </div>
      </div>

      {/* Checkpoints Timeline */}
      <div>
        <h3 className="mb-2 text-sm text-zinc-400">Checkpoints</h3>
        <div className="space-y-2">
          {checkpoints.length === 0 ? (
            <div className="text-xs text-zinc-600">No checkpoints</div>
          ) : (
            checkpoints.map((cp) => (
              <div
                key={cp.id}
                onClick={() => onCheckpointSelect(cp.id)}
                className={`cursor-pointer rounded border-l-2 px-2 py-1 text-xs ${
                  cp.id === selectedCheckpoint
                    ? 'border-green-500 bg-zinc-800 text-green-200'
                    : 'border-zinc-700 text-zinc-400 hover:bg-zinc-800'
                }`}
              >
                <div className="font-mono">{cp.label}</div>
                <div className="text-zinc-600">
                  seq:{cp.seq} line:{cp.line_idx} git:{cp.git}
                </div>
              </div>
            ))
          )}
        </div>
      </div>

      {/* Segments Table */}
      <div>
        <h3 className="mb-2 text-sm text-zinc-400">Segments</h3>
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="border-b border-zinc-800 text-zinc-500">
                <th className="pb-1 text-left">Seq</th>
                <th className="pb-1 text-left">Lines</th>
                <th className="pb-1 text-left">First TS</th>
                <th className="pb-1 text-left">Last TS</th>
              </tr>
            </thead>
            <tbody>
              {segments.map((seg) => (
                <tr key={seg.seq} className="border-b border-zinc-900 text-zinc-400">
                  <td className="py-1">{seg.seq}</td>
                  <td className="py-1">{seg.lines}</td>
                  <td className="py-1 font-mono text-[10px]">
                    {new Date(seg.first_ts * 1000).toISOString().substr(11, 8)}
                  </td>
                  <td className="py-1 font-mono text-[10px]">
                    {new Date(seg.last_ts * 1000).toISOString().substr(11, 8)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
