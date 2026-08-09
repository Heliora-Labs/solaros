use spin::Mutex;

#[derive(Clone, Copy, PartialEq)]
pub enum KbdLayout {
    TurkishQ,
    Us,
}

struct Settings {
    layout: KbdLayout,
}

static SETTINGS: Mutex<Settings> = Mutex::new(Settings {
    layout: KbdLayout::TurkishQ,
});

pub fn layout() -> KbdLayout {
    SETTINGS.lock().layout
}

pub fn layout_code() -> &'static str {
    match layout() {
        KbdLayout::TurkishQ => "trq",
        KbdLayout::Us => "en-us",
    }
}

pub fn set_layout(l: KbdLayout) {
    SETTINGS.lock().layout = l;
}