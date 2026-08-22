// Copyright 2022-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod platform;
#[cfg(all(
    any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ),
    not(target_env = "ohos"),
    feature = "gtk"
))]
#[path = "gtk/mod.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod platform;
#[cfg(target_env = "ohos")]
#[path = "ohos/mod.rs"]
mod platform;

use std::{
    cell::{Ref, RefCell, RefMut},
    rc::Rc,
};

use crate::{items::*, IsMenuItem, MenuItemKind, MenuItemType};

pub(crate) use self::platform::*;

// Public re-export so tray-icon can inject the MenuClient at startup.
// muda does not hold an OpenHarmonyApp; tray-icon's set_ohos_app creates the client.
#[cfg(target_env = "ohos")]
pub use self::platform::set_menu_client;

/// Public re-export so tray-icon can bridge StatusBar menu clicks into muda's
/// menu event channel.
#[cfg(target_env = "ohos")]
pub use self::platform::send_menu_event;

/// Public re-export so tauri's window helpers can dispatch menu bridge calls
/// onto muda's single FIFO worker, serialising set_menu/remove_menu/visibility
/// operations (fixes Menu Bar flicker when real data arrives 1ms before the
/// empty "remove_menu" dispatch from a different executor).
#[cfg(target_env = "ohos")]
pub use self::platform::dispatch_menu_bridge_call;

impl dyn IsMenuItem + '_ {
    fn child(&self) -> Rc<RefCell<MenuChild>> {
        match self.kind() {
            MenuItemKind::MenuItem(i) => i.inner,
            MenuItemKind::Submenu(i) => i.inner,
            MenuItemKind::Predefined(i) => i.inner,
            MenuItemKind::Check(i) => i.inner,
            MenuItemKind::Icon(i) => i.inner,
        }
    }
}

/// Internal utilities
impl MenuChild {
    fn kind(&self, c: Rc<RefCell<MenuChild>>) -> MenuItemKind {
        match self.item_type() {
            MenuItemType::Submenu => {
                let id = c.borrow().id().clone();
                MenuItemKind::Submenu(Submenu {
                    id: Rc::new(id),
                    inner: c,
                })
            }
            MenuItemType::MenuItem => {
                let id = c.borrow().id().clone();
                MenuItemKind::MenuItem(MenuItem {
                    id: Rc::new(id),
                    inner: c,
                })
            }
            MenuItemType::Predefined => {
                let id = c.borrow().id().clone();
                MenuItemKind::Predefined(PredefinedMenuItem {
                    id: Rc::new(id),
                    inner: c,
                })
            }
            MenuItemType::Check => {
                let id = c.borrow().id().clone();
                MenuItemKind::Check(CheckMenuItem {
                    id: Rc::new(id),
                    inner: c,
                })
            }
            MenuItemType::Icon => {
                let id = c.borrow().id().clone();
                MenuItemKind::Icon(IconMenuItem {
                    id: Rc::new(id),
                    inner: c,
                })
            }
        }
    }
}

#[allow(unused)]
impl MenuItemKind {
    pub(crate) fn as_ref(&self) -> &dyn IsMenuItem {
        match self {
            MenuItemKind::MenuItem(i) => i,
            MenuItemKind::Submenu(i) => i,
            MenuItemKind::Predefined(i) => i,
            MenuItemKind::Check(i) => i,
            MenuItemKind::Icon(i) => i,
        }
    }

    pub(crate) fn child(&self) -> Ref<'_, MenuChild> {
        match self {
            MenuItemKind::MenuItem(i) => i.inner.borrow(),
            MenuItemKind::Submenu(i) => i.inner.borrow(),
            MenuItemKind::Predefined(i) => i.inner.borrow(),
            MenuItemKind::Check(i) => i.inner.borrow(),
            MenuItemKind::Icon(i) => i.inner.borrow(),
        }
    }

    pub(crate) fn child_mut(&self) -> RefMut<'_, MenuChild> {
        match self {
            MenuItemKind::MenuItem(i) => i.inner.borrow_mut(),
            MenuItemKind::Submenu(i) => i.inner.borrow_mut(),
            MenuItemKind::Predefined(i) => i.inner.borrow_mut(),
            MenuItemKind::Check(i) => i.inner.borrow_mut(),
            MenuItemKind::Icon(i) => i.inner.borrow_mut(),
        }
    }
}
