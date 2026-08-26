use crate::runtime::{GAME_HEIGHT, GAME_WIDTH};
use egui::{Pos2, Rect};
use ruffle_core::{
    Player, PlayerEvent,
    events::{
        ImeEvent, KeyDescriptor, KeyLocation, LogicalKey, MouseButton, MouseWheelDelta, NamedKey,
        PhysicalKey,
    },
};

pub(crate) fn event_needs_game_focus(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(_)
    )
}

pub(crate) fn forward_event(
    player: &mut Player,
    event: &egui::Event,
    viewport: Rect,
    last: &mut Option<(f64, f64)>,
) {
    match event {
        egui::Event::PointerMoved(position) => {
            if let Some((x, y)) = game_position(*position, viewport) {
                *last = Some((x, y));
                player.set_mouse_in_stage(true);
                player.handle_event(PlayerEvent::MouseMove { x, y });
            } else if last.take().is_some() {
                player.set_mouse_in_stage(false);
                player.handle_event(PlayerEvent::MouseLeave);
            }
        }
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } => {
            if let Some((x, y)) = game_position(*pos, viewport) {
                let button = match button {
                    egui::PointerButton::Primary => MouseButton::Left,
                    egui::PointerButton::Secondary => MouseButton::Right,
                    egui::PointerButton::Middle => MouseButton::Middle,
                    _ => MouseButton::Unknown,
                };
                player.handle_event(if *pressed {
                    PlayerEvent::MouseDown {
                        x,
                        y,
                        button,
                        index: None,
                    }
                } else {
                    PlayerEvent::MouseUp { x, y, button }
                });
            }
        }
        egui::Event::PointerGone => {
            *last = None;
            player.set_mouse_in_stage(false);
            player.handle_event(PlayerEvent::MouseLeave);
        }
        egui::Event::MouseWheel { delta, .. } if last.is_some() => {
            player.handle_event(PlayerEvent::MouseWheel {
                delta: MouseWheelDelta::Pixels(f64::from(delta.y)),
            });
        }
        egui::Event::Key {
            key,
            physical_key,
            pressed,
            repeat,
            ..
        } => {
            if *repeat && !*pressed {
                return;
            }
            if let Some(key) = key_descriptor(*physical_key, *key) {
                player.handle_event(if *pressed {
                    PlayerEvent::KeyDown { key }
                } else {
                    PlayerEvent::KeyUp { key }
                });
            }
        }
        egui::Event::Text(text) | egui::Event::Paste(text) => {
            for codepoint in text.chars() {
                player.handle_event(PlayerEvent::TextInput { codepoint });
            }
        }
        egui::Event::Ime(event) => match event {
            egui::ImeEvent::Preedit(text) => {
                player.handle_event(PlayerEvent::Ime(ImeEvent::Preedit(text.clone(), None)));
            }
            egui::ImeEvent::Commit(text) => {
                player.handle_event(PlayerEvent::Ime(ImeEvent::Commit(text.clone())));
            }
            _ => {}
        },
        egui::Event::WindowFocused(focused) => {
            player.handle_event(if *focused {
                PlayerEvent::FocusGained
            } else {
                PlayerEvent::FocusLost
            });
        }
        _ => {}
    }
}

fn game_position(position: Pos2, viewport: Rect) -> Option<(f64, f64)> {
    if !viewport.contains(position) || viewport.width() <= 0.0 || viewport.height() <= 0.0 {
        return None;
    }
    Some((
        f64::from((position.x - viewport.left()) / viewport.width()) * f64::from(GAME_WIDTH),
        f64::from((position.y - viewport.top()) / viewport.height()) * f64::from(GAME_HEIGHT),
    ))
}

fn key_descriptor(physical: Option<egui::Key>, logical: egui::Key) -> Option<KeyDescriptor> {
    Some(KeyDescriptor {
        physical_key: map_physical_key(physical.unwrap_or(logical)),
        logical_key: map_logical_key(logical)?,
        key_location: KeyLocation::Standard,
    })
}

fn map_logical_key(key: egui::Key) -> Option<LogicalKey> {
    use egui::Key;
    let named = match key {
        Key::ArrowDown => NamedKey::ArrowDown,
        Key::ArrowLeft => NamedKey::ArrowLeft,
        Key::ArrowRight => NamedKey::ArrowRight,
        Key::ArrowUp => NamedKey::ArrowUp,
        Key::Escape => NamedKey::Escape,
        Key::Tab => NamedKey::Tab,
        Key::Backspace => NamedKey::Backspace,
        Key::Enter => NamedKey::Enter,
        Key::Space => return Some(LogicalKey::Character(' ')),
        Key::Insert => NamedKey::Insert,
        Key::Delete => NamedKey::Delete,
        Key::Home => NamedKey::Home,
        Key::End => NamedKey::End,
        Key::PageUp => NamedKey::PageUp,
        Key::PageDown => NamedKey::PageDown,
        Key::F1 => NamedKey::F1,
        Key::F2 => NamedKey::F2,
        Key::F3 => NamedKey::F3,
        Key::F4 => NamedKey::F4,
        Key::F5 => NamedKey::F5,
        Key::F6 => NamedKey::F6,
        Key::F7 => NamedKey::F7,
        Key::F8 => NamedKey::F8,
        Key::F9 => NamedKey::F9,
        Key::F10 => NamedKey::F10,
        Key::F11 => NamedKey::F11,
        Key::F12 => NamedKey::F12,
        value => return key_character(value).map(LogicalKey::Character),
    };
    Some(LogicalKey::Named(named))
}

