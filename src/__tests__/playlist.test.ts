import {
  errorText,
  formatDate,
  formatDateTime,
  formatLastShuffled,
  formatNumber,
  formatProgress,
  formatTrackCount,
  isValidClientId,
  maskClientId,
  parsePlaylistId,
  progressPercent,
  stepLabel,
} from '../playlist';
import { describe, expect, it } from 'vitest';

const ID = '37i9dQZF1DXcBWIGoYBM5M';

describe('parsePlaylistId', () => {
  it('accepts a bare ID', () => {
    expect(parsePlaylistId(ID)).toBe(ID);
    expect(parsePlaylistId(`  ${ID}\n`)).toBe(ID);
  });

  it('accepts spotify: URIs, including the legacy user form', () => {
    expect(parsePlaylistId(`spotify:playlist:${ID}`)).toBe(ID);
    expect(parsePlaylistId(`spotify:user:owner:playlist:${ID}`)).toBe(ID);
  });

  it('accepts share links with ?si=, intl prefixes and embeds', () => {
    expect(
      parsePlaylistId(`https://open.spotify.com/playlist/${ID}?si=abc123`),
    ).toBe(ID);
    expect(
      parsePlaylistId(`https://open.spotify.com/intl-fr/playlist/${ID}`),
    ).toBe(ID);
    expect(
      parsePlaylistId(`https://open.spotify.com/embed/playlist/${ID}/`),
    ).toBe(ID);
  });

  it('rejects garbage', () => {
    expect(parsePlaylistId('')).toBeNull();
    expect(parsePlaylistId('hello world')).toBeNull();
    expect(parsePlaylistId('tooShort123')).toBeNull();
    expect(parsePlaylistId(`spotify:album:${ID}`)).toBeNull();
    expect(parsePlaylistId(`https://example.com/playlist/${ID}`)).toBeNull();
    expect(parsePlaylistId(`https://open.spotify.com/album/${ID}`)).toBeNull();
    expect(parsePlaylistId('https://open.spotify.com/playlist/')).toBeNull();
    expect(
      parsePlaylistId('https://open.spotify.com/playlist/not-a-valid-id'),
    ).toBeNull();
  });
});

describe('isValidClientId', () => {
  it('accepts 32 hex characters, ignoring surrounding spaces', () => {
    expect(isValidClientId('0123456789abcdef0123456789ABCDEF')).toBe(true);
    expect(isValidClientId('  0123456789abcdef0123456789abcdef ')).toBe(true);
  });

  it('rejects anything else', () => {
    expect(isValidClientId('')).toBe(false);
    expect(isValidClientId('0123456789abcdef')).toBe(false);
    expect(isValidClientId('0123456789abcdef0123456789abcdeg')).toBe(false);
  });
});

describe('maskClientId', () => {
  it('keeps the first and last four characters', () => {
    expect(maskClientId('0123456789abcdef0123456789abcdef')).toBe('0123…cdef');
  });

  it('leaves short values alone', () => {
    expect(maskClientId('abc')).toBe('abc');
  });
});

describe('stepLabel', () => {
  it('formats the step counter', () => {
    expect(stepLabel(1, 3)).toBe('Step 1 of 3');
  });
});

describe('formatNumber / formatTrackCount', () => {
  it('uses thousands separators', () => {
    expect(formatNumber(1000)).toBe('1,000');
  });

  it('handles 0, singular and plural', () => {
    expect(formatTrackCount(0)).toBe('0 tracks');
    expect(formatTrackCount(1)).toBe('1 track');
    expect(formatTrackCount(1000)).toBe('1,000 tracks');
  });
});

describe('formatProgress', () => {
  it('labels each phase', () => {
    expect(formatProgress({ phase: 'reading', done: 150, total: 1000 })).toBe(
      'Reading… 150 of 1,000',
    );
    expect(formatProgress({ phase: 'writing', done: 400, total: 1000 })).toBe(
      'Shuffling… 400 of 1,000',
    );
    expect(formatProgress({ phase: 'restoring', done: 0, total: 1 })).toBe(
      'Putting it back… 0 of 1',
    );
  });
});

describe('progressPercent', () => {
  it('returns 0 for an empty total', () => {
    expect(progressPercent({ phase: 'reading', done: 0, total: 0 })).toBe(0);
  });

  it('rounds and caps at 100', () => {
    expect(progressPercent({ phase: 'writing', done: 1, total: 3 })).toBe(33);
    expect(progressPercent({ phase: 'writing', done: 5, total: 4 })).toBe(100);
  });
});

describe('formatLastShuffled', () => {
  const now = Date.UTC(2026, 9, 6, 12, 0, 0);
  const minute = 60_000;
  const hour = 60 * minute;

  it('handles never and just now', () => {
    expect(formatLastShuffled(null, now)).toBe('Never shuffled');
    expect(formatLastShuffled(now - 5_000, now)).toBe('Shuffled just now');
    expect(formatLastShuffled(now + 5_000, now)).toBe('Shuffled just now');
  });

  it('uses minutes and hours with singular and plural', () => {
    expect(formatLastShuffled(now - minute, now)).toBe('Shuffled 1 minute ago');
    expect(formatLastShuffled(now - 5 * minute, now)).toBe(
      'Shuffled 5 minutes ago',
    );
    expect(formatLastShuffled(now - hour, now)).toBe('Shuffled 1 hour ago');
    expect(formatLastShuffled(now - 3 * hour, now)).toBe(
      'Shuffled 3 hours ago',
    );
  });

  it('says yesterday, then falls back to a date', () => {
    expect(formatLastShuffled(now - 30 * hour, now)).toBe('Shuffled yesterday');
    const old = now - 5 * 24 * hour;
    expect(formatLastShuffled(old, now)).toBe(`Shuffled on ${formatDate(old)}`);
  });
});

describe('formatDate / formatDateTime', () => {
  it('formats in US English', () => {
    const at = new Date(2026, 9, 6, 9, 5).getTime();
    expect(formatDate(at)).toBe('Oct 6, 2026');
    expect(formatDateTime(at)).toBe('Oct 6, 9:05 AM');
  });
});

describe('errorText', () => {
  it('strips the Error: prefix', () => {
    expect(errorText(new Error('Boom'))).toBe('Boom');
    expect(errorText('Plain message')).toBe('Plain message');
  });
});
