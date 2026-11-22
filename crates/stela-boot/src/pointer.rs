//! The firmware's pointer, when this PC has one: relative movement drawn as
//! a small arrow on the canvas, and a click on the highlighted row.
//!
//! UEFI's Simple Pointer reports a USB mouse (many laptops' touchpads do
//! not appear here until an OS loads its own driver), so a missing pointer
//! leaves the keyboard page unchanged.
use crate::ui::{self, Canvas};
use alloc::vec::Vec;
use uefi::{
    boot::{self, ScopedProtocol},
    proto::console::pointer::Pointer,
};

/// Where the arrow is, in canvas units.
#[derive(Default)]
pub(crate) struct Cursor {
    pub(crate) x: i32,
    pub(crate) y: i32,
    /// Set when a pointer was found, so the page draws the arrow.
    pub(crate) shown: bool,
    /// A click seen since the page last looked.
    pub(crate) clicked: bool,
    devices: Vec<ScopedProtocol<Pointer>>,
}

impl Cursor {
    /// Look for the firmware's pointers. Called before boot services end.
    pub(crate) fn open() -> Self {
        let mut devices = Vec::new();
        if let Ok(handles) = boot::find_handles::<Pointer>() {
            for handle in handles {
                if let Ok(pointer) = boot::open_protocol_exclusive::<Pointer>(handle) {
                    devices.push(pointer);
                }
            }
        }
        Cursor {
            shown: !devices.is_empty(),
            devices,
            ..Cursor::default()
        }
    }

    /// Take the movement and button state the firmware has queued. A button
    /// held from the last call is not a new click.
    pub(crate) fn poll(&mut self) {
        let mut left = false;
        for pointer in &mut self.devices {
            let Ok(Some(state)) = pointer.read_state() else {
                continue;
            };
            self.x += state.relative_movement_x;
            self.y += state.relative_movement_y;
            left |= bool::from(state.left_button);
        }
        if left && !self.clicked {
            self.clicked = true;
        }
    }
}

/// The arrow: a few pixels of orange over whatever the page drew.
pub(crate) fn draw(canvas: &mut Canvas, cursor: &Cursor) {
    if !cursor.shown {
        return;
    }
    let (x, y) = (cursor.x.max(0) as usize, cursor.y.max(0) as usize);
    let (width, height) = canvas.size();
    let x = x.min(width.saturating_sub(10));
    let y = y.min(height.saturating_sub(14));
    for row in 0..12 {
        let span = match row {
            0..=2 => 2,
            3..=5 => 4,
            6..=8 => 7,
            _ => 10,
        };
        canvas.fill(x, y + row, span, 1, ui::ORANGE);
    }
    canvas.fill(x, y, 1, 12, ui::TEXT);
}

/// Whether a point in canvas units falls inside a row's rectangle.
pub(crate) fn hits(cursor: &Cursor, x: usize, y: usize, width: usize, height: usize) -> bool {
    cursor.shown
        && cursor.x >= 0
        && cursor.y >= 0
        && (cursor.x as usize) >= x
        && (cursor.x as usize) < x.saturating_add(width)
        && (cursor.y as usize) >= y
        && (cursor.y as usize) < y.saturating_add(height)
}
