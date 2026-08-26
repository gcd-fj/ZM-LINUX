mod diagnostics;
mod input;
mod navigator;
mod runtime;
mod ui_backend;

pub use runtime::{
    GAME_HEIGHT, GAME_WIDTH, GameFrameInput, GameRuntime, RUFFLE_REVISION, RuntimeEvent,
};
