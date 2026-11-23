//! Bringing the window to the front on Wayland with an XDG activation token.
//!
//! A Wayland client may raise its own window only with a token the compositor
//! issued for a user action. A tray host that hands one over (GNOME's
//! AppIndicator extension does, through `ProvideXdgActivationToken`) also
//! starts a launch sequence for it, which shows the busy cursor until the
//! token is used: left unused, the cursor spins until the compositor gives up.
//! winit has no call to activate an open window with a given token, so this
//! sends `xdg_activation_v1.activate` itself, on its own queue of the
//! window's Wayland connection.

use iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

/// Activates `window` with `token`. `false` when it is no Wayland window (X11)
/// or the compositor lacks xdg-activation, so the caller can fall back to an
/// ordinary focus request.
#[allow(
    unsafe_code,
    reason = "the window's own Wayland display and surface, from winit"
)]
pub(super) fn activate(window: &dyn iced::window::Window, token: &str) -> bool {
    let (Ok(display), Ok(handle)) = (window.display_handle(), window.window_handle()) else {
        return false;
    };
    let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) =
        (display.as_raw(), handle.as_raw())
    else {
        return false;
    };
    // SAFETY: both pointers come from the live window winit owns, and this
    // runs on the event-loop thread while that window exists. A connection
    // made from a foreign display never closes it when dropped.
    let result = unsafe { send(display.display.as_ptr(), surface.surface.as_ptr(), token) };
    match result {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("could not activate the window with the tray's token: {e}");
            false
        }
    }
}

/// # Safety
/// `display` must be a live `wl_display` and `surface` a live `wl_surface` of it.
#[allow(
    unsafe_code,
    reason = "wrapping the window's foreign Wayland display and surface"
)]
unsafe fn send(
    display: *mut std::ffi::c_void,
    surface: *mut std::ffi::c_void,
    token: &str,
) -> Result<(), String> {
    // SAFETY: see the caller.
    let backend = unsafe { Backend::from_foreign_display(display.cast()) };
    let conn = Connection::from_backend(backend);
    // SAFETY: see the caller.
    let surface_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }
        .map_err(|e| e.to_string())?;
    let surface = WlSurface::from_id(&conn, surface_id).map_err(|e| e.to_string())?;
    let (globals, queue) = registry_queue_init::<Registry>(&conn).map_err(|e| e.to_string())?;
    let activation: XdgActivationV1 = globals
        .bind(&queue.handle(), 1..=1, ())
        .map_err(|e| e.to_string())?;
    activation.activate(token.to_string(), &surface);
    activation.destroy();
    conn.flush().map_err(|e| e.to_string())
}

/// The state of the private queue: nothing to keep.
struct Registry;

impl Dispatch<WlRegistry, GlobalListContents> for Registry {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationV1, ()> for Registry {
    fn event(
        _: &mut Self,
        _: &XdgActivationV1,
        _: <XdgActivationV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