fn map_physical_key(key: egui::Key) -> PhysicalKey {
    use egui::Key;
    match key {
        Key::ArrowDown => PhysicalKey::ArrowDown,
        Key::ArrowLeft => PhysicalKey::ArrowLeft,
        Key::ArrowRight => PhysicalKey::ArrowRight,
        Key::ArrowUp => PhysicalKey::ArrowUp,
        Key::Escape => PhysicalKey::Escape,
        Key::Tab => PhysicalKey::Tab,
        Key::Backspace => PhysicalKey::Backspace,
        Key::Enter => PhysicalKey::Enter,
        Key::Space => PhysicalKey::Space,
        Key::Insert => PhysicalKey::Insert,
        Key::Delete => PhysicalKey::Delete,
        Key::Home => PhysicalKey::Home,
        Key::End => PhysicalKey::End,
        Key::PageUp => PhysicalKey::PageUp,
        Key::PageDown => PhysicalKey::PageDown,
        Key::F1 => PhysicalKey::F1,
        Key::F2 => PhysicalKey::F2,
        Key::F3 => PhysicalKey::F3,
        Key::F4 => PhysicalKey::F4,
        Key::F5 => PhysicalKey::F5,
        Key::F6 => PhysicalKey::F6,
        Key::F7 => PhysicalKey::F7,
        Key::F8 => PhysicalKey::F8,
        Key::F9 => PhysicalKey::F9,
        Key::F10 => PhysicalKey::F10,
        Key::F11 => PhysicalKey::F11,
        Key::F12 => PhysicalKey::F12,
        value => match key_character(value).map(|value| value.to_ascii_uppercase()) {
            Some('A') => PhysicalKey::KeyA,
            Some('B') => PhysicalKey::KeyB,
            Some('C') => PhysicalKey::KeyC,
            Some('D') => PhysicalKey::KeyD,
            Some('E') => PhysicalKey::KeyE,
            Some('F') => PhysicalKey::KeyF,
            Some('G') => PhysicalKey::KeyG,
            Some('H') => PhysicalKey::KeyH,
            Some('I') => PhysicalKey::KeyI,
            Some('J') => PhysicalKey::KeyJ,
            Some('K') => PhysicalKey::KeyK,
            Some('L') => PhysicalKey::KeyL,
            Some('M') => PhysicalKey::KeyM,
            Some('N') => PhysicalKey::KeyN,
            Some('O') => PhysicalKey::KeyO,
            Some('P') => PhysicalKey::KeyP,
            Some('Q') => PhysicalKey::KeyQ,
            Some('R') => PhysicalKey::KeyR,
            Some('S') => PhysicalKey::KeyS,
            Some('T') => PhysicalKey::KeyT,
            Some('U') => PhysicalKey::KeyU,
            Some('V') => PhysicalKey::KeyV,
            Some('W') => PhysicalKey::KeyW,
            Some('X') => PhysicalKey::KeyX,
            Some('Y') => PhysicalKey::KeyY,
            Some('Z') => PhysicalKey::KeyZ,
            Some('0') => PhysicalKey::Digit0,
            Some('1') => PhysicalKey::Digit1,
            Some('2') => PhysicalKey::Digit2,
            Some('3') => PhysicalKey::Digit3,
            Some('4') => PhysicalKey::Digit4,
            Some('5') => PhysicalKey::Digit5,
            Some('6') => PhysicalKey::Digit6,
            Some('7') => PhysicalKey::Digit7,
            Some('8') => PhysicalKey::Digit8,
            Some('9') => PhysicalKey::Digit9,
            _ => PhysicalKey::Unknown,
        },
    }
}

fn key_character(key: egui::Key) -> Option<char> {
    use egui::Key;
    Some(match key {
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_viewport_coordinates() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(940.0, 590.0));
        assert_eq!(
            game_position(Pos2::new(100.0, 50.0), rect),
            Some((0.0, 0.0))
        );
        assert_eq!(
            game_position(Pos2::new(1040.0, 640.0), rect),
            Some((940.0, 590.0))
        );
        assert_eq!(game_position(Pos2::new(20.0, 20.0), rect), None);
    }

    #[test]
    fn keyboard_input_is_processed_without_waiting_for_a_frame() {
        let event = egui::Event::Text("测试".into());
        assert!(event_needs_game_focus(&event));
        assert!(key_descriptor(Some(egui::Key::A), egui::Key::A).is_some());
    }
}
