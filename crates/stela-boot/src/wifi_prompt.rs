//! The network chooser a `--wifi-prompt` line asks for.
//!
//! `stela.efi` joins a network before the engine streams the install image,
//! so the choice is made here, in the firmware's own time: a page in Stela's
//! look lists the networks this USB remembers, and a name and passphrase
//! typed on it join the list. The chosen passphrase goes to the key file the
//! engine reads (`--wifi-key`), and the chosen name is written into this
//! boot's options (`--wifi SSID`), so a laptop carried between networks
//! needs no other computer to change it: the next boot highlights the last
//! one and Enter joins it.
//!
//! The list is plaintext, beside the passphrase file Stela already keeps in
//! plaintext on the ESP ([`LIST`]): anyone holding the USB can read both.

use crate::{
    choice::{self, Press, Wait},
    console::{font, Framebuffer},
    output,
    ui::{self, Canvas},
};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use stela_boot_core::{options::System, options::WIFI};
use uefi::{
    CString16,
    fs::{FileSystem, PathBuf},
};

/// The networks this USB remembers, one `SSID<TAB>passphrase` per line, the
/// last one joined first.
const LIST: &str = "\\EFI\\stela\\wifi.list";
/// The key file `stela setup` writes the passphrase to.
const DEFAULT_KEY: &str = stela_boot_core::options::DEFAULT_WIFI_KEY;
/// How many networks the list keeps; older ones fall off.
const KEPT: usize = 8;
/// A network name is 1 to 32 bytes.
const MAX_SSID: usize = 32;
/// A passphrase is 8 to 63 printable ASCII characters, or 64 hex digits.
const MAX_PASSPHRASE: usize = 64;

/// Join the network chosen at start, when the options ask for it
/// (`--wifi-prompt`). `options` gains the `--wifi` word the engine reads,
/// and the passphrase is written where `--wifi-key` names it.
pub(crate) fn ask(options: &mut Vec<u8>, volume: &mut FileSystem) {
    let Ok(text) = core::str::from_utf8(options) else {
        return;
    };
    let Ok(system) = System::parse(text) else {
        return;
    };
    if !system.wifi_prompt {
        return;
    }
    let Some(chosen) = choose(volume) else {
        output::arguments(format_args!(
            "WIFI skipped at start: no network is joined, so a network install needs a cable\n"
        ));
        return;
    };
    let path = system.wifi_key.as_deref().unwrap_or(DEFAULT_KEY);
    let written = CString16::try_from(path)
        .map(PathBuf::from)
        .ok()
        .is_some_and(|path| volume.write(&path, chosen.passphrase.as_bytes()).is_ok());
    if !written {
        output::arguments(format_args!(
            "WIFI refused: the key file {path} cannot be written, so {} is not joined\n",
            chosen.ssid
        ));
        return;
    }
    remember(volume, &chosen);
    output::arguments(format_args!("WIFI {} chosen at start\n", chosen.ssid));
    let mut line = String::from(text);
    line.push(' ');
    line.push_str(&format!("{WIFI} {}", escaped(&chosen.ssid)));
    *options = line.into_bytes();
}

/// A network the list holds, with the passphrase it was joined with.
#[derive(Clone)]
struct Saved {
    ssid: String,
    passphrase: String,
}

/// What the picker wants next.
enum Step {
    Stay,
    Leave,
    Chosen(Saved),
}

/// The page the picker shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    /// The saved networks, a new one, and leaving without one.
    List,
    /// A network name being typed.
    Ssid,
    /// Its passphrase being typed.
    Passphrase,
}

struct Picker {
    saved: Vec<Saved>,
    cursor: usize,
    page: Page,
    /// The name typed on the first page, kept while the passphrase is typed.
    name: String,
    typed: String,
    notice: Option<&'static str>,
}

/// The chosen network, or `None` when the person left without one or this
/// PC cannot show a page.
fn choose(volume: &mut FileSystem) -> Option<Saved> {
    let Some((screen, font)) = Framebuffer::from_firmware().zip(font::console()) else {
        output::arguments(format_args!(
            "WIFI refused: no framebuffer to draw the choice on\n"
        ));
        return None;
    };
    let mut canvas = Canvas::new(screen, font);
    let mut picker = Picker {
        saved: load(volume),
        cursor: 0,
        page: Page::List,
        name: String::new(),
        typed: String::new(),
        notice: None,
    };
    output::arguments(format_args!(
        "WIFI prompt: {} saved network(s)\n",
        picker.saved.len()
    ));
    loop {
        picker.draw(&mut canvas);
        canvas.show();
        let press = match choice::key(None) {
            Wait::Key(press) => press,
            Wait::NoInput => return None,
            Wait::Nothing | Wait::Timeout => continue,
        };
        picker.notice = None;
        match picker.press(press) {
            Step::Stay => {}
            Step::Leave => return None,
            Step::Chosen(chosen) => return Some(chosen),
        }
    }
}

impl Picker {
    /// The rows of the list page: the saved networks, a new one, leaving.
    fn rows(&self) -> usize {
        self.saved.len() + 2
    }

