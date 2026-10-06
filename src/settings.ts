import {
  type ShuffleState,
  errorText,
  formatDateTime,
  formatTrackCount,
  isValidClientId,
} from './playlist';

interface SettingsData {
  launchAtLogin: boolean;
  clientId: string | null;
  userName: string | null;
  keychainOk: boolean;
}

interface BackupSummary {
  id: string;
  playlistName: string;
  createdAt: number;
  trackCount: number;
}

const CLIENT_ID_HINT = 'Changing it logs you out.';

export interface SettingsView {
  /** Reloads everything the view shows; call it each time the view opens. */
  open(): Promise<void>;
}

// ── DOM glue (runs only in browser) ──────────────────────────────────────────

/** Wires the Settings view of the main window. `onBack` returns to the main flow. */
export async function initSettingsView(
  onBack: () => void,
): Promise<SettingsView> {
  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');

  // Elements
  const btnBack = document.getElementById('btn-back')!;
  const accountName = document.getElementById('settings-account')!;
  const btnLogout = document.getElementById('btn-logout') as HTMLButtonElement;
  const keychainWarning = document.getElementById('keychain-warning')!;
  const launchAtLogin = document.getElementById(
    'launch-at-login',
  ) as HTMLInputElement;
  const launchError = document.getElementById('launch-error')!;
  const backupList = document.getElementById('backup-list')!;
  const restoreStatus = document.getElementById('restore-status')!;
  const clientIdInput = document.getElementById(
    'settings-client-id',
  ) as HTMLInputElement;
  const btnSaveClientId = document.getElementById(
    'btn-settings-save-client-id',
  ) as HTMLButtonElement;
  const clientIdHint = document.getElementById('settings-client-id-hint')!;
  const btnOpenBackups = document.getElementById('btn-open-backups')!;
  const appVersion = document.getElementById('app-version')!;
  const btnUpdate = document.getElementById('btn-update') as HTMLButtonElement;
  const updateStatus = document.getElementById('update-status')!;

  let running = false;
  let pendingRestore: string | null = null;
  let pendingTimer: ReturnType<typeof setTimeout> | undefined;

  function renderBackups(backups: BackupSummary[]): void {
    backupList.innerHTML = '';
    if (backups.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'text-[12px] text-white/40';
      empty.textContent = 'No saved orders yet.';
      backupList.appendChild(empty);
      return;
    }
    for (const b of backups) {
      const row = document.createElement('div');
      row.className =
        'flex items-center justify-between gap-3 px-3 py-2 rounded-lg bg-white/5 border border-white/10';

      const label = document.createElement('span');
      label.className = 'text-[12px] text-white/70 truncate';
      label.textContent = `${formatDateTime(b.createdAt)} · ${formatTrackCount(b.trackCount)}`;

      const btn = document.createElement('button');
      btn.className =
        'shrink-0 text-[11px] text-white/40 hover:text-white/80 cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
      btn.textContent = 'Restore';
      btn.disabled = running;
      btn.addEventListener('click', () => {
        void restore(b.id, btn);
      });

      row.append(label, btn);
      backupList.appendChild(row);
    }
  }

  async function loadBackups(): Promise<void> {
    try {
      renderBackups(await invoke<BackupSummary[]>('list_backups'));
    } catch (e) {
      restoreStatus.textContent = errorText(e);
    }
  }

  /** Two-step confirm: the first click arms the button, the second restores. */
  async function restore(id: string, btn: HTMLButtonElement): Promise<void> {
    if (pendingRestore !== id) {
      pendingRestore = id;
      for (const other of backupList.querySelectorAll('button')) {
        other.textContent = 'Restore';
      }
      btn.textContent = 'Click again to restore';
      clearTimeout(pendingTimer);
      pendingTimer = setTimeout(() => {
        pendingRestore = null;
        btn.textContent = 'Restore';
      }, 4000);
      return;
    }
    clearTimeout(pendingTimer);
    pendingRestore = null;
    restoreStatus.textContent = 'Restoring…';
    try {
      await invoke('restore_backup', { backupId: id });
    } catch (e) {
      restoreStatus.textContent = errorText(e);
    }
  }

  async function open(): Promise<void> {
    launchError.textContent = '';
    restoreStatus.textContent = '';
    const data = await invoke<SettingsData>('get_settings');
    accountName.textContent = `Logged in as ${data.userName ?? 'Spotify user'}`;
    keychainWarning.hidden = data.keychainOk;
    launchAtLogin.checked = data.launchAtLogin;
    clientIdInput.value = data.clientId ?? '';
    clientIdHint.textContent = CLIENT_ID_HINT;
    clientIdHint.classList.remove('text-red-300/90');
    if (!appVersion.textContent) {
      appVersion.textContent = `Shuflo v${await invoke<string>('get_app_version')}`;
    }
    await loadBackups();
  }

  // ── Event wiring ────────────────────────────────────────────────────────────

  btnBack.addEventListener('click', onBack);

  btnLogout.addEventListener('click', () => {
    void (async () => {
      btnLogout.disabled = true;
      try {
        // The main flow takes over once the status changes to logged out.
        await invoke('logout');
      } finally {
        btnLogout.disabled = false;
      }
    })();
  });

  launchAtLogin.addEventListener('change', () => {
    void (async () => {
      launchError.textContent = '';
      try {
        await invoke('set_launch_at_login', { enabled: launchAtLogin.checked });
      } catch (e) {
        launchAtLogin.checked = !launchAtLogin.checked;
        launchError.textContent = errorText(e);
      }
    })();
  });

  btnSaveClientId.addEventListener('click', () => {
    void (async () => {
      const clientId = clientIdInput.value.trim();
      if (!isValidClientId(clientId)) {
        clientIdHint.textContent =
          'A Client ID is 32 letters and numbers. Copy it from your app in the Spotify dashboard.';
        clientIdHint.classList.add('text-red-300/90');
        return;
      }
      btnSaveClientId.disabled = true;
      try {
        await invoke('set_client_id', { clientId });
        clientIdHint.textContent = CLIENT_ID_HINT;
        clientIdHint.classList.remove('text-red-300/90');
        btnSaveClientId.textContent = 'Saved ✓';
        setTimeout(() => {
          btnSaveClientId.textContent = 'Save';
        }, 1500);
      } catch (e) {
        clientIdHint.textContent = errorText(e);
        clientIdHint.classList.add('text-red-300/90');
      } finally {
        btnSaveClientId.disabled = false;
      }
    })();
  });

  btnOpenBackups.addEventListener('click', () => {
    void invoke('open_backups_folder');
  });

  await listen<ShuffleState>('shuffle-state', ({ payload }) => {
    running = payload.state === 'running';
    for (const btn of backupList.querySelectorAll('button')) {
      btn.disabled = running;
    }
    if (payload.kind === 'restore') {
      if (payload.state === 'done') {
        restoreStatus.textContent = 'Previous order restored ✓';
      } else if (payload.state === 'error') {
        restoreStatus.textContent = payload.message ?? 'Restore failed.';
      }
    }
    if (payload.state !== 'running') void loadBackups();
  });

  await listen<{
    state: string;
    version?: string;
    message?: string;
  }>('update-status', ({ payload }) => {
    switch (payload.state) {
      case 'checking':
        btnUpdate.textContent = 'Checking…';
        btnUpdate.disabled = true;
        updateStatus.textContent = '';
        break;
      case 'up-to-date':
        btnUpdate.textContent = 'Up to date ✓';
        setTimeout(() => {
          btnUpdate.textContent = 'Check for Updates';
          btnUpdate.disabled = false;
        }, 2000);
        break;
      case 'downloading':
        btnUpdate.textContent = `Downloading v${payload.version ?? ''}…`;
        btnUpdate.disabled = true;
        break;
      case 'installing':
        btnUpdate.textContent = 'Installing…';
        btnUpdate.disabled = true;
        break;
      case 'error':
        btnUpdate.textContent = 'Check for Updates';
        btnUpdate.disabled = false;
        updateStatus.textContent = `Couldn't check for updates: ${payload.message ?? 'unknown error'}`;
        break;
    }
  });

  btnUpdate.addEventListener('click', () => {
    void invoke('check_update');
  });

  return { open };
}
