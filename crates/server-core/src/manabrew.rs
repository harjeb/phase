//! Opt-in ManaBrew presentation for authoritative, token-authenticated sessions.
//!
//! Create/join/reconnect and socket-generation checks remain the native transport's
//! responsibility. Call these methods while holding its existing session lock;
//! never accept a viewer or action table from a client. No single-user host or AI
//! seat is introduced by this adapter.

use engine::types::PlayerId;
pub use manabrew_compat::ClientToServerMessage;
use manabrew_compat::{
    build_prompt, build_state_update, prepare_snapshot_with_prompt_id, translate_client_message,
    AdapterError, AgentPrompt, CardTextLookup, PreparedManabrewSnapshot, StateUpdate,
};
use serde::{Deserialize, Serialize};

use crate::session::{GameSession, RevisionedActionResult, SessionActionError};

#[derive(Debug)]
pub enum ManabrewError {
    InvalidPlayerToken,
    GameNotStarted,
    PromptIdExhausted,
    InvalidPayload(String),
    Adapter(AdapterError),
    Session(SessionActionError),
}

impl ManabrewError {
    pub fn into_session_error(self) -> SessionActionError {
        match self {
            Self::Session(error) => error,
            error => SessionActionError::RequestRejected(format!("ManaBrew: {error:?}")),
        }
    }
}

impl From<AdapterError> for ManabrewError {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

/// The outer envelope is Phase-owned; `update` and `prompt` use the adapter's
/// extension-aware ManaBrew DTOs. A waiting viewer receives no prompt, not the
/// deciding seat's private prompt. Unsupported conversions fail closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManabrewSnapshot {
    pub game_code: String,
    pub state_revision: u64,
    pub your_player: PlayerId,
    pub update: StateUpdate,
    pub prompt: Option<AgentPrompt>,
}

/// Additional compatibility-envelope bound, within the WebSocket frame limit.
pub fn guard_message(message: &ClientToServerMessage) -> Result<(), String> {
    let bytes = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    if bytes.len() > 65_536 {
        return Err("ManaBrew message exceeds 65536 bytes".into());
    }
    Ok(())
}

impl GameSession {
    fn prepare_manabrew(&self, token: &str) -> Result<PreparedManabrewSnapshot, ManabrewError> {
        let viewer = self
            .player_for_token(token)
            .filter(|player| !self.ai_seats.contains(player))
            .ok_or(ManabrewError::InvalidPlayerToken)?;
        if !self.game_started {
            return Err(ManabrewError::GameNotStarted);
        }
        // Seat-bound, revision-bound, nonzero IDs. Checked arithmetic must never
        // wrap an old capability into validity. The revision is persisted by the
        // native session and also advances on takeback, not just forward play.
        let prompt_id = self
            .state_revision
            .checked_mul(4)
            .and_then(|value| value.checked_add(u64::from(viewer.0) + 1))
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(ManabrewError::PromptIdExhausted)?;
        Ok(prepare_snapshot_with_prompt_id(
            &self.state,
            viewer,
            &self.game_code,
            prompt_id,
        )?)
    }

    /// Same authenticated snapshot builder after start, transitions and native
    /// reconnect acceptance. Re-reading a revision never rotates its prompt ID.
    pub fn manabrew_snapshot(
        &self,
        token: &str,
        cards: &impl CardTextLookup,
    ) -> Result<ManabrewSnapshot, ManabrewError> {
        let prepared = self.prepare_manabrew(token)?;
        let update = build_state_update(&prepared, cards)?;
        let prompt = match build_prompt(&prepared, cards) {
            Ok(prompt) => Some(prompt),
            Err(AdapterError::NoAuthorizedPrompt { .. }) => None,
            Err(error) => return Err(error.into()),
        };
        Ok(ManabrewSnapshot {
            game_code: self.game_code.clone(),
            state_revision: self.state_revision,
            your_player: prepared.viewer,
            update,
            prompt,
        })
    }

    /// Translate against current authoritative state. The transport must retain
    /// the session lock until its ordinary action handler has applied the result.
    pub fn translate_manabrew_message(
        &self,
        token: &str,
        message: ClientToServerMessage,
    ) -> Result<engine::types::actions::GameAction, ManabrewError> {
        guard_message(&message).map_err(ManabrewError::InvalidPayload)?;
        let prepared = self.prepare_manabrew(token)?;
        let action = translate_client_message(message, &prepared.prompt_context(), &self.state)?;
        crate::game_action_payload_guard::guard_game_action_payload(&action)
            .map_err(ManabrewError::InvalidPayload)?;
        Ok(action)
    }

    /// Translate under the same lock as application, then use native session
    /// validation. Returns the standard revisioned transition for the existing
    /// persistence/broadcast pipeline; callers must not increment it again.
    pub fn handle_manabrew_message(
        &mut self,
        token: &str,
        message: ClientToServerMessage,
    ) -> Result<RevisionedActionResult, ManabrewError> {
        let action = self.translate_manabrew_message(token, message)?;
        let result = self
            .handle_action_with_card_db_outcome(token, action, None)
            .map_err(ManabrewError::Session)?;
        Ok((self.advance_state_revision(), result))
    }
}