    fn new_row(&self) -> usize {
        self.saved.len()
    }

    fn leave_row(&self) -> usize {
        self.saved.len() + 1
    }

    fn press(&mut self, press: Press) -> Step {
        match self.page {
            Page::List => self.press_list(press),
            Page::Ssid => self.press_ssid(press),
            Page::Passphrase => self.press_passphrase(press),
        }
    }

    fn press_list(&mut self, press: Press) -> Step {
        let rows = self.rows();
        match press {
            Press::Escape => return Step::Leave,
            Press::Up => self.cursor = (self.cursor + rows - 1) % rows,
            Press::Down => self.cursor = (self.cursor + 1) % rows,
            Press::Home => self.cursor = 0,
            Press::End => self.cursor = rows - 1,
            Press::Enter if self.cursor < self.saved.len() => {
                return Step::Chosen(self.saved[self.cursor].clone());
            }
            Press::Enter if self.cursor == self.leave_row() => return Step::Leave,
            Press::Enter => {
                self.page = Page::Ssid;
                self.typed = String::new();
            }
            _ => {}
        }
        Step::Stay
    }

    fn press_ssid(&mut self, press: Press) -> Step {
        match press {
            Press::Escape => self.page = Page::List,
            Press::Enter if self.typed.is_empty() => {
                self.notice = Some("Type the network's name first");
            }
            Press::Enter => {
                self.name = self.typed.clone();
                self.page = Page::Passphrase;
                self.typed = String::new();
            }
            Press::Backspace => {
                self.typed.pop();
            }
            Press::Character(character) if self.typed.len() < MAX_SSID && typable(character) => {
                self.typed.push(character);
            }
            _ => {}
        }
        Step::Stay
    }

    fn press_passphrase(&mut self, press: Press) -> Step {
        match press {
            Press::Escape => {
                self.page = Page::Ssid;
                self.typed = self.name.clone();
            }
            Press::Enter => match usable_passphrase(&self.typed) {
                true => {
                    return Step::Chosen(Saved {
                        ssid: self.name.clone(),
                        passphrase: self.typed.clone(),
                    });
                }
                false => {
                    self.notice =
                        Some("A passphrase is 8 to 63 printable characters, or 64 hex digits");
                }
            },
            Press::Backspace => {
                self.typed.pop();
            }
            Press::Character(character)
                if self.typed.len() < MAX_PASSPHRASE && typable(character) =>
            {
                self.typed.push(character);
            }
            _ => {}
        }
        Step::Stay
    }

    fn draw(&self, canvas: &mut Canvas) {
        let (width, height) = canvas.size();
        canvas.fill(0, 0, width, height, ui::BACKGROUND);
        let top = canvas.header() + 16;
        let column = width.saturating_sub(32).min(720);
        let left = (width - column) / 2;
        match self.page {
            Page::List => self.draw_list(canvas, left, top, column, height),
            Page::Ssid => self.draw_typed(canvas, left, top, column, "Network name", false),
            Page::Passphrase => self.draw_typed(canvas, left, top, column, "Passphrase", true),
        }
        if let Some(notice) = self.notice {
            canvas.text_centred(width / 2, height - 76, notice, 1, ui::ORANGE);
        }
        let keys: &[(&str, &str)] = match self.page {
            Page::List => &[("Enter", "Join"), ("Esc", "Skip")],
            Page::Ssid => &[("Enter", "Next"), ("Esc", "Back")],
            Page::Passphrase => &[("Enter", "Join"), ("Esc", "Back")],
        };
        footer(canvas, width, height, keys);
    }

    fn draw_list(
        &self,
        canvas: &mut Canvas,
        left: usize,
        top: usize,
        column: usize,
        height: usize,
    ) {
        canvas.text(left, top, "Choose a Wi-Fi network", 1, ui::TEXT);
        canvas.text_right(
            left + column,
            top,
            "Stela remembers these on the USB",
            1,
            ui::DIM,
        );
        const ROW: usize = 52;
        const GAP: usize = 8;
        let first_row = top + 36;
        let fits = (height.saturating_sub(first_row + 96) / (ROW + GAP)).max(1);
        let first = stela_boot_core::screen::window(self.cursor, self.rows(), fits);
        for (offset, row) in (first..self.rows()).take(fits).enumerate() {
            let y = first_row + offset * (ROW + GAP);
            let chosen = row == self.cursor;
            match chosen {
                true => {
                    canvas.panel(left, y, column, ROW, 8, ui::RAISED);
                    canvas.panel(left + 6, y + 9, 3, ROW.saturating_sub(18), 1, ui::ORANGE);
                }
                false => canvas.panel(left, y, column, ROW, 8, ui::CARD),
            }
            let (name, detail) = match row {
                row if row < self.saved.len() => (
                    self.saved[row].ssid.as_str(),
                    if row == 0 { "last joined" } else { "" },
                ),
                row if row == self.new_row() => ("New network…", "type a name and passphrase"),
                _ => ("Skip", "use a cable instead"),
            };
            let ink = match chosen {
                true => ui::TEXT,
                false => ui::DIM,
            };
            canvas.text(left + 24, y + 10, name, 1, ink);
            if !detail.is_empty() {
                canvas.text(left + 24, y + 30, detail, 1, ui::DIM);
            }
        }
    }

