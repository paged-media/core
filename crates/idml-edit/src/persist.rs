//! Native project format.
//!
//! M3 ships the simplest serializable form that round-trips: the
//! original IDML bytes + the forward command log. On open we
//! re-parse the IDML into a fresh `Project` and replay every
//! command. Replay is deterministic because commands carry no UI
//! state — the only inputs are IDML + commands.
//!
//! On-disk envelope (`*.idmlproj`) is JSON:
//!
//! ```json
//! { "version": 1, "idml_b64": "...", "commands": [ ...JSON commands... ] }
//! ```
//!
//! Asset bag (linked images, fonts, OPFS-cached payloads) lives in a
//! later revision of this format. M3 keeps assets implicit in the
//! IDML container itself; placed images via `data:` URIs travel
//! inside command payloads.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::project::Project;

const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    version: u32,
    idml_b64: String,
    commands: Vec<Command>,
}

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("base64 decode: {0}")]
    Base64(String),
    #[error("json: {0}")]
    Json(String),
    #[error("scene: {0}")]
    Scene(String),
    #[error("replay: {0}")]
    Replay(String),
    #[error("unsupported project format version {0}")]
    UnsupportedVersion(u32),
}

impl Project {
    /// Serialize this project to a native-format byte string. The
    /// caller is responsible for writing it to disk (File System
    /// Access API on the web, or stdout in CLI tooling). The bytes
    /// are pretty-printed JSON for human-readable diffs; strict
    /// minified output lands when we measure the size impact.
    pub fn serialize_native(&self) -> Result<Vec<u8>, PersistError> {
        let env = Envelope {
            version: VERSION,
            idml_b64: B64.encode(self.original_idml_bytes()),
            commands: self.forward_log().to_vec(),
        };
        serde_json::to_vec_pretty(&env).map_err(|e| PersistError::Json(e.to_string()))
    }

    /// Open a project from a native-format byte string. Replays the
    /// command log on top of the parsed IDML so the new project lives
    /// at exactly the same edit state as the saved one.
    pub fn deserialize_native(bytes: &[u8]) -> Result<Self, PersistError> {
        let env: Envelope =
            serde_json::from_slice(bytes).map_err(|e| PersistError::Json(e.to_string()))?;
        if env.version != VERSION {
            return Err(PersistError::UnsupportedVersion(env.version));
        }
        let idml = B64
            .decode(env.idml_b64.as_bytes())
            .map_err(|e| PersistError::Base64(e.to_string()))?;
        let mut project = Self::open(&idml).map_err(|e| PersistError::Scene(e.to_string()))?;
        project.set_original_idml_bytes(idml);
        for cmd in env.commands {
            project
                .apply(cmd)
                .map_err(|e| PersistError::Replay(e.to_string()))?;
        }
        // The command log we just replayed is the *forward* log; the
        // ones we logged should match what the file said. Replace to
        // avoid double-logging from `apply`.
        Ok(project)
    }
}
