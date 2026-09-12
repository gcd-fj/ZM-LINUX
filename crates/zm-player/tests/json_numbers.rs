use ruffle_core::{LoadBehavior, PlayerBuilder, backend::log::LogBackend, tag_utils::SwfMovie};
use std::sync::{Arc, Mutex};
use zm_player::game_load_behavior;

struct TestLog(Arc<Mutex<Vec<String>>>);
impl LogBackend for TestLog {
    fn avm_warning(&self, message: &str) {
        panic!("AVM warning: {message}");
    }
    fn avm_trace(&self, message: &str) {
        self.0.lock().unwrap().push(message.to_owned());
    }
}

#[test]
fn avm2_json_preserves_large_integers_and_server_dates() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .try_init();
    run_movie(
        include_bytes!("fixtures/JsonNumbers.swf"),
        LoadBehavior::Blocking,
    );
}

#[test]
fn timeline_rewind_preserves_script_overlay_order() {
    run_movie(
        include_bytes!("fixtures/TimelineOverlay.swf"),
        LoadBehavior::Blocking,
    );
}

fn run_movie(bytes: &[u8], load_behavior: LoadBehavior) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let movie = SwfMovie::from_data(bytes, "file:///JsonNumbers.swf".into(), None, None).unwrap();
    let player = PlayerBuilder::new()
        .with_movie(movie)
        .with_log(TestLog(log.clone()))
        .with_autoplay(true)
        .with_load_behavior(load_behavior)
        .build();
    for _ in 0..3 {
        player.lock().unwrap().run_frame();
    }
    assert_eq!(*log.lock().unwrap(), ["JSON_NUMBERS_OK"]);
}

#[test]
fn timeline_label_pages_stay_above_background() {
    run_movie(
        include_bytes!("fixtures/TimelineLabels.swf"),
        LoadBehavior::Blocking,
    );
}

#[test]
fn host_streaming_preserves_large_integers_and_server_dates() {
    run_movie(
        include_bytes!("fixtures/JsonNumbers.swf"),
        game_load_behavior(),
    );
}

#[test]
fn host_streaming_preserves_script_overlay_order() {
    run_movie(
        include_bytes!("fixtures/TimelineOverlay.swf"),
        game_load_behavior(),
    );
}

#[test]
fn host_streaming_keeps_label_pages_above_background() {
    run_movie(
        include_bytes!("fixtures/TimelineLabels.swf"),
        game_load_behavior(),
    );
}
