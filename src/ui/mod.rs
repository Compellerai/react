//! UI components using egui

mod controls;
mod floating_logos;
mod virtual_stage;

pub use controls::{ControlPanel, OutputState, NdiResolution, NdiTallyInfo, AuthAction, YouTubeAction, YouTubeState, YouTubeQuality, YouTubePrivacy, YouTubeStreamConfig, TwitchAction, TwitchStreamState, TwitchResolution, KickAction, DualStreamAction, DualStreamState, StreamTarget, TwitchServerInfo};
pub use floating_logos::FloatingLogos;
pub use virtual_stage::VirtualStageView;
