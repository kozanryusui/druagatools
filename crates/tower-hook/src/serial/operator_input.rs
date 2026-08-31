use druaga_tower_board::{OperatorAction, OperatorInputEvent, OperatorInputState};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_0, VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7, VK_8, VK_9,
    VK_BACK, VK_DOWN, VK_F1, VK_F2, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4,
    VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_SHIFT, VK_SPACE, VK_UP,
};

use crate::reader_protocol::ReaderSide;

const ACTION_KEYS: [(OperatorAction, VIRTUAL_KEY); 6] = [
    (OperatorAction::Test, VK_F2),
    (OperatorAction::Service, VK_F1),
    (OperatorAction::SelectUp, VK_UP),
    (OperatorAction::SelectDown, VK_DOWN),
    (OperatorAction::Enter, VK_SPACE),
    (OperatorAction::Coin, VK_BACK),
];

// A key-down edge for N requests `cardN.bin`. Shift selects the right reader;
// an unmodified key selects the left reader. Holding a key does not repeat the
// request. The main keyboard and numeric keypad use the same map.
const CARD_KEYS: [(u8, VIRTUAL_KEY, VIRTUAL_KEY); 10] = [
    (0, VK_0, VK_NUMPAD0),
    (1, VK_1, VK_NUMPAD1),
    (2, VK_2, VK_NUMPAD2),
    (3, VK_3, VK_NUMPAD3),
    (4, VK_4, VK_NUMPAD4),
    (5, VK_5, VK_NUMPAD5),
    (6, VK_6, VK_NUMPAD6),
    (7, VK_7, VK_NUMPAD7),
    (8, VK_8, VK_NUMPAD8),
    (9, VK_9, VK_NUMPAD9),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CardMountRequest {
    pub number: u8,
    pub side: ReaderSide,
}

pub struct KeyboardInputCapture {
    previous: [OperatorInputState; ACTION_KEYS.len()],
    previous_cards: [bool; CARD_KEYS.len()],
    test_latched: OperatorInputState,
}

impl KeyboardInputCapture {
    pub const fn new() -> Self {
        Self {
            previous: [OperatorInputState::Released; ACTION_KEYS.len()],
            previous_cards: [false; CARD_KEYS.len()],
            test_latched: OperatorInputState::Released,
        }
    }

    pub fn reset(&mut self) {
        self.previous = [OperatorInputState::Released; ACTION_KEYS.len()];
        self.previous_cards = [false; CARD_KEYS.len()];
        self.test_latched = OperatorInputState::Released;
    }

    pub fn poll(&mut self) -> [Option<OperatorInputEvent>; ACTION_KEYS.len()] {
        let mut events = [None; ACTION_KEYS.len()];
        for (index, (action, key)) in ACTION_KEYS.into_iter().enumerate() {
            let current = key_state(key);
            if action == OperatorAction::Test {
                if current == OperatorInputState::Pressed
                    && self.previous[index] == OperatorInputState::Released
                {
                    self.test_latched = match self.test_latched {
                        OperatorInputState::Released => OperatorInputState::Pressed,
                        OperatorInputState::Pressed => OperatorInputState::Released,
                    };
                    events[index] = Some(OperatorInputEvent::new(action, self.test_latched));
                }
                self.previous[index] = current;
                continue;
            }
            if current != self.previous[index] {
                events[index] = Some(OperatorInputEvent::new(action, current));
                self.previous[index] = current;
            }
        }
        events
    }

    pub fn poll_card_mount(&mut self) -> Option<CardMountRequest> {
        let mut selected = None;
        let side = if key_state(VK_SHIFT) == OperatorInputState::Pressed {
            ReaderSide::Right
        } else {
            ReaderSide::Left
        };
        for (index, (number, main_key, keypad_key)) in CARD_KEYS.into_iter().enumerate() {
            let pressed = key_state(main_key) == OperatorInputState::Pressed
                || key_state(keypad_key) == OperatorInputState::Pressed;
            if pressed && !self.previous_cards[index] && selected.is_none() {
                selected = Some(CardMountRequest { number, side });
            }
            self.previous_cards[index] = pressed;
        }
        selected
    }
}

fn key_state(key: VIRTUAL_KEY) -> OperatorInputState {
    // SAFETY: GetAsyncKeyState accepts every virtual-key value. These constants are valid keys.
    let state = unsafe { GetAsyncKeyState(i32::from(key)) };
    if (state as u16 & 0x8000) != 0 {
        OperatorInputState::Pressed
    } else {
        OperatorInputState::Released
    }
}
