//! Local checkpoints, separate from recordings and named views.
use crate::{app::App, offline_tools::Tools, recording::Recording};
use color_eyre::{eyre::eyre, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zenmon_core::profile::TuiProfile;

const MAX_STATE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    version: u32,
    fingerprint: String,
    pub position: usize,
    pub selected_key: Option<String>,
    pub view: TuiProfile,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub search: Option<(String, Option<String>)>,
}
impl State {
    pub fn snapshot(recording: &Recording, position: usize, app: &App, tools: &Tools) -> Self {
        Self {
            version: 1,
            fingerprint: recording.fingerprint.clone(),
            position,
            selected_key: app.selected_topic_key(),
            view: app.snapshot_profile("offline investigation"),
            start: tools.start,
            end: tools.end,
            search: tools.last_search.clone(),
        }
    }
    fn validate(&self, recording: &Recording) -> Result<bool> {
        if self.version != 1 {
            return Err(eyre!(
                "unsupported investigation state version {}",
                self.version
            ));
        }
        if self.fingerprint != recording.fingerprint {
            return Ok(false);
        }
        if [Some(self.position), self.start, self.end]
            .into_iter()
            .flatten()
            .any(|p| p >= recording.entries.len())
        {
            return Err(eyre!("investigation positions are outside the recording"));
        }
        if let Some((command, key)) = &self.search {
            let (verb, argument) = command
                .split_once(' ')
                .ok_or_else(|| eyre!("invalid saved search"))?;
            if !matches!(
                verb,
                "find" | "find-prev" | "find-all" | "find-all-prev" | "change" | "change-prev"
            ) || argument.trim().is_empty()
                || (verb.contains("all") != key.is_none())
                || key.as_ref().is_some_and(|k| !recording.keys.contains(k))
            {
                return Err(eyre!("invalid saved search"));
            }
        }
        if self
            .selected_key
            .as_ref()
            .is_some_and(|k| !recording.keys.contains(k))
        {
            return Err(eyre!("invalid saved selected key"));
        }
        Ok(true)
    }
}

pub(crate) fn state_path(source: &Path) -> Result<PathBuf> {
    let root = match std::env::var_os("ZENMON_TUI_STATE_DIR") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        Some(_) => return Err(eyre!("ZENMON_TUI_STATE_DIR is empty")),
        None => directories::ProjectDirs::from("", "", "zenmon")
            .ok_or_else(|| eyre!("cannot determine local state directory"))?
            .data_local_dir()
            .join("investigations"),
    };
    let source = source.canonicalize()?;
    let key = Sha256::digest(source.as_os_str().as_encoded_bytes());
    Ok(root.join(format!("{key:x}.json")))
}

fn load(path: &Path, recording: &Recording) -> Result<(Option<State>, bool)> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((None, false)),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(eyre!("investigation state exceeds 1 MiB"));
    }
    let state: State = serde_json::from_slice(&bytes)?;
    if state.validate(recording)? {
        Ok((Some(state), false))
    } else {
        Ok((None, true))
    }
}

