// Copyright 2022-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

pub(crate) mod icon;

pub(crate) use icon::PlatformIcon;

use crate::{
    accelerator::KeyAccelerator,
    icon::{Icon, NativeIcon},
    items::PredefinedMenuItemType,
    util::{AddOp, Counter},
    IsMenuItem, MenuId, MenuItemType,
};
use keyboard_types::{Key, Modifiers};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
};

impl fmt::Display for KeyAccelerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(Modifiers::CONTROL) {
            write!(f, "Ctrl+")?;
        }
        if self.mods.contains(Modifiers::SHIFT) {
            write!(f, "Shift+")?;
        }
        if self.mods.contains(Modifiers::ALT) {
            write!(f, "Alt+")?;
        }
        if self.mods.contains(Modifiers::SUPER) {
            write!(f, "Super+")?;
        }
        match &self.key {
            Key::Character(s) => match s.as_str() {
                " " => write!(f, "Space"),
                c => write!(f, "{}", c.to_uppercase()),
            },
            Key::Tab => write!(f, "Tab"),
            Key::Escape => write!(f, "Esc"),
            Key::Delete => write!(f, "Del"),
            Key::Insert => write!(f, "Ins"),
            Key::PageUp => write!(f, "PgUp"),
            Key::PageDown => write!(f, "PgDn"),
            Key::ArrowLeft => write!(f, "Left"),
            Key::ArrowRight => write!(f, "Right"),
            Key::ArrowUp => write!(f, "Up"),
            Key::ArrowDown => write!(f, "Down"),
            key => write!(f, "{:?}", key),
        }
    }
}

// Use openharmony-ability-plugin-menu's MenuItemData for serialization
use openharmony_ability_plugin_menu::MenuItemData;

static COUNTER: Counter = Counter::new();

static CHECK_ITEMS: once_cell::sync::Lazy<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

// ─── MenuClient initialization (injected by tray-icon or tauri) ─────────────
// muda does not hold an OpenHarmonyApp reference; tray-icon's set_ohos_app
// creates a MenuClient and injects it here via set_menu_client().

static MENU_CLIENT: once_cell::sync::OnceCell<openharmony_ability_plugin_menu::MenuClient> =
    once_cell::sync::OnceCell::new();

/// Called by tray-icon (or tauri) at startup to inject the MenuClient.
/// muda does not create its own MenuClient (it does not hold OpenHarmonyApp).
pub fn set_menu_client(client: openharmony_ability_plugin_menu::MenuClient) {
    if MENU_CLIENT.set(client).is_err() {
        panic!("MENU_CLIENT already set");
    }
    // Eagerly initialize the event channel so the sender is registered with
    // plugin-menu before any bridge event can arrive.
    let _ = menu_event_receiver();
}

pub(crate) fn get_menu_client() -> &'static openharmony_ability_plugin_menu::MenuClient {
    MENU_CLIENT.get().expect("MENU_CLIENT not initialized")
}

// ─── Menu event channel (owned by muda) ─────────────────────────────────────
// muda owns the MENU_EVENT_CHANNEL. The Sender is registered with plugin-menu
// (via register_menu_event_sender) so on_main_thread_event can forward decoded
// menu_id strings here. tray-icon also calls send_menu_event() to inject tray
// menu clicks into this channel. The event listener thread (start_event_listener)
// consumes from menu_event_receiver().

static MENU_EVENT_CHANNEL: once_cell::sync::Lazy<(
    crossbeam_channel::Sender<String>,
    crossbeam_channel::Receiver<String>,
)> = once_cell::sync::Lazy::new(|| {
    let (tx, rx) = crossbeam_channel::unbounded();
    // Register our sender with plugin-menu so bridge events reach this channel.
    openharmony_ability_plugin_menu::register_menu_event_sender(tx.clone());
    (tx, rx)
});

/// Returns the menu event receiver (consumed by muda's event listener thread).
pub fn menu_event_receiver() -> &'static crossbeam_channel::Receiver<String> {
    &MENU_EVENT_CHANNEL.1
}

/// Sends a menu event into muda's channel (called by tray-icon to bridge
/// StatusBar menu clicks into muda's event stream).
pub fn send_menu_event(menu_id: String) {
    let _ = MENU_EVENT_CHANNEL.0.send(menu_id);
}

// ─── Bridge worker thread ───────────────────────────────────────────────────
// All MenuClient bridge calls must run on a Rust worker thread, never on the
// ArkTS/N-API main thread. The main thread owns the TSFN queue; blocking it
// with futures_executor::block_on prevents the very TSFN callbacks that deliver
// bridge responses, causing a deadlock (THREAD_BLOCK_6S watchdog). Same pattern
// as tray-icon's bridge worker. A single FIFO queue serialises menu operations
// (e.g. set-menubar before popup) and frees the main thread to pump the TSFN
// event loop so pending calls (tray add, menu set-menubar) can complete.

type MenuBridgeCommand = Box<dyn FnOnce() + Send + 'static>;

fn menu_bridge_worker_tx() -> &'static std::sync::mpsc::Sender<MenuBridgeCommand> {
    static TX: once_cell::sync::Lazy<std::sync::mpsc::Sender<MenuBridgeCommand>> =
        once_cell::sync::Lazy::new(|| {
            let (tx, rx) = std::sync::mpsc::channel::<MenuBridgeCommand>();
            std::thread::Builder::new()
                .name("menu-bridge".to_string())
                .spawn(move || {
                    log::debug!("[muda] menu bridge worker started");
                    while let Ok(cmd) = rx.recv() {
                        cmd();
                    }
                    log::debug!("[muda] menu bridge worker exiting");
                })
                .expect("Failed to spawn menu-bridge worker thread");
            tx
        });
    &TX
}

/// Dispatch a menu bridge call to the dedicated worker thread (fire-and-forget).
pub fn dispatch_menu_bridge_call(f: impl FnOnce() + Send + 'static) {
    if menu_bridge_worker_tx().send(Box::new(f)).is_err() {
        log::warn!("[muda] menu bridge worker channel closed, call dropped");
    }
}

