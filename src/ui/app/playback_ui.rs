use super::{ContinueRequest, NowPlaying};

/// UI-owned playback intent and the latest state reported by the supervisor.
/// The supervisor itself remains outside AppState; this substate only bridges
/// user actions and renderable playback information.
#[derive(Default)]
pub(crate) struct PlaybackUiState {
    pub play_requested: bool,
    pub continue_request: Option<ContinueRequest>,
    pub now_playing: Option<NowPlaying>,
}