    fn draw_typed(
        &self,
        canvas: &mut Canvas,
        left: usize,
        top: usize,
        column: usize,
        title: &str,
        secret: bool,
    ) {
        canvas.text(left, top, title, 1, ui::TEXT);
        canvas.text_right(left + column, top, &self.name, 1, ui::DIM);
        let field = top + 40;
        canvas.panel(left, field, column, 52, 8, ui::RAISED);
        let mut shown = String::new();
        match secret {
            true => {
                for _ in 0..self.typed.len() {
                    shown.push('*');
                }
            }
            false => shown.push_str(&self.typed),
        }
        canvas.text(left + 24, field + 32, &shown, 1, ui::TEXT);
        if secret {
            canvas.text(
                left,
                field + 76,
                "8 to 63 printable characters, or 64 hex digits",
                1,
                ui::DIM,
            );
            canvas.text(
                left,
                field + 96,
                "Kept in plaintext on this USB, as `stela create --wifi` keeps it",
                1,
                ui::DIM,
            );
        }
    }
}

/// The keys a page offers, centred at its foot.
fn footer(canvas: &mut Canvas, width: usize, height: usize, keys: &[(&str, &str)]) {
    let gap = 24;
    let total: usize = keys
        .iter()
        .map(|(key, label)| canvas.text_width(key, 1) + 8 + canvas.text_width(label, 1))
        .sum::<usize>()
        + gap * keys.len().saturating_sub(1);
    let mut x = width.saturating_sub(total) / 2;
    for (key, label) in keys {
        canvas.text(x, height - 40, key, 1, ui::ORANGE);
        x += canvas.text_width(key, 1) + 8;
        canvas.text(x, height - 40, label, 1, ui::DIM);
        x += canvas.text_width(label, 1) + gap;
    }
}

/// The networks the ESP lists, newest first.
fn load(volume: &mut FileSystem) -> Vec<Saved> {
    let Ok(path) = CString16::try_from(LIST).map(PathBuf::from) else {
        return Vec::new();
    };
    let Ok(bytes) = volume.read(&path) else {
        return Vec::new();
    };
    let Ok(text) = core::str::from_utf8(&bytes) else {
        output::arguments(format_args!("WIFI list is not UTF-8: it is ignored\n"));
        return Vec::new();
    };
    let mut saved: Vec<Saved> = Vec::new();
    for line in text.lines() {
        let Some((ssid, passphrase)) = line.split_once('\t') else {
            continue;
        };
        if ssid.is_empty() || ssid.len() > MAX_SSID || !usable_passphrase(passphrase) {
            continue;
        }
        if saved.iter().any(|other| other.ssid == ssid) {
            continue;
        }
        saved.push(Saved {
            ssid: ssid.to_string(),
            passphrase: passphrase.to_string(),
        });
        if saved.len() == KEPT {
            break;
        }
    }
    saved
}

/// Put `chosen` first in the list the ESP holds, dropping any copy of it.
fn remember(volume: &mut FileSystem, chosen: &Saved) {
    let mut saved = load(volume);
    saved.retain(|other| other.ssid != chosen.ssid);
    saved.insert(0, chosen.clone());
    saved.truncate(KEPT);
    let mut text = String::new();
    for network in &saved {
        text.push_str(&format!("{}\t{}\n", network.ssid, network.passphrase));
    }
    let Some(path) = CString16::try_from(LIST).map(PathBuf::from).ok() else {
        return;
    };
    if volume.write(&path, text.as_bytes()).is_err() {
        output::arguments(format_args!(
            "WIFI list not updated: the USB could not be written\n"
        ));
    }
}

/// A character a name or passphrase may hold: printable ASCII, so the line
/// the ESP holds stays one line.
fn typable(character: char) -> bool {
    character.is_ascii() && !character.is_ascii_control()
}

/// A passphrase Stela joins with: 8 to 63 printable ASCII characters, or a
/// 64-digit hexadecimal PSK (`setup::wifi::check_passphrase`).
fn usable_passphrase(passphrase: &str) -> bool {
    let printable = passphrase
        .bytes()
        .all(|byte| (0x20..0x7f).contains(&byte));
    let psk = passphrase.len() == 64 && passphrase.bytes().all(|byte| byte.is_ascii_hexdigit());
    psk || printable && (8..=63).contains(&passphrase.len())
}

/// `ssid` as an option line writes it: `%`, whitespace and control
/// characters as `%XX` (`options::Ssid`).
fn escaped(ssid: &str) -> String {
    let mut out = String::new();
    for character in ssid.chars() {
        if character == '%' || character.is_whitespace() || character.is_control() {
            let mut bytes = [0u8; 4];
            for byte in character.encode_utf8(&mut bytes).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(character);
        }
    }
    out
}