pub struct Menu {
    id: MenuId,
    children: Vec<Rc<RefCell<MenuChild>>>,
}

impl Menu {
    pub fn new(id: Option<MenuId>) -> Self {
        Self {
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            children: Vec::new(),
        }
    }

    pub fn id(&self) -> &MenuId {
        &self.id
    }

    pub fn add_menu_item(&mut self, item: &dyn IsMenuItem, op: AddOp) -> crate::Result<()> {
        match op {
            AddOp::Append => self.children.push(item.child()),
            AddOp::Insert(position) => self.children.insert(position, item.child()),
        }
        Ok(())
    }

    pub fn remove(&mut self, item: &dyn IsMenuItem) -> crate::Result<()> {
        let index = self
            .children
            .iter()
            .position(|e: &Rc<RefCell<MenuChild>>| e.borrow().id == item.id())
            .ok_or(crate::Error::NotAChildOfThisMenu)?;
        self.children.remove(index);
        Ok(())
    }

    pub fn items(&self) -> Vec<crate::MenuItemKind> {
        self.children
            .iter()
            .map(|c: &Rc<RefCell<MenuChild>>| c.borrow().kind(c.clone()))
            .collect()
    }

    pub fn to_menu_items(&self) -> Vec<MenuItemData> {
        self.children
            .iter()
            .map(|c| c.borrow().to_menu_item_data())
            .collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.to_menu_items()).unwrap_or_default()
    }

    pub fn popup(&self, x: Option<f64>, y: Option<f64>, window_id: &str) -> crate::Result<()> {
        init_menu_event_listener();
        collect_check_items(&self.children);
        let json = self.to_json();
        let request = openharmony_ability_plugin_menu::MenuPopupRequest {
            json_data: json,
            x,
            y,
            window_id: window_id.to_string(),
        };
        // Dispatch to the menu bridge worker (fire-and-forget). The bridge call
        // must NOT run on the main thread: block_on + receiver.await would
        // deadlock the TSFN event loop (THREAD_BLOCK_6S).
        log::info!("[muda] popup: dispatching to worker");
        dispatch_menu_bridge_call(move || {
            let client = get_menu_client();
            log::info!("[muda] worker: popup before block_on");
            if let Err(e) = futures_executor::block_on(client.popup(request)) {
                log::warn!("[muda] popup error in worker: {}", e);
            }
        });
        Ok(())
    }

    pub fn refresh_menubar(&self, window_id: &str) -> crate::Result<()> {
        init_menu_event_listener();
        let json = self.to_json();
        let request = openharmony_ability_plugin_menu::MenuSetMenubarRequest {
            json_data: json,
            window_id: window_id.to_string(),
        };
        // Dispatch to the menu bridge worker (fire-and-forget). Without this,
        // window creation's refresh_menubar blocks the main thread on
        // set_menubar's receiver.await → deadlock (THREAD_BLOCK_6S).
        log::info!("[muda] refresh_menubar: dispatching to worker");
        dispatch_menu_bridge_call(move || {
            let client = get_menu_client();
            log::info!("[muda] worker: set_menubar before block_on");
            if let Err(e) = futures_executor::block_on(client.set_menubar(request)) {
                log::warn!("[muda] set_menubar error in worker: {}", e);
            }
        });
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct MenuChild {
    item_type: MenuItemType,
    text: String,
    enabled: bool,
    id: MenuId,
    accelerator: Option<KeyAccelerator>,
    predefined_item_type: Option<PredefinedMenuItemType>,
    checked: Option<Arc<AtomicBool>>,
    is_syncing_checked_state: Option<Arc<AtomicBool>>,
    icon: Option<Icon>,
    native_icon: Option<String>,
    pub children: Option<Vec<Rc<RefCell<MenuChild>>>>,
}

impl MenuChild {
    pub fn new(
        text: &str,
        enabled: bool,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::MenuItem,
            checked: None,
            children: None,
            icon: None,
            native_icon: None,
            is_syncing_checked_state: None,
            predefined_item_type: None,
        }
    }

    pub fn new_submenu(text: &str, enabled: bool, id: Option<MenuId>) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            children: Some(Vec::new()),
            item_type: MenuItemType::Submenu,
            icon: None,
            native_icon: None,
            is_syncing_checked_state: None,
            predefined_item_type: None,
            accelerator: None,
            checked: None,
        }
    }

    pub(crate) fn new_predefined(item_type: PredefinedMenuItemType, text: Option<String>) -> Self {
        Self {
            text: text.unwrap_or_else(|| item_type.text().to_string()),
            enabled: true,
            accelerator: item_type.accelerator().map(Into::into),
            id: MenuId(COUNTER.next().to_string()),
            item_type: MenuItemType::Predefined,
            predefined_item_type: Some(item_type),
            checked: None,
            children: None,
            icon: None,
            native_icon: None,
            is_syncing_checked_state: None,
        }
    }

    pub fn new_check(
        text: &str,
        enabled: bool,
        checked: bool,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            checked: Some(Arc::new(AtomicBool::new(checked))),
            is_syncing_checked_state: Some(Arc::new(AtomicBool::new(false))),
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Check,
            children: None,
            icon: None,
            native_icon: None,
            predefined_item_type: None,
        }
    }

    pub fn new_icon(
        text: &str,
        enabled: bool,
        icon: Option<Icon>,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            icon,
            native_icon: None,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Icon,
            children: None,
            checked: None,
            is_syncing_checked_state: None,
            predefined_item_type: None,
        }
    }

    pub fn new_native_icon(
        text: &str,
        enabled: bool,
        native_icon: Option<NativeIcon>,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Icon,
            children: None,
            checked: None,
            icon: None,
            native_icon: native_icon.and_then(native_icon_to_ohos).map(|s| s.to_string()),
            is_syncing_checked_state: None,
            predefined_item_type: None,
        }
    }
}

