import {
  type PlaylistInfo,
  type Progress,
  type ShuffleState,
  errorText,
  formatLastShuffled,
  formatProgress,
  formatTrackCount,
  isValidClientId,
  maskClientId,
  parsePlaylistId,
  progressPercent,
  stepLabel,
} from './playlist';
import { initSettingsView } from './settings';

interface Status {
  clientIdConfigured: boolean;
  clientId: string | null;
  loggedIn: boolean;
  userName: string | null;
  playlistId: string | null;
  playlist: PlaylistInfo | null;
  playlistError: string | null;
  lastShuffledAt: number | null;
  running: boolean;
}

/** Setup is: Spotify app → connect → choose playlist. */
const SETUP_STEPS = 3;

type View = 'loading' | 'setup' | 'connect' | 'choose' | 'ready' | 'settings';

const CHECK_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/></svg>';
const ALERT_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/></svg>';
const SUCCESS_BANNER = [
  'bg-emerald-500/10',
  'border-emerald-500/25',
  'text-emerald-200',
];
const ERROR_BANNER = ['bg-red-500/10', 'border-red-500/25', 'text-red-200'];

// ── DOM glue (runs only in browser) ──────────────────────────────────────────

async function initMain(): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');
  const { getCurrentWindow } = await import('@tauri-apps/api/window');

  // Elements
  const views: Record<View, HTMLElement> = {
    loading: document.getElementById('view-loading')!,
    setup: document.getElementById('view-setup')!,
    connect: document.getElementById('view-connect')!,
    choose: document.getElementById('view-choose')!,
    ready: document.getElementById('view-ready')!,
    settings: document.getElementById('view-settings')!,
  };
  const footer = document.getElementById('footer')!;
  const account = document.getElementById('account')!;
  const btnSettings = document.getElementById('btn-settings')!;
  const setupStep = document.getElementById('setup-step')!;
  const btnOpenDashboard = document.getElementById('btn-open-dashboard')!;
  const copyButtons =
    document.querySelectorAll<HTMLButtonElement>('button[data-copy]');
  const clientIdInput = document.getElementById(
    'client-id',
  ) as HTMLInputElement;
  const btnSaveClientId = document.getElementById(
    'btn-save-client-id',
  ) as HTMLButtonElement;
  const setupError = document.getElementById('setup-error')!;
  const connectStep = document.getElementById('connect-step')!;
  const btnConnect = document.getElementById(
    'btn-connect',
  ) as HTMLButtonElement;
  const connectError = document.getElementById('connect-error')!;
  const btnConnectBack = document.getElementById('btn-connect-back')!;
  const connectClientId = document.getElementById('connect-client-id')!;
  const btnChangeClientId = document.getElementById('btn-change-client-id')!;
  const chooseStep = document.getElementById('choose-step')!;
  const playlistSelect = document.getElementById(
    'playlist-select',
  ) as HTMLSelectElement;
  const playlistSelectStatus = document.getElementById(
    'playlist-select-status',
  )!;
  const playlistLink = document.getElementById(
    'playlist-link',
  ) as HTMLInputElement;
  const btnUsePlaylist = document.getElementById(
    'btn-use-playlist',
  ) as HTMLButtonElement;
  const btnCancelChoose = document.getElementById('btn-cancel-choose')!;
  const chooseError = document.getElementById('choose-error')!;
  const playlistCover = document.getElementById(
    'playlist-cover',
  ) as HTMLImageElement;
  const playlistName = document.getElementById('playlist-name')!;
  const playlistMeta = document.getElementById('playlist-meta')!;
  const btnChangePlaylist = document.getElementById('btn-change-playlist')!;
  const btnShuffle = document.getElementById(
    'btn-shuffle',
  ) as HTMLButtonElement;
  const progressBox = document.getElementById('progress')!;
  const progressBar = document.getElementById('progress-bar')!;
  const progressText = document.getElementById('progress-text')!;
  const resultBox = document.getElementById('result')!;
  const resultIcon = document.getElementById('result-icon')!;
  const resultText = document.getElementById('result-text')!;
  const btnOpenSpotify = document.getElementById('btn-open-spotify')!;

  let status: Status | null = null;
  let choosing = false;
  let editingClientId = false;
  let settingsOpen = false;
  let running = false;
  let lastRefresh = 0;

  function show(view: View): void {
    for (const [name, el] of Object.entries(views)) {
      el.hidden = name !== view;
    }
  }

  function setRunning(value: boolean): void {
    running = value;
    btnShuffle.disabled = running || !status?.playlist;
    btnShuffle.textContent = running ? 'Shuffling…' : 'Shuffle playlist';
    btnChangePlaylist.hidden = running;
    if (running) {
      resultBox.hidden = true;
      progressBox.hidden = false;
    } else {
      progressBox.hidden = true;
      progressBar.style.width = '0%';
    }
  }

  /** Fills an error banner, or hides it when `message` is empty. */
  function setError(banner: HTMLElement, message: string): void {
    banner.querySelector('p')!.textContent = message;
    banner.hidden = message === '';
  }

  function showResult(text: string, ok: boolean): void {
    resultBox.classList.remove(...SUCCESS_BANNER, ...ERROR_BANNER);
    resultBox.classList.add(...(ok ? SUCCESS_BANNER : ERROR_BANNER));
    resultIcon.innerHTML = ok ? CHECK_ICON : ALERT_ICON;
    resultText.textContent = text;
    btnOpenSpotify.hidden = !ok;
    resultBox.hidden = false;
  }

  async function loadPlaylists(): Promise<void> {
    playlistSelect.innerHTML = '';
    playlistSelect.disabled = true;
    playlistSelectStatus.textContent = 'Loading your playlists…';
    try {
      const playlists = await invoke<PlaylistInfo[]>('list_playlists');
      for (const p of playlists) {
        const opt = document.createElement('option');
        opt.value = p.id;
        opt.textContent = `${p.name} (${formatTrackCount(p.total)})`;
        if (p.id === status?.playlistId) opt.selected = true;
        playlistSelect.appendChild(opt);
      }
      playlistSelect.disabled = playlists.length === 0;
      playlistSelectStatus.textContent =
        playlists.length === 0
          ? 'This account has no playlists it can reorder. Paste a link instead.'
          : '';
    } catch (e) {
      playlistSelectStatus.textContent = errorText(e);
    }
  }

  function render(): void {
    if (!status) return;
    account.textContent = `Logged in as ${status.userName ?? 'Spotify user'}`;

    // Settings waits until setup is done.
    const setUp = status.loggedIn && status.playlistId !== null;
    if (!setUp) settingsOpen = false;
    footer.hidden = !setUp || settingsOpen || choosing;

    if (settingsOpen) {
      show('settings');
      return;
    }

    if (!status.clientIdConfigured || editingClientId) {
      setupStep.textContent = stepLabel(1, SETUP_STEPS);
      // Keep what the user is typing when a refresh lands mid-edit.
      if (!views.setup.hidden) return;
      clientIdInput.value = status.clientId ?? '';
      setError(setupError, '');
      show('setup');
      return;
    }

    if (!status.loggedIn) {
      connectStep.textContent = stepLabel(2, SETUP_STEPS);
      connectClientId.textContent = maskClientId(status.clientId ?? '');
      show('connect');
      return;
    }

    if (choosing || !status.playlistId) {
      chooseStep.textContent = stepLabel(3, SETUP_STEPS);
      chooseStep.hidden = status.playlistId !== null;
      btnCancelChoose.hidden = status.playlistId === null;
      if (!views.choose.hidden) return;
      setError(chooseError, '');
      playlistLink.value = '';
      show('choose');
      void loadPlaylists();
      return;
    }

    show('ready');
    const imageUrl = status.playlist?.imageUrl ?? null;
    if (!imageUrl) {
      playlistCover.hidden = true;
      playlistCover.removeAttribute('src');
    } else if (playlistCover.src !== imageUrl) {
      playlistCover.hidden = true;
      playlistCover.src = imageUrl;
    }
    if (status.playlist) {
      playlistName.textContent = status.playlist.name;
      playlistMeta.textContent = `${formatTrackCount(status.playlist.total)} · ${formatLastShuffled(status.lastShuffledAt, Date.now())}`;
    } else {
      playlistName.textContent = 'Playlist unavailable';
      playlistMeta.textContent = status.playlistError ?? '';
    }
    setRunning(running || status.running);
  }

  async function refresh(): Promise<void> {
    lastRefresh = Date.now();
    try {
      status = await invoke<Status>('get_status');
    } catch (e) {
      show('ready');
      showResult(errorText(e), false);
      return;
    }
    render();
  }

  // ── Event wiring ────────────────────────────────────────────────────────────

  const settingsView = await initSettingsView(() => {
    settingsOpen = false;
    render();
  });

  function openSettings(): void {
    if (!status?.loggedIn || status.playlistId === null) return;
    settingsOpen = true;
    render();
    void settingsView.open();
  }

  btnSettings.addEventListener('click', openSettings);

  // The note icon behind the cover shows until it loads, and stays if it fails.
  playlistCover.addEventListener('load', () => {
    playlistCover.hidden = false;
  });

  btnOpenDashboard.addEventListener('click', () => {
    void invoke('open_spotify_dashboard');
  });

  for (const btn of copyButtons) {
    btn.addEventListener('click', () => {
      void (async () => {
        try {
          await navigator.clipboard.writeText(btn.dataset.copy ?? '');
          btn.textContent = 'Copied ✓';
        } catch {
          btn.textContent = 'Select it to copy';
        }
        setTimeout(() => {
          btn.textContent = 'Copy';
        }, 1500);
      })();
    });
  }

  async function saveClientId(): Promise<void> {
    const clientId = clientIdInput.value.trim();
    if (!isValidClientId(clientId)) {
      setError(
        setupError,
        'A Client ID is 32 letters and numbers. Copy it from your app in the dashboard.',
      );
      return;
    }
    btnSaveClientId.disabled = true;
    setError(setupError, '');
    try {
      await invoke('set_client_id', { clientId });
      editingClientId = false;
      await refresh();
    } catch (e) {
      setError(setupError, errorText(e));
    } finally {
      btnSaveClientId.disabled = false;
    }
  }

  btnSaveClientId.addEventListener('click', () => {
    void saveClientId();
  });

  clientIdInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') void saveClientId();
  });

  function backToClientId(): void {
    editingClientId = true;
    setError(connectError, '');
    render();
  }

  btnConnectBack.addEventListener('click', backToClientId);
  btnChangeClientId.addEventListener('click', backToClientId);

  btnConnect.addEventListener('click', () => {
    void (async () => {
      btnConnect.disabled = true;
      btnConnect.textContent = 'Waiting for the browser…';
      setError(connectError, '');
      try {
        await invoke('login');
      } catch (e) {
        setError(connectError, errorText(e));
      } finally {
        btnConnect.textContent = 'Connect Spotify';
        btnConnect.disabled = false;
      }
    })();
  });

  btnUsePlaylist.addEventListener('click', () => {
    void (async () => {
      setError(chooseError, '');
      const link = playlistLink.value.trim();
      const id = link ? parsePlaylistId(link) : playlistSelect.value;
      if (!id) {
        setError(
          chooseError,
          link
            ? "That doesn't look like a Spotify playlist link."
            : 'Pick a playlist or paste a link.',
        );
        return;
      }
      btnUsePlaylist.disabled = true;
      try {
        await invoke('set_playlist', { id });
        choosing = false;
        resultBox.hidden = true;
        await refresh();
      } catch (e) {
        setError(chooseError, errorText(e));
      } finally {
        btnUsePlaylist.disabled = false;
      }
    })();
  });

  btnCancelChoose.addEventListener('click', () => {
    choosing = false;
    render();
  });

  btnChangePlaylist.addEventListener('click', () => {
    choosing = true;
    render();
  });

  btnShuffle.addEventListener('click', () => {
    void (async () => {
      setRunning(true);
      progressText.textContent = 'Starting…';
      try {
        await invoke('shuffle');
      } catch (e) {
        // Run failures also arrive as a shuffle-state event; this covers calls
        // rejected before the run starts.
        showResult(errorText(e), false);
      } finally {
        // The call returns when the run is over, whatever events said meanwhile.
        setRunning(false);
      }
    })();
  });

  btnOpenSpotify.addEventListener('click', () => {
    void invoke('open_playlist');
  });

  await listen<Progress>('shuffle-progress', ({ payload }) => {
    progressBox.hidden = false;
    progressBar.style.width = `${progressPercent(payload)}%`;
    progressText.textContent = formatProgress(payload);
  });

  await listen<ShuffleState>('shuffle-state', ({ payload }) => {
    if (payload.state === 'running') {
      setRunning(true);
      progressText.textContent = 'Starting…';
      return;
    }
    setRunning(false);
    if (payload.state === 'done') {
      showResult(
        payload.kind === 'restore'
          ? 'Previous order restored'
          : 'Playlist shuffled',
        true,
      );
    } else {
      showResult(payload.message ?? 'Something went wrong.', false);
    }
    void refresh();
  });

  await listen('status-changed', () => {
    void refresh();
  });

  // Tray → Settings.
  await listen('show-settings', openSettings);

  // Closing the window is held back while Shuflo is writing to the playlist.
  await listen('close-blocked', () => {
    // The banner and progress live on the playlist screen.
    settingsOpen = false;
    render();
    showResult(
      'Shuflo is still updating the playlist. Close it once it has finished.',
      false,
    );
  });

  // Reopening from the tray should show fresh data, without hitting Spotify on every focus.
  await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused && !running && Date.now() - lastRefresh > 30_000) {
      void refresh();
    }
  });

  // ── Initial load ────────────────────────────────────────────────────────────

  await refresh();
}

if (typeof document !== 'undefined') {
  void initMain();
}
