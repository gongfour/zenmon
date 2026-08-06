import { useCallback, useEffect, useState } from "react";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as autostartEnabled,
} from "@tauri-apps/plugin-autostart";

import {
  applyTrayUpdate,
  checkUpdates,
  getCliStatus,
  getConfig,
  getStatus,
  getUpdateStatus,
  hideSettings,
  installCli,
  newProfile,
  onCaptureStatus,
  onUpdateStatus,
  openStoreFolder,
  saveConfig,
  toggleCapture,
  updateCli,
  type AppConfig,
  type CaptureStatus,
  type CliStatus,
  type Profile,
  type UpdateStatus,
} from "./api";
import { loadMode, setThemeMode, type ThemeMode } from "./theme";

const MB = 1024 * 1024;

/** Status → pill class + label. Idle and failure are distinct states, so they
 *  get distinct colors rather than a single "not running". */
function pillFor(status: CaptureStatus | null): { cls: string; text: string } {
  switch (status?.state) {
    case "running":
      return { cls: "run", text: "capturing" };
    case "starting":
      return { cls: "warn", text: "connecting" };
    case "failed":
      return { cls: "stop", text: "failed" };
    default:
      return { cls: "idle", text: "stopped" };
  }
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < MB) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / MB).toFixed(1)} MB`;
}

/** One line summarizing the tray updater's state. */
function trayUpdateText(u: UpdateStatus | null): string {
  switch (u?.phase) {
    case "checking":
      return "checking…";
    case "up_to_date":
      return "up to date";
    case "available":
      return `v${u.available_version} available`;
    case "downloading": {
      const got = formatBytes(u.downloaded ?? 0);
      return u.total ? `downloading ${got} / ${formatBytes(u.total)}` : `downloading ${got}`;
    }
    case "installing":
      return "installing — the app restarts by itself";
    case "error":
      return u.message ?? "update check failed";
    default:
      return "not checked yet";
  }
}

/** One line summarizing the CLI install state. */
function cliText(cli: CliStatus | null): string {
  if (!cli) return "…";
  if (!cli.path) {
    return cli.bundled_available
      ? "not on PATH — Install puts the copy bundled with this app on your user PATH"
      : "not installed — only installer builds bundle the CLI";
  }
  const where =
    cli.managed_by === "tray"
      ? "bundled with the tray, updates with it"
      : cli.managed_by === "cargo"
        ? "managed by cargo install — update by rebuilding from source"
        : cli.update_available
          ? "update available"
          : "up to date";
  const pending = cli.pending_path ? " · PATH change pending, open a new terminal" : "";
  return `${cli.path} · ${where}${pending}`;
}

export function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [saved, setSaved] = useState<AppConfig | null>(null);
  const [editing, setEditing] = useState(0);
  const [status, setStatus] = useState<CaptureStatus | null>(null);
  const [launchOnLogin, setLaunchOnLogin] = useState(false);
  const [theme, setTheme] = useState<ThemeMode>(loadMode());
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [update, setUpdate] = useState<UpdateStatus | null>(null);
  const [cli, setCli] = useState<CliStatus | null>(null);
  const [cliMessage, setCliMessage] = useState<string | null>(null);
  const [cliBusy, setCliBusy] = useState(false);
  const [confirmUpdate, setConfirmUpdate] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);

  const load = useCallback(async () => {
    const cfg = await getConfig();
    setConfig(cfg);
    setSaved(cfg);
    const idx = cfg.profiles.findIndex((p) => p.name === cfg.app.selected_profile);
    setEditing(idx >= 0 ? idx : 0);
  }, []);

  useEffect(() => {
    load().catch((e) => setError(String(e)));
    getStatus().then(setStatus).catch(() => {});
    autostartEnabled().then(setLaunchOnLogin).catch(() => {});
    getUpdateStatus().then(setUpdate).catch(() => {});
    getCliStatus().then(setCli).catch(() => {});
    const unlisten = onCaptureStatus(setStatus);
    const unlistenUpdate = onUpdateStatus(setUpdate);
    return () => {
      unlisten.then((f) => f()).catch(() => {});
      unlistenUpdate.then((f) => f()).catch(() => {});
    };
  }, [load]);

  if (!config) {
    return (
      <div className="app">
        <div className="body">
          <div className="note info">
            <span className="msg">Loading configuration…</span>
          </div>
        </div>
      </div>
    );
  }

  const profile: Profile | undefined = config.profiles[editing];
  const dirty = JSON.stringify(config) !== JSON.stringify(saved);
  const pill = pillFor(status);
  const capturing = status?.state === "running" || status?.state === "starting";

  const patchProfile = (patch: Partial<Profile>) => {
    setConfig({
      ...config,
      profiles: config.profiles.map((p, i) => (i === editing ? { ...p, ...patch } : p)),
    });
  };

  const onSave = async () => {
    try {
      await saveConfig(config);
      await load();
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const onAddProfile = async () => {
    const used = new Set(config.profiles.map((p) => p.name));
    let n = config.profiles.length + 1;
    while (used.has(`profile-${n}`)) n += 1;
    const created = await newProfile(`profile-${n}`);
    setConfig({ ...config, profiles: [...config.profiles, created] });
    setEditing(config.profiles.length);
  };

  const onDeleteProfile = () => {
    const remaining = config.profiles.filter((_, i) => i !== editing);
    const selected = config.profiles[editing]?.name === config.app.selected_profile;
    setConfig({
      ...config,
      profiles: remaining,
      app: {
        ...config.app,
        selected_profile: selected ? (remaining[0]?.name ?? "") : config.app.selected_profile,
      },
    });
    setEditing(Math.max(0, Math.min(editing, remaining.length - 1)));
    setConfirmDelete(false);
  };

  const onToggleLaunchOnLogin = async (next: boolean) => {
    try {
      if (next) await enableAutostart();
      else await disableAutostart();
      setLaunchOnLogin(next);
    } catch (e) {
      setError(String(e));
    }
  };

  const onThemeChange = (mode: ThemeMode) => {
    setThemeMode(mode);
    setTheme(mode);
  };

  const onCheckUpdates = () => {
    setCliMessage(null);
    // Failures surface through the update-status events, not the promise.
    checkUpdates().catch(() => {});
    getCliStatus().then(setCli).catch(() => {});
  };

  const onApplyUpdate = (confirmed: boolean) => {
    setConfirmUpdate(false);
    applyTrayUpdate(confirmed).catch((e) => {
      if (String(e).includes("capture-running")) setConfirmUpdate(true);
      else setError(String(e));
    });
  };

  const onInstallCli = async () => {
    try {
      setCli(await installCli());
      setCliMessage("added to your user PATH — new terminals will find `zenmon`");
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const onUpdateCli = async () => {
    setCliBusy(true);
    setCliMessage(null);
    try {
      const res = await updateCli();
      setCliMessage(
        res.installed
          ? `CLI updated ${res.from} → ${res.to}`
          : `nothing to do — installed ${res.current}, available ${res.available}`,
      );
      setCli(await getCliStatus());
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setCliBusy(false);
    }
  };

  const updateBusy =
    update?.phase === "checking" ||
    update?.phase === "downloading" ||
    update?.phase === "installing";

  // Close hides to the tray; edits would sit unsaved in the hidden webview
  // and die with the process. Ask instead of losing them silently.
  const onCloseRequested = () => {
    if (dirty) setConfirmClose(true);
    else hideSettings();
  };

  const onDiscardAndClose = () => {
    if (saved) {
      setConfig(saved);
      setEditing(Math.max(0, Math.min(editing, saved.profiles.length - 1)));
    }
    setConfirmClose(false);
    hideSettings();
  };

  const onSaveAndClose = async () => {
    setConfirmClose(false);
    try {
      await saveConfig(config);
      await load();
      setError(null);
      hideSettings();
    } catch (e) {
      // Validation refused it — stay open with the reason showing.
      setError(String(e));
    }
  };

  return (
    <div className="app">
      <div className="topbar">
        <div className="tb-main">
          <div className="eyebrow">zenmon</div>
          <div className="h1">Capture settings</div>
        </div>
        <div style={{ textAlign: "right", minWidth: 0 }}>
          <span className={`pill ${pill.cls}`}>
            <i />
            {pill.text}
          </span>
          <div className={`statline ${status?.last_error ? "bad" : ""}`}>
            {status?.last_error
              ? status.last_error
              : status && status.state === "running"
                ? `${status.profile_name} · ${status.messages_written} msgs · ${formatBytes(status.bytes_written)}`
                : (config.app.selected_profile || "no profile")}
          </div>
        </div>
      </div>

      <div className="body">
        {error && (
          <div className="note err">
            <span className="msg">{error}</span>
          </div>
        )}

        <div className="pstrip">
          {config.profiles.map((p, i) => (
            <button
              key={`${p.name}-${i}`}
              className={`pchip ${i === editing ? "on" : ""}`}
              onClick={() => setEditing(i)}
            >
              {p.name === config.app.selected_profile && <span className="d" />}
              {p.name}
            </button>
          ))}
          <button className="pchip add" onClick={onAddProfile}>
            + New
          </button>
        </div>

        {profile && (
          <>
            <div className="fieldset">
              <div className="fs-h">
                <span className="lbl">Connection</span>
                <span className="grow" />
                {profile.name !== config.app.selected_profile && (
                  <button
                    className="btn sm"
                    onClick={() =>
                      setConfig({
                        ...config,
                        app: { ...config.app, selected_profile: profile.name },
                      })
                    }
                  >
                    Use this profile
                  </button>
                )}
              </div>

              <div className="row">
                <label>Name</label>
                <div className="inp">
                  <input
                    aria-label="Profile name"
                    value={profile.name}
                    onChange={(e) => patchProfile({ name: e.target.value })}
                  />
                </div>
              </div>

              <div className="row">
                <label>
                  Endpoint
                  <span className="hint">Zenoh router to connect to</span>
                </label>
                <div className="inp">
                  <input
                    aria-label="Endpoint"
                    value={profile.endpoint}
                    onChange={(e) => patchProfile({ endpoint: e.target.value })}
                  />
                </div>
              </div>

              <div className="row">
                <label>
                  Mode
                  <span className="hint">peer needs no router</span>
                </label>
                <div className="seg">
                  {["client", "peer"].map((m) => (
                    <button
                      key={m}
                      className={profile.mode === m ? "on" : ""}
                      aria-pressed={profile.mode === m}
                      onClick={() => patchProfile({ mode: m })}
                    >
                      {m}
                    </button>
                  ))}
                </div>
              </div>

              <div className="row">
                <label>
                  Namespace
                  <span className="hint">optional key prefix</span>
                </label>
                <div className="inp">
                  <input
                    aria-label="Namespace"
                    value={profile.namespace ?? ""}
                    placeholder="(none)"
                    onChange={(e) =>
                      patchProfile({ namespace: e.target.value || null })
                    }
                  />
                </div>
              </div>

              <div className="row">
                <label>
                  Key expression
                  <span className="hint">what to record</span>
                </label>
                <div className="inp">
                  <input
                    aria-label="Key expression"
                    value={profile.key_expr}
                    onChange={(e) => patchProfile({ key_expr: e.target.value })}
                  />
                </div>
              </div>
            </div>

            <div className="fieldset">
              <div className="fs-h">
                <span className="lbl">Storage</span>
                <span className="grow" />
                <button
                  className="btn sm"
                  onClick={() =>
                    openStoreFolder(profile.output_dir).catch((e) => setError(String(e)))
                  }
                >
                  Open folder
                </button>
              </div>

              <div className="row">
                <label>Output directory</label>
                <div className="inp wide">
                  <input
                    aria-label="Output directory"
                    value={profile.output_dir}
                    onChange={(e) => patchProfile({ output_dir: e.target.value })}
                  />
                </div>
              </div>

              <div className="row">
                <label>
                  Rotate size
                  <span className="hint">start a new segment past this size</span>
                </label>
                <div className="num">
                  <input
                    type="number"
                    min={1}
                    aria-label="Rotate size in megabytes"
                    value={Math.round(profile.rotate_size_bytes / MB)}
                    onChange={(e) =>
                      patchProfile({ rotate_size_bytes: Math.max(1, +e.target.value) * MB })
                    }
                  />
                  <span className="unit">MB</span>
                </div>
              </div>

              <div className="row">
                <label>
                  Rotate interval
                  <span className="hint">…or past this age</span>
                </label>
                <div className="num">
                  <input
                    type="number"
                    min={1}
                    aria-label="Rotate interval in minutes"
                    value={Math.round(profile.rotate_interval_secs / 60)}
                    onChange={(e) =>
                      patchProfile({ rotate_interval_secs: Math.max(1, +e.target.value) * 60 })
                    }
                  />
                  <span className="unit">min</span>
                </div>
              </div>

              <div className="row">
                <label>
                  Max total size
                  <span className="hint">delete oldest segments past this</span>
                </label>
                <div className="num">
                  <input
                    type="number"
                    min={1}
                    aria-label="Maximum total size in megabytes"
                    value={Math.round(profile.max_total_size_bytes / MB)}
                    onChange={(e) =>
                      patchProfile({ max_total_size_bytes: Math.max(1, +e.target.value) * MB })
                    }
                  />
                  <span className="unit">MB</span>
                </div>
              </div>

              <div className="row">
                <label>
                  Max age
                  <span className="hint">…or older than this</span>
                </label>
                <div className="num">
                  <input
                    type="number"
                    min={1}
                    aria-label="Maximum age in days"
                    value={Math.round(profile.max_age_secs / 86400)}
                    onChange={(e) =>
                      patchProfile({ max_age_secs: Math.max(1, +e.target.value) * 86400 })
                    }
                  />
                  <span className="unit">days</span>
                </div>
              </div>
            </div>
          </>
        )}

        <div className="fieldset">
          <div className="fs-h">
            <span className="lbl">Preferences</span>
          </div>

          <div className="row">
            <label className="ck">
              <input
                type="checkbox"
                checked={config.app.resume_capture_on_launch}
                onChange={(e) =>
                  setConfig({
                    ...config,
                    app: { ...config.app, resume_capture_on_launch: e.target.checked },
                  })
                }
              />
              Resume capture on launch
            </label>
            <span className="mono" style={{ color: "var(--dim)", fontSize: 11 }}>
              only if it was running at exit
            </span>
          </div>

          <div className="row">
            <label className="ck">
              <input
                type="checkbox"
                checked={launchOnLogin}
                onChange={(e) => onToggleLaunchOnLogin(e.target.checked)}
              />
              Launch zenmon-tray at login
            </label>
            <span className="mono" style={{ color: "var(--dim)", fontSize: 11 }}>
              starts the tray icon, not capture
            </span>
          </div>

          <div className="row">
            <label>Theme</label>
            <div className="seg">
              {(["system", "light", "dark"] as ThemeMode[]).map((m) => (
                <button
                  key={m}
                  className={theme === m ? "on" : ""}
                  aria-pressed={theme === m}
                  onClick={() => onThemeChange(m)}
                >
                  {m}
                </button>
              ))}
            </div>
          </div>
        </div>

        <div className="fieldset">
          <div className="fs-h">
            <span className="lbl">Updates</span>
            <span className="grow" />
            <button className="btn sm" disabled={updateBusy} onClick={onCheckUpdates}>
              Check for updates
            </button>
          </div>

          <div className="row">
            <label>
              zenmon-tray
              <span className="hint">
                {`v${update?.current_version ?? cli?.tray_version ?? "?"}`}
              </span>
            </label>
            <span
              className="mono"
              style={{
                color: update?.phase === "error" ? "var(--stop, #b91c1c)" : "var(--dim)",
                fontSize: 11,
                minWidth: 0,
              }}
            >
              {trayUpdateText(update)}
            </span>
            {update?.phase === "available" && (
              <button className="btn sm primary" onClick={() => onApplyUpdate(false)}>
                Update &amp; restart
              </button>
            )}
          </div>

          <div className="row">
            <label>
              zenmon CLI
              <span className="hint">{cli?.version ? `v${cli.version}` : "not installed"}</span>
            </label>
            <span className="mono" style={{ color: "var(--dim)", fontSize: 11, minWidth: 0 }}>
              {cliText(cli)}
            </span>
            {cli && !cli.path && cli.bundled_available && (
              <button className="btn sm" onClick={onInstallCli}>
                Install CLI
              </button>
            )}
            {cli?.path && cli.managed_by === "external" && cli.update_available && (
              <button className="btn sm" disabled={cliBusy} onClick={onUpdateCli}>
                {cliBusy ? "Updating…" : "Update CLI"}
              </button>
            )}
          </div>

          {cliMessage && (
            <div className="row">
              <span className="mono" style={{ color: "var(--dim)", fontSize: 11 }}>
                {cliMessage}
              </span>
            </div>
          )}
        </div>
      </div>

      <div className="footbar">
        <button
          className="btn"
          onClick={() => toggleCapture().catch((e) => setError(String(e)))}
        >
          {capturing ? "Stop capture" : "Start capture"}
        </button>
        <button
          className="btn danger"
          disabled={config.profiles.length <= 1}
          onClick={() => setConfirmDelete(true)}
        >
          Delete profile
        </button>
        <span className="grow" />
        <button className="btn" onClick={onCloseRequested}>
          Close
        </button>
        <button className="btn" disabled={!dirty} onClick={() => saved && setConfig(saved)}>
          Revert
        </button>
        <button className="btn primary" disabled={!dirty} onClick={onSave}>
          Save
        </button>
      </div>

      {confirmUpdate && (
        <div className="modal-scrim" onClick={() => setConfirmUpdate(false)}>
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="modal-update-title"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-title" id="modal-update-title">Update zenmon-tray</div>
            <div className="modal-body">
              Capture is running. It will be stopped cleanly — the current segment is
              closed — and resumed automatically after the update restarts the app.
            </div>
            <div className="modal-actions">
              <button className="btn" onClick={() => setConfirmUpdate(false)}>
                Cancel
              </button>
              <button className="btn primary" onClick={() => onApplyUpdate(true)}>
                Stop capture &amp; update
              </button>
            </div>
          </div>
        </div>
      )}

      {confirmClose && (
        <div className="modal-scrim" onClick={() => setConfirmClose(false)}>
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="modal-close-title"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-title" id="modal-close-title">Unsaved changes</div>
            <div className="modal-body">
              The settings window closes to the tray, and unsaved edits would be
              lost when the app exits. Save them first?
            </div>
            <div className="modal-actions">
              <button className="btn" onClick={() => setConfirmClose(false)}>
                Keep editing
              </button>
              <button className="btn danger" onClick={onDiscardAndClose}>
                Discard &amp; close
              </button>
              <button className="btn primary" onClick={onSaveAndClose}>
                Save &amp; close
              </button>
            </div>
          </div>
        </div>
      )}

      {confirmDelete && profile && (
        <div className="modal-scrim" onClick={() => setConfirmDelete(false)}>
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="modal-delete-title"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-title" id="modal-delete-title">Delete profile</div>
            <div className="modal-body">
              Remove <span className="strong mono">{profile.name}</span> from the profile
              list? Captured files under its output directory are left untouched.
            </div>
            <div className="modal-actions">
              <button className="btn" onClick={() => setConfirmDelete(false)}>
                Cancel
              </button>
              <button className="btn danger" onClick={onDeleteProfile}>
                Delete
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