/// Map NativeIcon variants to OHOS system symbol resource names.
///
/// Only a few NativeIcon variants have confirmed OHOS system symbol equivalents
/// at API 12 (compileSdkVersion 5.0.0). Most of the 56 NativeIcon variants are
/// macOS-specific UI metaphors with no OHOS counterpart.
///
/// Validated symbols: ohos_star (Add), ohos_lock (LockLocked), ohos_wifi (Network).
/// All other variants map to `None` (no icon), consistent with Windows/Linux behavior
/// where unmapped NativeIcons render without an icon.
///
/// When the SDK adds more system symbols (e.g. ohos_trash, ohos_share), this mapping
/// can be extended. The ArkTS side (MenuBarComponent.nativeIconSymbol) must also be
/// updated with a matching `$r()` case.
fn native_icon_to_ohos(icon: NativeIcon) -> Option<&'static str> {
    match icon {
        NativeIcon::Add => Some("sys.symbol.ohos_star"),
        NativeIcon::LockLocked => Some("sys.symbol.ohos_lock"),
        NativeIcon::Network => Some("sys.symbol.ohos_wifi"),
        // All other variants: no confirmed system symbol at API 12
        _ => None,
    }
}

impl MenuChild {
    pub(crate) fn item_type(&self) -> MenuItemType {
        self.item_type
    }

    pub fn to_menu_item_data(&self) -> MenuItemData {
        let item_type = match self.item_type {
            MenuItemType::MenuItem => "item",
            MenuItemType::Submenu => "submenu",
            MenuItemType::Predefined => "predefined",
            MenuItemType::Check => "check",
            MenuItemType::Icon => "icon",
        };

        let predefined_type = self.predefined_item_type.as_ref().map(|t| {
            match t {
                PredefinedMenuItemType::Separator => "separator",
                PredefinedMenuItemType::Copy => "copy",
                PredefinedMenuItemType::Cut => "cut",
                PredefinedMenuItemType::Paste => "paste",
                PredefinedMenuItemType::SelectAll => "selectAll",
                PredefinedMenuItemType::Undo => "undo",
                PredefinedMenuItemType::Redo => "redo",
                PredefinedMenuItemType::Minimize => "minimize",
                PredefinedMenuItemType::Maximize => "maximize",
                PredefinedMenuItemType::Fullscreen => "fullscreen",
                PredefinedMenuItemType::CloseWindow => "close",
                PredefinedMenuItemType::Quit => "quit",
                PredefinedMenuItemType::Hide => "hide",
                PredefinedMenuItemType::HideOthers => "hideOthers",
                PredefinedMenuItemType::ShowAll => "showAll",
                PredefinedMenuItemType::About(_) => "about",
                PredefinedMenuItemType::Services => "services",
                PredefinedMenuItemType::BringAllToFront => "bringAllToFront",
                PredefinedMenuItemType::None => "none",
            }
        }).map(|s| s.to_string());

        MenuItemData {
            id: self.id.0.clone(),
            item_type: item_type.to_string(),
            text: Some(self.text.replace("&", "")),
            enabled: Some(self.enabled),
            accelerator: self.accelerator.clone().map(|k| k.to_string()),
            predefined_type,
            checked: self.checked.as_ref().map(|c| c.load(Ordering::Relaxed)),
            icon: self.icon.as_ref().map(|i| {
                let png_data = encode_rgba_to_png(&i.inner.raw, i.inner.width, i.inner.height);
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_data)
            }),
            native_icon: self.native_icon.clone(),
            submenu_items: self.children.as_ref().map(|c| {
                c.iter().map(|child| child.borrow().to_menu_item_data()).collect()
            }),
            about_metadata: self.predefined_item_type.as_ref().and_then(|t| {
                if let PredefinedMenuItemType::About(ref metadata) = t {
                    metadata.as_ref().map(|m| openharmony_ability_plugin_menu::AboutMetadataData {
                        name: m.name.clone(),
                        version: m.version.clone(),
                        short_version: m.short_version.clone(),
                        authors: m.authors.clone(),
                        comments: m.comments.clone(),
                        copyright: m.copyright.clone(),
                        license: m.license.clone(),
                        website: m.website.clone(),
                    })
                } else {
                    None
                }
            }),
        }
    }

    pub fn id(&self) -> &MenuId {
        &self.id
    }

    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_key_accelerator(
        &mut self,
        accelerator: Option<KeyAccelerator>,
    ) -> crate::Result<()> {
        self.accelerator = accelerator;
        Ok(())
    }
}

impl MenuChild {
    pub fn is_checked(&self) -> bool {
        self.checked
            .as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn set_checked(&mut self, checked: bool) {
        if let Some(c) = &self.checked {
            c.store(checked, Ordering::Release);
        }
        if let Some(is_syncing) = &self.is_syncing_checked_state {
            is_syncing.store(false, Ordering::Release);
        }
    }
}

impl MenuChild {
pub fn set_icon(&mut self, icon: Option<Icon>) {
        self.icon = icon;
    }

    pub fn set_native_icon(&mut self, icon: Option<NativeIcon>) {
        self.native_icon = icon.and_then(native_icon_to_ohos).map(|s| s.to_string());
    }
}

impl MenuChild {
    pub fn add_menu_item(&mut self, item: &dyn IsMenuItem, op: AddOp) -> crate::Result<()> {
        match op {
            AddOp::Append => self.children.as_mut().unwrap().push(item.child()),
            AddOp::Insert(position) => self
                .children
                .as_mut()
                .unwrap()
                .insert(position, item.child()),
        }
        Ok(())
    }

    pub fn remove(&mut self, item: &dyn IsMenuItem) -> crate::Result<()> {
        let index = self
            .children
            .as_ref()
            .unwrap()
            .iter()
            .position(|e: &Rc<RefCell<MenuChild>>| e.borrow().id == item.id())
            .ok_or(crate::Error::NotAChildOfThisMenu)?;
        self.children.as_mut().unwrap().remove(index);
        Ok(())
    }

    pub fn items(&self) -> Vec<crate::MenuItemKind> {
        self.children
            .as_ref()
            .unwrap()
            .iter()
            .map(|c: &Rc<RefCell<MenuChild>>| c.borrow().kind(c.clone()))
            .collect()
    }

