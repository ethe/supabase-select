interface StatusBarProps {
  segmentCount: number;
  checkpointCount: number;
  lastUpdate: string;
}

export function StatusBar({ segmentCount, checkpointCount, lastUpdate }: StatusBarProps) {
  return (
    <div className="border-t border-zinc-800 bg-zinc-950 px-4 py-2 text-xs text-zinc-600">
      segments: {segmentCount} • checkpoints: {checkpointCount} • last update: {lastUpdate}
    </div>
  );
}
