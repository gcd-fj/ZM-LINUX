use ruffle_core::{
    LoadBehavior, Player, PlayerBuilder,
    backend::navigator::{NullExecutor, NullNavigatorBackend},
    limits::ExecutionLimit,
    tag_utils::SwfMovie,
};
use std::sync::{Arc, Mutex};
use zm_player::game_load_behavior;

/// An ImportAssets tag stops preloading until its asynchronous dependency finishes.
/// Holding the executor provides a deterministic loading boundary without sleeps,
/// network access, large fixtures, or relying on a machine's decoding speed.
struct PendingImport {
    player: Arc<Mutex<Player>>,
    executor: NullExecutor,
    _directory: tempfile::TempDir,
}

impl PendingImport {
    fn new(load_behavior: LoadBehavior) -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("dependency.swf"), movie(1, false)).unwrap();
        let executor = NullExecutor::new();
        let navigator = NullNavigatorBackend::with_base_path(directory.path(), &executor).unwrap();
        let root_url = url::Url::from_file_path(directory.path().join("root.swf")).unwrap();
        let movie = SwfMovie::from_data(&movie(3, true), root_url.into(), None, None).unwrap();
        let player = PlayerBuilder::new()
            .with_movie(movie)
            .with_navigator(navigator)
            .with_autoplay(true)
            .with_load_behavior(load_behavior)
            .build();

        {
            let mut player = player.lock().unwrap();
            assert!(!player.preload(&mut ExecutionLimit::none()));
            assert_eq!(frames_loaded(&mut player), 2);
            assert_eq!(player.current_frame(), Some(0));
            player.render();
            assert!(!player.needs_render());
        }

        Self {
            player,
            executor,
            _directory: directory,
        }
    }

    fn complete_import(&mut self) {
        // Do not hold the player lock while executing a loader that also locks it.
        self.executor.run();
        let mut player = self.player.lock().unwrap();
        assert!(player.preload(&mut ExecutionLimit::none()));
        assert_eq!(frames_loaded(&mut player), 3);
    }
}

#[test]
fn host_streaming_advances_and_renders_while_a_dependency_is_pending() {
    let mut pending = PendingImport::new(game_load_behavior());

    {
        let mut player = pending.player.lock().unwrap();
        for expected_frame in [1, 2, 2] {
            player.run_frame();
            assert_eq!(player.current_frame(), Some(expected_frame));
            assert!(player.needs_render());
            assert!(!player.preload(&mut ExecutionLimit::none()));
            assert_eq!(frames_loaded(&mut player), 2);
            player.render();
            assert!(!player.needs_render());
        }
    }

    pending.complete_import();
    let mut player = pending.player.lock().unwrap();
    player.run_frame();
    assert_eq!(player.current_frame(), Some(3));
    assert!(player.needs_render());
}

#[test]
fn delayed_loading_freezes_until_the_same_dependency_completes() {
    let mut pending = PendingImport::new(LoadBehavior::Delayed);

    {
        let mut player = pending.player.lock().unwrap();
        for _ in 0..3 {
            player.run_frame();
            assert_eq!(player.current_frame(), Some(0));
            assert!(!player.needs_render());
            assert!(!player.preload(&mut ExecutionLimit::none()));
            assert_eq!(frames_loaded(&mut player), 2);
        }
    }

    pending.complete_import();
    let mut player = pending.player.lock().unwrap();
    for expected_frame in 1..=3 {
        player.run_frame();
        assert_eq!(player.current_frame(), Some(expected_frame));
        assert!(player.needs_render());
        player.render();
    }
}

fn frames_loaded(player: &mut Player) -> i32 {
    player.mutate_with_update_context(|context| {
        context
            .stage
            .root_clip()
            .unwrap()
            .as_movie_clip()
            .unwrap()
            .frames_loaded()
    })
}

fn movie(frame_count: u16, has_import: bool) -> Vec<u8> {
    // Uncompressed SWF 7: zero-sized RECT, 24 fps, followed by the frame count.
    let mut body = vec![0x08, 0x00, 0x00, 0x18];
    body.extend_from_slice(&frame_count.to_le_bytes());
    for frame in 0..frame_count {
        if has_import && frame == 2 {
            // ImportAssets with a local URL and no exported symbols is enough to
            // suspend preloading between the second and third ShowFrame tags.
            let mut import = b"dependency.swf\0".to_vec();
            import.extend_from_slice(&0_u16.to_le_bytes());
            tag(&mut body, 57, &import);
        }
        tag(&mut body, 1, &[]); // ShowFrame
    }
    tag(&mut body, 0, &[]); // End

    let mut bytes = b"FWS\x07".to_vec();
    bytes.extend_from_slice(&u32::try_from(body.len() + 8).unwrap().to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

fn tag(bytes: &mut Vec<u8>, code: u16, body: &[u8]) {
    assert!(body.len() < 63);
    let header = (code << 6) | u16::try_from(body.len()).unwrap();
    bytes.extend_from_slice(&header.to_le_bytes());
    bytes.extend_from_slice(body);
}
