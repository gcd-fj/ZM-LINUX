mod diagnostics;
mod input;
mod navigator;
mod runtime;
mod task_diagnostics;
mod ui_backend;

pub use diagnostics::ResourceLoadingProgress;
pub use runtime::{
    GAME_HEIGHT, GAME_WIDTH, GameFrameInput, GameRuntime, RUFFLE_REVISION, RuntimeEvent,
    RuntimeMessage, game_load_behavior,
};
