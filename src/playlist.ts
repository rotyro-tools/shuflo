// ── Shared types (mirror the Rust structs serialized over IPC) ────────────────

export interface PlaylistInfo {
  id: string;
  name: string;
  ownerId: string;
  collaborative: boolean;
  snapshotId: string;
  total: number;
  imageUrl: string | null;
}

export type Phase = 'reading' | 'writing' | 'restoring';

export interface Progress {
  phase: Phase;
  done: number;
  total: number;
}

export interface ShuffleState {
  state: 'running' | 'done' | 'error';
  kind: 'shuffle' | 'restore';
  message?: string;
}

// ── Pure helpers (exported for unit tests) ────────────────────────────────────

const PLAYLIST_ID = /^[A-Za-z0-9]{22}$/;
const PLAYLIST_URI = /^spotify:(?:user:[^:]+:)?playlist:([A-Za-z0-9]{22})$/;

/**
 * Extracts a playlist ID from a share link (`?si=` and `intl-xx` paths included),
 * a `spotify:playlist:` URI or a bare ID. Returns null for anything else.
 */
export function parsePlaylistId(input: string): string | null {
  const value = input.trim();
  if (PLAYLIST_ID.test(value)) return value;

  const uri = PLAYLIST_URI.exec(value);
  if (uri) return uri[1];

  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (url.hostname !== 'open.spotify.com') return null;
  const segments = url.pathname.split('/').filter(Boolean);
  const id = segments[segments.indexOf('playlist') + 1] ?? '';
  return segments.includes('playlist') && PLAYLIST_ID.test(id) ? id : null;
}

const CLIENT_ID = /^[0-9a-f]{32}$/i;

/** Spotify Client IDs are 32 hex characters. */
export function isValidClientId(value: string): boolean {
  return CLIENT_ID.test(value.trim());
}

/** Shortens a Client ID for display, e.g. "1a2b…9f0e". */
export function maskClientId(id: string): string {
  return id.length > 8 ? `${id.slice(0, 4)}…${id.slice(-4)}` : id;
}

export function stepLabel(step: number, total: number): string {
  return `Step ${step} of ${total}`;
}

export function formatNumber(n: number): string {
  return n.toLocaleString('en-US');
}

export function formatTrackCount(n: number): string {
  return `${formatNumber(n)} ${n === 1 ? 'track' : 'tracks'}`;
}

const PHASE_LABELS: Record<Phase, string> = {
  reading: 'Reading',
  writing: 'Shuffling',
  restoring: 'Putting it back',
};

export function formatProgress(progress: Progress): string {
  const label = PHASE_LABELS[progress.phase];
  return `${label}… ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
}

/** Share of the current phase that is done, as a whole percentage (0–100). */
export function progressPercent(progress: Progress): number {
  if (progress.total <= 0) return 0;
  return Math.min(100, Math.round((progress.done / progress.total) * 100));
}

function plural(n: number, unit: string): string {
  return `${n} ${unit}${n === 1 ? '' : 's'}`;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "Shuffled 5 minutes ago" style label for the last shuffle time (epoch ms). */
export function formatLastShuffled(at: number | null, now: number): string {
  if (at === null) return 'Never shuffled';
  const elapsed = Math.max(0, now - at);
  if (elapsed < MINUTE) return 'Shuffled just now';
  if (elapsed < HOUR)
    return `Shuffled ${plural(Math.floor(elapsed / MINUTE), 'minute')} ago`;
  if (elapsed < DAY)
    return `Shuffled ${plural(Math.floor(elapsed / HOUR), 'hour')} ago`;
  if (elapsed < 2 * DAY) return 'Shuffled yesterday';
  return `Shuffled on ${formatDate(at)}`;
}

export function formatDate(at: number): string {
  return new Date(at).toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  });
}

export function formatDateTime(at: number): string {
  return new Date(at).toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

/** Strips the "Error: " prefix Tauri adds to rejected invoke() calls. */
export function errorText(e: unknown): string {
  return String(e).replace(/^Error:\s*/, '');
}