fn save(path: &Path, state: &State) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(eyre!("investigation state exceeds 1 MiB"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| eyre!("missing state directory"))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    // Unique temporary names avoid collisions between concurrent viewers.
    // NamedTempFile is owner-only on Unix; the last completed checkpoint wins.
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[derive(Default)]
pub(crate) struct Checkpoints {
    path: Option<PathBuf>,
    saved: Option<State>,
    pending: Option<tokio::task::JoinHandle<Result<()>>>,
}
impl Checkpoints {
    /// A corrupt/future file disables writes for this run instead of destroying it.
    pub fn open(
        source: &Path,
        recording: &Recording,
        enabled: bool,
    ) -> (Self, Option<State>, Option<String>) {
        if !enabled {
            return (Self::default(), None, None);
        }
        let result = (|| {
            let path = state_path(source)?;
            let (state, changed) = load(&path, recording)?;
            Ok::<_, color_eyre::Report>((path, state, changed))
        })();
        match result {
            Ok((path, state, changed)) => (
                Self { path: Some(path), saved: state.clone(), pending: None }, state,
                changed.then(|| "Recording or loaded selection changed; saved investigation was not restored".into()),
            ),
            Err(e) => (Self::default(), None, Some(format!("Investigation persistence disabled: {e}"))),
        }
    }
    pub fn enabled(&self) -> bool {
        self.path.is_some()
    }

    pub async fn checkpoint(&mut self, state: State, finish: bool) -> Result<()> {
        if self
            .pending
            .as_ref()
            .is_some_and(|job| finish || job.is_finished())
        {
            let result = self.pending.take().unwrap().await;
            if let Err(e) = result.map_err(color_eyre::Report::from).and_then(|r| r) {
                self.path = None;
                return Err(e);
            }
        }
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if self.pending.is_some() || self.saved.as_ref() == Some(&state) {
            return Ok(());
        }
        self.saved = Some(state.clone());
        let job = tokio::task::spawn_blocking(move || save(&path, &state));
        if finish {
            job.await??;
        } else {
            self.pending = Some(job);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenmon_core::{capture::CaptureRecord, inspection::Selection};
    fn fixture(path: &Path, changed: bool) {
        let mut file = fs::File::create(path).unwrap();
        for i in 0..4 {
            let r = CaptureRecord {
                schema_version: 1,
                key_expr: if i == 3 { "other/key" } else { "demo/key" }.into(),
                payload_base64: if changed && i == 3 { "MQ==" } else { "MA==" }.into(),
                encoding: "application/json".into(),
                attachment_base64: None,
                source_timestamp: None,
                received_at: None,
                received_offset_ms: i * 1000,
                kind: "PUT".into(),
            };
            writeln!(file, "{}", serde_json::to_string(&r).unwrap()).unwrap();
        }
    }
    fn selected(path: &Path, key: &str) -> Recording {
        Recording::load_selected(
            path,
            &Selection::new(key, None, None).unwrap(),
            100,
            100000,
            |_| Ok(()),
        )
        .unwrap()
    }
    fn state(recording: &Recording) -> State {
        let mut app = App::new("test".into());
        app.topic_filter = "demo".into();
        let mut tools = Tools::default();
        tools.start = Some(0);
        tools.end = Some(2);
        tools.last_search = Some(("find 0".into(), Some("demo/key".into())));
        let mut state = State::snapshot(recording, 1, &app, &tools);
        state.selected_key = Some("demo/key".into());
        state
    }
    #[test]
    fn roundtrip_and_atomic_replacement_preserve_capture() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.ndjson");
        fixture(&source, false);
        let original = fs::read(&source).unwrap();
        let recording = selected(&source, "demo/**");
        let path = dir.path().join("state/checkpoint.json");
        let mut state = state(&recording);
        save(&path, &state).unwrap();
        assert_eq!(
            load(&path, &recording).unwrap(),
            (Some(state.clone()), false)
        );
        state.position = 2;
        save(&path, &state).unwrap();
        assert_eq!(load(&path, &recording).unwrap().0.unwrap().position, 2);
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn changed_excluded_payload_or_selection_invalidates_positions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.ndjson");
        fixture(&source, false);
        let recording = selected(&source, "demo/**");
        let saved = state(&recording);
        assert!(!saved.validate(&selected(&source, "**")).unwrap());
        // Same number of records, same file size, same selected bytes: an
        // excluded record changing must still invalidate the source identity.
        fixture(&source, true);
        assert!(!saved.validate(&selected(&source, "demo/**")).unwrap());
        fixture(&source, false);
        assert!(saved.validate(&selected(&source, "demo/**")).unwrap());
    }
    #[test]
    fn malformed_future_oversized_and_unsafe_state_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.ndjson");
        fixture(&source, false);
        let recording = selected(&source, "**");
        let path = dir.path().join("checkpoint.json");
        assert_eq!(load(&path, &recording).unwrap(), (None, false));
        fs::write(&path, "{broken").unwrap();
        assert!(load(&path, &recording).is_err());
        fs::write(&path, vec![b' '; MAX_STATE_BYTES + 1]).unwrap();
        assert!(load(&path, &recording).is_err());
        let mut saved = state(&recording);
        saved.version = 99;
        assert!(saved.validate(&recording).is_err());
        saved.version = 1;
        saved.position = 100;
        assert!(saved.validate(&recording).is_err());
        saved.position = 1;
        saved.search = Some(("export /tmp/unsafe".into(), None));
        assert!(saved.validate(&recording).is_err());
        saved.search = Some(("find 0".into(), None));
        assert!(saved.validate(&recording).is_err());
    }
    #[tokio::test]
    async fn final_checkpoint_follows_pending_write_and_disabled_mode_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.ndjson");
        fixture(&source, false);
        let recording = selected(&source, "**");
        let path = dir.path().join("checkpoint.json");
        let mut checkpoints = Checkpoints {
            path: Some(path.clone()),
            ..Checkpoints::default()
        };
        let mut state = state(&recording);
        checkpoints.checkpoint(state.clone(), false).await.unwrap();
        state.position = 2;
        checkpoints.checkpoint(state.clone(), true).await.unwrap();
        assert_eq!(load(&path, &recording).unwrap().0.unwrap().position, 2);
        let (mut disabled, restored, message) =
            Checkpoints::open(Path::new("does-not-exist"), &recording, false);
        assert!(restored.is_none() && message.is_none() && !disabled.enabled());
        disabled.checkpoint(state, true).await.unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