    pub fn to_json(&self) -> String {
        let items: Vec<MenuItemData> = self.children.as_ref().map(|c| {
            c.iter().map(|child| child.borrow().to_menu_item_data()).collect()
        }).unwrap_or_default();
        serde_json::to_string(&items).unwrap_or_default()
    }

    pub fn popup(&self, x: Option<f64>, y: Option<f64>, window_id: &str) -> crate::Result<()> {
        init_menu_event_listener();
        if let Some(ref children) = self.children {
            collect_check_items(children);
        }
        let json = self.to_json();
        let request = openharmony_ability_plugin_menu::MenuPopupRequest {
            json_data: json,
            x,
            y,
            window_id: window_id.to_string(),
        };
        // Dispatch to the menu bridge worker (fire-and-forget) — see Menu::popup.
        log::info!("[muda] submenu popup: dispatching to worker");
        dispatch_menu_bridge_call(move || {
            let client = get_menu_client();
            if let Err(e) = futures_executor::block_on(client.popup(request)) {
                log::warn!("[muda] popup error in submenu worker: {}", e);
            }
        });
        Ok(())
    }
}

fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
    }
    png_data
}

static EVENT_LISTENER_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn collect_check_items(children: &[Rc<RefCell<MenuChild>>]) {
    let mut guard = CHECK_ITEMS.lock().unwrap();
    guard.clear();
    for child in children {
        collect_check_item_recursive(&child.borrow(), &mut guard);
    }
}

fn collect_check_item_recursive(child: &MenuChild, map: &mut HashMap<String, Arc<AtomicBool>>) {
    if child.item_type == MenuItemType::Check {
        if let Some(ref checked) = child.checked {
            map.insert(child.id.0.clone(), checked.clone());
        }
    }
    if let Some(ref sub_children) = child.children {
        for sub in sub_children {
            collect_check_item_recursive(&sub.borrow(), map);
        }
    }
}

fn start_event_listener() {
    if EVENT_LISTENER_STARTED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }

    std::thread::spawn(|| {
        let receiver = menu_event_receiver();
        while let Ok(menu_id) = receiver.recv() {
            {
                let guard = CHECK_ITEMS.lock().unwrap();
                if let Some(checked) = guard.get(&menu_id) {
                    let old = checked.load(Ordering::Relaxed);
                    checked.store(!old, Ordering::Release);
                }
            }
            crate::MenuEvent::send(crate::MenuEvent {
                id: crate::MenuId::new(menu_id),
            });
        }
    });
}

