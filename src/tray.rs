use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    TogglePlay,
    Next,
    Previous,
    Quit,
}

/// Cross-platform System Tray / Menu Bar icon with quick playback controls.
pub struct SystemTrayBridge {
    _tray: Option<TrayIcon>,
    id_show: MenuId,
    id_play: MenuId,
    id_next: MenuId,
    id_prev: MenuId,
    id_quit: MenuId,
}

impl SystemTrayBridge {
    pub fn new(ephemeral_screenshot: bool) -> Self {
        let show_item = MenuItem::new("Show TxPlayer", true, None);
        let play_item = MenuItem::new("Play / Pause", true, None);
        let next_item = MenuItem::new("Next Track", true, None);
        let prev_item = MenuItem::new("Previous Track", true, None);
        let quit_item = MenuItem::new("Quit TxPlayer", true, None);

        let id_show = show_item.id().clone();
        let id_play = play_item.id().clone();
        let id_next = next_item.id().clone();
        let id_prev = prev_item.id().clone();
        let id_quit = quit_item.id().clone();

        if ephemeral_screenshot {
            return Self {
                _tray: None,
                id_show,
                id_play,
                id_next,
                id_prev,
                id_quit,
            };
        }

        let menu = Menu::new();
        let _ = menu.append(&show_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&play_item);
        let _ = menu.append(&next_item);
        let _ = menu.append(&prev_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit_item);

        let icon = load_tray_icon();
        let tray = icon.and_then(|ic| {
            TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(format!("TxPlayer v{}", crate::APP_VERSION))
                .with_icon(ic)
                .build()
                .ok()
        });

        Self {
            _tray: tray,
            id_show,
            id_play,
            id_next,
            id_prev,
            id_quit,
        }
    }

    pub fn poll(&self) -> Option<TrayCommand> {
        self._tray.as_ref()?;
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.id_show {
                return Some(TrayCommand::ShowWindow);
            } else if event.id == self.id_play {
                return Some(TrayCommand::TogglePlay);
            } else if event.id == self.id_next {
                return Some(TrayCommand::Next);
            } else if event.id == self.id_prev {
                return Some(TrayCommand::Previous);
            } else if event.id == self.id_quit {
                return Some(TrayCommand::Quit);
            }
        }
        if let Ok(event) = TrayIconEvent::receiver().try_recv()
            && let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
        {
            return Some(TrayCommand::ShowWindow);
        }
        None
    }
}

fn load_tray_icon() -> Option<tray_icon::Icon> {
    let img = image::load_from_memory(crate::APP_ICON_PNG).ok()?;
    let small = img.thumbnail(32, 32).into_rgba8();
    let (w, h) = small.dimensions();
    tray_icon::Icon::from_rgba(small.into_raw(), w, h).ok()
}
