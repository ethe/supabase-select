import { useEffect, useRef } from 'react';

interface TerminalLine {
  ts: number;
  type: string;
  text?: string;
  name?: string;
  stdout?: string;
  detail?: any;
}

interface TerminalPanelProps {
  lines: TerminalLine[];
}

export function TerminalPanel({ lines }: TerminalPanelProps) {
  const terminalRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // Auto-scroll to bottom when lines change
    if (terminalRef.current) {
      terminalRef.current.scrollTop = terminalRef.current.scrollHeight;
    }
  }, [lines]);

  const getLineClass = (type: string) => {
    switch (type) {
      case 'user':
        return 'text-cyan-400';
      case 'agent':
        return 'text-green-400';
      case 'tool':
        return 'text-yellow-400';
      case 'compacted':
        return 'text-purple-400';
      case 'error':
        return 'text-red-400';
      default:
        return 'text-zinc-400';
    }
  };

  const formatLine = (line: TerminalLine) => {
    const timestamp = new Date(line.ts * 1000).toISOString().substr(11, 8);
    const typeLabel = `[${line.type.toUpperCase()}]`.padEnd(12);

    if (line.type === 'tool') {
      return `${timestamp} ${typeLabel} ${line.name}: ${line.stdout}`;
    } else if (line.type === 'compacted') {
      return `${timestamp} ${typeLabel} ${JSON.stringify(line.detail)}`;
    } else {
      return `${timestamp} ${typeLabel} ${line.text || ''}`;
    }
  };

  // Filter out any undefined/null lines
  const validLines = lines.filter((line) => line && line.type);

  return (
    <div
      ref={terminalRef}
      className="h-[64vh] overflow-y-auto border border-zinc-800 bg-zinc-950 p-4 font-mono text-sm"
    >
      {validLines.length === 0 ? (
        <div className="text-zinc-600">
          Terminal ready. Select a session and click Open or Replay.
        </div>
      ) : (
        validLines.map((line, idx) => (
          <div key={idx} className={getLineClass(line.type)}>
            {formatLine(line)}
          </div>
        ))
      )}
    </div>
  );
}
