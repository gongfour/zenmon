import { useCallback, useEffect, useState } from "react";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as autostartEnabled,
} from "@tauri-apps/plugin-autostart";

import {
  getConfig,
  getStatus,
  hideSettings,
  newProfile,
  onCaptureStatus,
  openStoreFolder,
  saveConfig,
  toggleCapture,
  type AppConfig,
  type CaptureStatus,
  type Profile,
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

export function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [saved, setSaved] = useState<AppConfig | null>(null);
  const [editing, setEditing] = useState(0);
  const [status, setStatus] = useState<CaptureStatus | null>(null);
  const [launchOnLogin, setLaunchOnLogin] = useState(false);
  const [theme, setTheme] = useState<ThemeMode>(loadMode());
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

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
    const unlisten = onCaptureStatus(setStatus);
    return () => {
      unlisten.then((f) => f()).catch(() => {});
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
    // A rename has to carry the selection with it. The backend only sees the
    // finished config and can't tell a rename from a delete-plus-add, so it
    // falls back to the first profile — selection would silently jump.
    const renamingSelected =
      patch.name !== undefined && config.profiles[editing]?.name === config.app.selected_profile;
    setConfig({
      ...config,
      profiles: config.profiles.map((p, i) => (i === editing ? { ...p, ...patch } : p)),
      app: renamingSelected
        ? { ...config.app, selected_profile: patch.name as string }
        : config.app,
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

  // Capture always runs on the *saved* config, so starting with edits still in
  // the form would silently use the old endpoint. Commit them first; stopping
  // needs no save, and saving there would restart the capture just to end it.
  const onToggleCapture = async () => {
    try {
      if (!capturing && dirty) {
        await saveConfig(config);
        await load();
      }
      await toggleCapture();
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
    // Adding a profile means intending to use it. Leaving the selection behind
    // makes capture run on the old profile even after the new one is saved.
    setConfig({
      ...config,
      profiles: [...config.profiles, created],
      app: { ...config.app, selected_profile: created.name },
    });
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
                <button className="btn sm" onClick={() => openStoreFolder().catch((e) => setError(String(e)))}>
                  Open folder
                </button>
              </div>

              <div className="row">
                <label>Output directory</label>
                <div className="inp wide">
                  <input
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
                  onClick={() => onThemeChange(m)}
                >
                  {m}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>

      <div className="footbar">
        <button className="btn" onClick={onToggleCapture}>
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
        {dirty && <span className="unsaved">unsaved — starting capture applies them</span>}
        <button className="btn" onClick={() => hideSettings()}>
          Close
        </button>
        <button className="btn" disabled={!dirty} onClick={() => saved && setConfig(saved)}>
          Revert
        </button>
        <button className="btn primary" disabled={!dirty} onClick={onSave}>
          Save
        </button>
      </div>

      {confirmDelete && profile && (
        <div className="modal-scrim" onClick={() => setConfirmDelete(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">Delete profile</div>
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