pub fn init_menu_event_listener() {
    start_event_listener();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::PredefinedMenuItemType;
    use keyboard_types::{Key, Modifiers};

    #[test]
    fn to_menu_item_data_accelerator() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()));
        let child = MenuChild::new("Open", true, Some(accel), None);
        let data = child.to_menu_item_data();
        assert!(data.accelerator.is_some());
        assert_eq!(data.accelerator.unwrap(), "Ctrl+O");
    }

    #[test]
    fn submenu_accelerator_preserved() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Key::Character("S".into()));
        let mut child = MenuChild::new_submenu("Save", true, None);
        child.set_key_accelerator(Some(accel)).unwrap();
        let data = child.to_menu_item_data();
        assert!(data.accelerator.is_some());
        assert_eq!(data.accelerator.unwrap(), "Ctrl+Shift+S");
    }

    #[test]
    fn about_metadata_in_menu_item_data() {
        let metadata = crate::AboutMetadata {
            name: Some("TestApp".to_string()),
            version: Some("1.0.0".to_string()),
            short_version: Some("1.0".to_string()),
            authors: Some(vec!("Alice".to_string(), "Bob".to_string())),
            copyright: Some("TestCorp".to_string()),
            ..Default::default()
        };
        let child = MenuChild::new_predefined(
            PredefinedMenuItemType::About(Some(metadata)),
            None,
        );
        let data = child.to_menu_item_data();
        assert!(data.about_metadata.is_some());
        let meta = data.about_metadata.unwrap();
        assert_eq!(meta.name.unwrap(), "TestApp");
        assert_eq!(meta.version.unwrap(), "1.0.0");
        assert_eq!(meta.short_version.unwrap(), "1.0");
        assert_eq!(meta.authors.unwrap(), vec!("Alice".to_string(), "Bob".to_string()));
        assert_eq!(meta.copyright.unwrap(), "TestCorp");
        assert!(meta.comments.is_none());
        assert!(meta.license.is_none());
        assert!(meta.website.is_none());
    }

    #[test]
    fn predefined_about_without_metadata() {
        let child = MenuChild::new_predefined(
            PredefinedMenuItemType::About(None),
            Some("About".to_string()),
        );
        let data = child.to_menu_item_data();
        assert!(data.about_metadata.is_none());
        assert_eq!(data.predefined_type.unwrap(), "about");
    }

    #[test]
    fn key_accelerator_display_ctrl_only() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()));
        assert_eq!(accel.to_string(), "Ctrl+O");
    }

    #[test]
    fn key_accelerator_display_ctrl_shift() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Key::Character("S".into()));
        assert_eq!(accel.to_string(), "Ctrl+Shift+S");
    }

    #[test]
    fn key_accelerator_display_ctrl_alt() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::ALT), Key::Character("D".into()));
        assert_eq!(accel.to_string(), "Ctrl+Alt+D");
    }

    #[test]
    fn key_accelerator_display_full_combo() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT | Modifiers::ALT), Key::Character("X".into()));
        assert_eq!(accel.to_string(), "Ctrl+Shift+Alt+X");
    }

    #[test]
    fn key_accelerator_display_no_modifiers() {
        let accel = KeyAccelerator::new(None, Key::Character("A".into()));
        assert_eq!(accel.to_string(), "A");
    }

    #[test]
    fn key_accelerator_display_special_keys() {
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Escape).to_string(), "Ctrl+Esc");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Delete).to_string(), "Ctrl+Del");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Insert).to_string(), "Ctrl+Ins");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::PageUp).to_string(), "Ctrl+PgUp");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::PageDown).to_string(), "Ctrl+PgDn");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Tab).to_string(), "Ctrl+Tab");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character(" ".into())).to_string(), "Ctrl+Space");
    }

    #[test]
    fn key_accelerator_display_arrow_keys() {
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowLeft).to_string(), "Ctrl+Left");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowRight).to_string(), "Ctrl+Right");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowUp).to_string(), "Ctrl+Up");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowDown).to_string(), "Ctrl+Down");
    }

    #[test]
    fn key_accelerator_display_lowercase_input() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("o".into()));
        assert_eq!(accel.to_string(), "Ctrl+O");
    }

    #[test]
    fn menu_child_disabled_item() {
        let child = MenuChild::new("Disabled", false, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.enabled, Some(false));
        assert_eq!(data.text, Some("Disabled".to_string()));
        assert!(data.accelerator.is_none());
    }

    #[test]
    fn menu_child_enabled_item() {
        let child = MenuChild::new("Enabled", true, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.enabled, Some(true));
    }

    #[test]
    fn menu_child_ampersand_stripped() {
        let child = MenuChild::new("Save &As", true, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.text, Some("Save As".to_string()));
    }

    #[test]
    fn menu_child_double_ampersand_stripped() {
        let child = MenuChild::new("A&&B", true, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.text, Some("AB".to_string()));
    }

    #[test]
    fn menu_child_checked_item() {
        let mut child = MenuChild::new_check("Toggle", true, true, None, None);
        child.checked = Some(Arc::new(AtomicBool::new(true)));
        let data = child.to_menu_item_data();
        assert_eq!(data.checked, Some(true));
        assert_eq!(data.item_type, "check");
    }

    #[test]
    fn menu_child_checked_item_false() {
        let mut child = MenuChild::new_check("Toggle", true, false, None, None);
        child.checked = Some(Arc::new(AtomicBool::new(false)));
        let data = child.to_menu_item_data();
        assert_eq!(data.checked, Some(false));
    }

    #[test]
    fn menu_child_submenu_with_nested_items() {
        let mut submenu = MenuChild::new_submenu("File", true, None);
        let open_item = MenuChild::new("Open", true, Some(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()))), None);
        let save_item = MenuChild::new("Save", true, Some(KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Key::Character("S".into()))), None);
        submenu.children = Some(vec![Rc::new(RefCell::new(open_item)), Rc::new(RefCell::new(save_item))]);
        let data = submenu.to_menu_item_data();
        assert_eq!(data.item_type, "submenu");
        assert_eq!(data.text, Some("File".to_string()));
        let children = data.submenu_items.unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].accelerator, Some("Ctrl+O".to_string()));
        assert_eq!(children[1].accelerator, Some("Ctrl+Shift+S".to_string()));
    }

    #[test]
    fn menu_child_submenu_empty() {
        let submenu = MenuChild::new_submenu("Empty", true, None);
        let data = submenu.to_menu_item_data();
        assert!(data.submenu_items.is_some());
        assert_eq!(data.submenu_items.unwrap().len(), 0);
    }

    #[test]
    fn menu_child_set_text() {
        let mut child = MenuChild::new("Original", true, None, None);
        child.set_text("Updated");
        let data = child.to_menu_item_data();
        assert_eq!(data.text, Some("Updated".to_string()));
    }

    #[test]
    fn menu_child_set_enabled() {
        let mut child = MenuChild::new("Item", true, None, None);
        child.set_enabled(false);
        let data = child.to_menu_item_data();
        assert_eq!(data.enabled, Some(false));
    }

    #[test]
    fn menu_child_set_key_accelerator() {
        let mut child = MenuChild::new("Item", true, None, None);
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("Q".into()));
        child.set_key_accelerator(Some(accel)).unwrap();
        let data = child.to_menu_item_data();
        assert_eq!(data.accelerator, Some("Ctrl+Q".to_string()));
    }

    #[test]
    fn menu_child_remove_accelerator() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("P".into()));
        let mut child = MenuChild::new("Print", true, Some(accel), None);
        child.set_key_accelerator(None).unwrap();
        let data = child.to_menu_item_data();
        assert!(data.accelerator.is_none());
    }

    #[test]
    fn menu_child_predefined_hide() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Hide, Some("Hide".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.item_type, "predefined");
        assert_eq!(data.predefined_type.unwrap(), "hide");
    }

    #[test]
    fn menu_child_predefined_close() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::CloseWindow, Some("Close".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.predefined_type.unwrap(), "close");
    }

    #[test]
    fn menu_child_predefined_quit() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Quit, Some("Quit".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.predefined_type.unwrap(), "quit");
    }

    #[test]
    fn menu_child_predefined_fullscreen() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Fullscreen, Some("Fullscreen".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.predefined_type.unwrap(), "fullscreen");
    }

    #[test]
    fn menu_to_json_structure() {
        let mut menu = Menu::new(None);
        let item = MenuChild::new("Open", true, Some(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()))), None);
        menu.children.push(Rc::new(RefCell::new(item)));
        let json_str = menu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.is_array());
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "item");
        assert_eq!(items[0]["text"], "Open");
        assert_eq!(items[0]["accelerator"], "Ctrl+O");
    }

    #[test]
    fn menu_to_json_submenu_nested() {
        let mut menu = Menu::new(None);
        let mut submenu = MenuChild::new_submenu("File", true, None);
        let open = MenuChild::new("Open", true, None, Some(MenuId::new("open_id")));
        submenu.children = Some(vec![Rc::new(RefCell::new(open))]);
        menu.children.push(Rc::new(RefCell::new(submenu)));
        let json_str = menu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let top = parsed.as_array().unwrap();
        assert_eq!(top[0]["type"], "submenu");
        let children = top[0]["submenuItems"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["type"], "item");
        assert_eq!(children[0]["id"], "open_id");
    }

    // ─── native_icon_to_ohos ──────────────────────────────────────────────

    #[test]
    fn native_icon_add_maps_to_ohos_star() {
        assert_eq!(native_icon_to_ohos(NativeIcon::Add), Some("sys.symbol.ohos_star"));
    }

    #[test]
    fn native_icon_lock_locked_maps_to_ohos_lock() {
        assert_eq!(native_icon_to_ohos(NativeIcon::LockLocked), Some("sys.symbol.ohos_lock"));
    }

    #[test]
    fn native_icon_network_maps_to_ohos_wifi() {
        assert_eq!(native_icon_to_ohos(NativeIcon::Network), Some("sys.symbol.ohos_wifi"));
    }

    #[test]
    fn native_icon_unmapped_returns_none() {
        // Variants without a confirmed OHOS system symbol return None
        assert_eq!(native_icon_to_ohos(NativeIcon::Bluetooth), None);
        assert_eq!(native_icon_to_ohos(NativeIcon::Bookmarks), None);
        assert_eq!(native_icon_to_ohos(NativeIcon::Caution), None);
        assert_eq!(native_icon_to_ohos(NativeIcon::Folder), None);
        assert_eq!(native_icon_to_ohos(NativeIcon::TrashEmpty), None);
    }

    #[test]
    fn menu_child_new_native_icon_maps_icon() {
        let child = MenuChild::new_native_icon("Add", true, Some(NativeIcon::Add), None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.native_icon, Some("sys.symbol.ohos_star".to_string()));
    }

    #[test]
    fn menu_child_new_native_icon_unmapped_is_none() {
        let child = MenuChild::new_native_icon("Bluetooth", true, Some(NativeIcon::Bluetooth), None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.native_icon, None);
    }

    #[test]
    fn menu_child_set_native_icon_updates_mapping() {
        let mut child = MenuChild::new_native_icon("X", true, Some(NativeIcon::Bluetooth), None, None);
        assert_eq!(child.native_icon, None);
        child.set_native_icon(Some(NativeIcon::Add));
        assert_eq!(child.native_icon, Some("sys.symbol.ohos_star".to_string()));
    }

    #[test]
    fn menu_child_set_native_icon_to_none() {
        let mut child = MenuChild::new_native_icon("X", true, Some(NativeIcon::Add), None, None);
        assert!(child.native_icon.is_some());
        child.set_native_icon(None);
        assert_eq!(child.native_icon, None);
    }

    // ─── KeyAccelerator Display: additional branches ─────────────────────

    #[test]
    fn key_accelerator_display_super_modifier() {
        let accel = KeyAccelerator::new(Some(Modifiers::SUPER), Key::Character("X".into()));
        assert_eq!(accel.to_string(), "Super+X");
    }

    #[test]
    fn key_accelerator_display_all_four_modifiers() {
        let accel = KeyAccelerator::new(
            Some(Modifiers::CONTROL | Modifiers::SHIFT | Modifiers::ALT | Modifiers::SUPER),
            Key::Character("K".into()),
        );
        assert_eq!(accel.to_string(), "Ctrl+Shift+Alt+Super+K");
    }

    #[test]
    fn key_accelerator_display_shift_only() {
        let accel = KeyAccelerator::new(Some(Modifiers::SHIFT), Key::Character("A".into()));
        assert_eq!(accel.to_string(), "Shift+A");
    }

    #[test]
    fn key_accelerator_display_alt_only() {
        let accel = KeyAccelerator::new(Some(Modifiers::ALT), Key::Character("Z".into()));
        assert_eq!(accel.to_string(), "Alt+Z");
    }

    #[test]
    fn key_accelerator_display_fallback_unmapped_key() {
        // An unmapped Key variant uses the Debug fallback
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Enter);
        let s = accel.to_string();
        assert!(s.starts_with("Ctrl+"));
    }

    // ─── Menu add/remove/items ───────────────────────────────────────────

    #[test]
    fn menu_children_append() {
        let mut menu = Menu::new(None);
        let item = Rc::new(RefCell::new(MenuChild::new("Item1", true, None, None)));
        menu.children.push(item);
        assert_eq!(menu.children.len(), 1);
    }

    #[test]
    fn menu_children_insert_at_position() {
        let mut menu = Menu::new(None);
        let item1 = Rc::new(RefCell::new(MenuChild::new("Item1", true, None, None)));
        let item2 = Rc::new(RefCell::new(MenuChild::new("Item2", true, None, None)));
        menu.children.push(item1);
        menu.children.insert(0, item2);
        assert_eq!(menu.children.len(), 2);
        assert_eq!(menu.children[0].borrow().text(), "Item2");
        assert_eq!(menu.children[1].borrow().text(), "Item1");
    }

    #[test]
    fn menu_children_remove_by_index() {
        let mut menu = Menu::new(None);
        let item = Rc::new(RefCell::new(MenuChild::new("Item1", true, None, Some(MenuId::new("removable")))));
        menu.children.push(item);
        assert_eq!(menu.children.len(), 1);
        // Simulate remove: find by id then remove
        let idx = menu.children.iter().position(|c| c.borrow().id.0 == "removable").unwrap();
        menu.children.remove(idx);
        assert_eq!(menu.children.len(), 0);
    }

    #[test]
    fn menu_items_returns_kinds() {
        let mut menu = Menu::new(None);
        let item = Rc::new(RefCell::new(MenuChild::new("Item1", true, None, None)));
        menu.children.push(item);
        let kinds = menu.items();
        assert_eq!(kinds.len(), 1);
    }

    #[test]
    fn menu_to_menu_items_collects_data() {
        let mut menu = Menu::new(None);
        let item = Rc::new(RefCell::new(MenuChild::new("Open", true, None, None)));
        menu.children.push(item);
        let items = menu.to_menu_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, Some("Open".to_string()));
    }

    // ─── MenuChild add/remove/items ──────────────────────────────────────

    #[test]
    fn submenu_child_add_and_remove_via_children() {
        let mut submenu = MenuChild::new_submenu("File", true, None);
        let item = Rc::new(RefCell::new(MenuChild::new("Open", true, None, Some(MenuId::new("open_id")))));
        submenu.children.as_mut().unwrap().push(item);
        assert_eq!(submenu.children.as_ref().unwrap().len(), 1);

        // Remove by finding by id
        let idx = submenu.children.as_ref().unwrap().iter()
            .position(|c| c.borrow().id.0 == "open_id").unwrap();
        submenu.children.as_mut().unwrap().remove(idx);
        assert_eq!(submenu.children.as_ref().unwrap().len(), 0);
    }

    #[test]
    fn submenu_child_to_json() {
        let mut submenu = MenuChild::new_submenu("Edit", true, None);
        let cut = MenuChild::new("Cut", true, Some(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("X".into()))), None);
        submenu.children = Some(vec![Rc::new(RefCell::new(cut))]);
        let json_str = submenu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["text"], "Cut");
        assert_eq!(items[0]["accelerator"], "Ctrl+X");
    }

    #[test]
    fn submenu_child_to_json_empty() {
        let submenu = MenuChild::new_submenu("Empty", true, None);
        let json_str = submenu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.as_array().unwrap().is_empty());
    }

    // ─── Check item serialization ─────────────────────────────────────────

    #[test]
    fn menu_to_json_includes_check_item() {
        let mut menu = Menu::new(None);
        let check = MenuChild::new_check("Toggle", true, true, None, Some(MenuId::new("check_id")));
        menu.children.push(Rc::new(RefCell::new(check)));
        let json_str = menu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "check");
        assert_eq!(items[0]["checked"], true);
        assert_eq!(items[0]["id"], "check_id");
    }

    #[test]
    fn menu_to_json_includes_icon_item_with_icon() {
        let mut menu = Menu::new(None);
        let icon = Icon {
            inner: PlatformIcon {
                raw: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255],
                width: 2,
                height: 2,
            },
        };
        let icon_item = MenuChild::new_icon("Colored", true, Some(icon), None, None);
        menu.children.push(Rc::new(RefCell::new(icon_item)));
        let json_str = menu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "icon");
        assert!(items[0]["icon"].as_str().unwrap().len() > 0);
    }

    // ─── encode_rgba_to_png ───────────────────────────────────────────────

    #[test]
    fn encode_rgba_to_png_produces_valid_png() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let png = encode_rgba_to_png(&rgba, 2, 2);
        // PNG magic bytes
        assert!(png.len() > 8);
        assert_eq!(&png[1..4], b"PNG");
    }

    // ─── collect_check_items ──────────────────────────────────────────────

    #[test]
    fn collect_check_items_collects_check_states() {
        let children: Vec<Rc<RefCell<MenuChild>>> = vec![
            Rc::new(RefCell::new(MenuChild::new_check("A", true, true, None, Some(MenuId::new("c1"))))),
            Rc::new(RefCell::new(MenuChild::new("Regular", true, None, None))),
            Rc::new(RefCell::new(MenuChild::new_check("B", true, false, None, Some(MenuId::new("c2"))))),
        ];
        collect_check_items(&children);
        let map = CHECK_ITEMS.lock().unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("c1"));
        assert!(map.contains_key("c2"));
    }

    #[test]
    fn collect_check_items_recurses_into_submenus() {
        let mut submenu = MenuChild::new_submenu("Sub", true, None);
        let check = MenuChild::new_check("Nested", true, true, None, Some(MenuId::new("nested_check")));
        submenu.children = Some(vec![Rc::new(RefCell::new(check))]);
        let children: Vec<Rc<RefCell<MenuChild>>> = vec![
            Rc::new(RefCell::new(submenu)),
        ];
        collect_check_items(&children);
        let map = CHECK_ITEMS.lock().unwrap();
        assert!(map.contains_key("nested_check"));
    }

    #[test]
    fn collect_check_items_clears_previous_entries() {
        // First collect with one item
        let children1: Vec<Rc<RefCell<MenuChild>>> = vec![
            Rc::new(RefCell::new(MenuChild::new_check("A", true, true, None, Some(MenuId::new("old_check"))))),
        ];
        collect_check_items(&children1);
        assert!(CHECK_ITEMS.lock().unwrap().contains_key("old_check"));

        // Then collect with different items — old should be gone
        let children2: Vec<Rc<RefCell<MenuChild>>> = vec![
            Rc::new(RefCell::new(MenuChild::new_check("B", true, false, None, Some(MenuId::new("new_check"))))),
        ];
        collect_check_items(&children2);
        let map = CHECK_ITEMS.lock().unwrap();
        assert!(!map.contains_key("old_check"));
        assert!(map.contains_key("new_check"));
    }

    // ─── Checked state toggle ─────────────────────────────────────────────

    #[test]
    fn check_item_toggle_checked_state() {
        let mut child = MenuChild::new_check("Toggle", true, false, None, None);
        assert!(!child.is_checked());
        child.set_checked(true);
        assert!(child.is_checked());
        child.set_checked(false);
        assert!(!child.is_checked());
    }

    #[test]
    fn regular_item_is_checked_returns_false() {
        let child = MenuChild::new("Regular", true, None, None);
        assert!(!child.is_checked());
    }

    // ─── Predefined item types ────────────────────────────────────────────

    #[test]
    fn predefined_separator() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Separator, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.predefined_type.unwrap(), "separator");
    }

    #[test]
    fn predefined_copy_cut_paste_selectall() {
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Copy, None).to_menu_item_data().predefined_type.unwrap(), "copy");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Cut, None).to_menu_item_data().predefined_type.unwrap(), "cut");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Paste, None).to_menu_item_data().predefined_type.unwrap(), "paste");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::SelectAll, None).to_menu_item_data().predefined_type.unwrap(), "selectAll");
    }

    #[test]
    fn predefined_undo_redo() {
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Undo, None).to_menu_item_data().predefined_type.unwrap(), "undo");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Redo, None).to_menu_item_data().predefined_type.unwrap(), "redo");
    }

    #[test]
    fn predefined_window_ops() {
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Minimize, None).to_menu_item_data().predefined_type.unwrap(), "minimize");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Maximize, None).to_menu_item_data().predefined_type.unwrap(), "maximize");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Fullscreen, None).to_menu_item_data().predefined_type.unwrap(), "fullscreen");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::CloseWindow, None).to_menu_item_data().predefined_type.unwrap(), "close");
    }

    #[test]
    fn predefined_app_ops() {
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Quit, None).to_menu_item_data().predefined_type.unwrap(), "quit");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Hide, None).to_menu_item_data().predefined_type.unwrap(), "hide");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::HideOthers, None).to_menu_item_data().predefined_type.unwrap(), "hideOthers");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::ShowAll, None).to_menu_item_data().predefined_type.unwrap(), "showAll");
    }

    #[test]
    fn predefined_misc_ops() {
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::Services, None).to_menu_item_data().predefined_type.unwrap(), "services");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::BringAllToFront, None).to_menu_item_data().predefined_type.unwrap(), "bringAllToFront");
        assert_eq!(MenuChild::new_predefined(PredefinedMenuItemType::None, None).to_menu_item_data().predefined_type.unwrap(), "none");
    }

    #[test]
    fn predefined_separator_has_default_text() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Separator, None);
        let data = child.to_menu_item_data();
        // Separator default text comes from item_type.text()
        assert!(data.text.is_some());
    }

    #[test]
    fn predefined_custom_text_override() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Copy, Some("Copy to Clipboard".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.text, Some("Copy to Clipboard".to_string()));
    }

    // ─── Icon item ────────────────────────────────────────────────────────

    #[test]
    fn icon_item_serializes_icon_to_base64_png() {
        let icon = Icon {
            inner: PlatformIcon {
                raw: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255],
                width: 2,
                height: 2,
            },
        };
        let child = MenuChild::new_icon("Colored", true, Some(icon), None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.item_type, "icon");
        assert!(data.icon.is_some());
        // The icon should be valid base64
        let icon_b64 = data.icon.unwrap();
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &icon_b64).unwrap();
        // Should start with PNG magic
        assert_eq!(&decoded[1..4], b"PNG");
    }

    #[test]
    fn icon_item_without_icon() {
        let child = MenuChild::new_icon("NoIcon", true, None, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.item_type, "icon");
        assert!(data.icon.is_none());
    }

    // ─── Menu id ──────────────────────────────────────────────────────────

    #[test]
    fn menu_new_generates_id() {
        let menu = Menu::new(None);
        assert!(!menu.id().0.is_empty());
    }

    #[test]
    fn menu_new_uses_provided_id() {
        let menu = Menu::new(Some(MenuId::new("custom_menu")));
        assert_eq!(menu.id().0, "custom_menu");
    }

    #[test]
    fn menu_child_new_generates_id() {
        let child = MenuChild::new("Test", true, None, None);
        assert!(!child.id().0.is_empty());
    }

    #[test]
    fn menu_child_new_uses_provided_id() {
        let child = MenuChild::new("Test", true, None, Some(MenuId::new("child_id")));
        assert_eq!(child.id().0, "child_id");
    }

    // ─── skip_serializing_if: Option fields must be absent (not null) when None ──
    // This is a regression guard for the OHOS 401 error: if serde emits `null`
    // for None Option fields instead of omitting them, ArkTS statusBarManager
    // rejects the JSON with a 401 "check param error". The skip_serializing_if
    // attributes on MenuItemData must ensure absent, not null.

    #[test]
    fn to_json_none_options_are_absent_not_null() {
        // A plain MenuItem with no accelerator, icon, predefined_type, checked, etc.
        // All Option fields should be ABSENT from the JSON — not present as `null`.
        let child = MenuChild::new("Plain", true, None, Some(MenuId::new("plain_id")));
        let json_str = child.to_menu_item_data().id; // just verify struct works
        let _ = json_str;
        let data = child.to_menu_item_data();
        let json = serde_json::to_string(&data).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = parsed.as_object().unwrap();

        // These fields have skip_serializing_if = "Option::is_none" and should
        // be ABSENT from the JSON when None (not present as null)
        assert!(!obj.contains_key("accelerator"), "accelerator should be absent, not null");
        assert!(!obj.contains_key("predefinedType"), "predefinedType should be absent, not null");
        assert!(!obj.contains_key("checked"), "checked should be absent, not null");
        assert!(!obj.contains_key("icon"), "icon should be absent, not null");
        assert!(!obj.contains_key("nativeIcon"), "nativeIcon should be absent, not null");
        assert!(!obj.contains_key("submenuItems"), "submenuItems should be absent, not null");
        assert!(!obj.contains_key("aboutMetadata"), "aboutMetadata should be absent, not null");

        // These fields are always present (not Option with skip)
        assert!(obj.contains_key("id"));
        assert!(obj.contains_key("type"));
        assert!(obj.contains_key("text"));
        assert!(obj.contains_key("enabled"));
    }

    #[test]
    fn to_json_none_values_not_null_in_menu_json() {
        // Verify via Menu::to_json() that None Option fields produce absent keys
        // (not null values) in the serialized JSON array
        let mut menu = Menu::new(None);
        let item = Rc::new(RefCell::new(MenuChild::new("Plain", true, None, None)));
        menu.children.push(item);
        let json_str = menu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let items = parsed.as_array().unwrap();
        let obj = items[0].as_object().unwrap();

        // No null values should appear for absent Option fields
        for key in &["accelerator", "predefinedType", "checked", "icon", "nativeIcon", "submenuItems", "aboutMetadata"] {
            assert!(
                !obj.contains_key(*key),
                "{} should be absent (not null) when None — key present: {}",
                key, obj.contains_key(*key)
            );
        }
    }

    #[test]
    fn to_json_some_values_present_in_json() {
        // When Option fields are Some, they should be present in the JSON
        let child = MenuChild::new_check("Toggle", true, true, None, Some(MenuId::new("check_id")));
        let data = child.to_menu_item_data();
        let json = serde_json::to_string(&data).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = parsed.as_object().unwrap();

        assert!(obj.contains_key("checked"), "checked should be present when Some");
        assert_eq!(obj["checked"], serde_json::Value::Bool(true));
    }
}
