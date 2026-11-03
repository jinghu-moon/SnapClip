//! Opt-in live desktop smoke test. Ordinary unit tests never create a desktop window.
#![cfg(windows)]

use std::time::{Duration, Instant};

use super::{ImageSource, NativeImages, PinEvent, PinRequest};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindow,
    WS_EX_NOACTIVATE,
};

fn next_event(manager: &NativeImages) -> PinEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = manager.try_recv_event() {
            return event;
        }
        assert!(
            Instant::now() < deadline,
            "pin host produced no lifecycle event"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn owned_pin() -> HWND {
    let class: Vec<u16> = "SnapClipPinWindow\0".encode_utf16().collect();
    let mut previous = std::ptr::null_mut();
    loop {
        let hwnd = unsafe {
            FindWindowExW(
                std::ptr::null_mut(),
                previous,
                class.as_ptr(),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return hwnd;
        }
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == unsafe { GetCurrentProcessId() } {
            return hwnd;
        }
        previous = hwnd;
    }
}

#[test]
#[ignore = "requires an interactive Windows desktop; creates and closes real pin windows"]
fn prepared_and_latest_images_share_ids_and_shutdown_closes_the_host() {
    let manager = NativeImages::start().unwrap();
    let request = PinRequest::new(
        ImageSource::from_bgra(vec![0x00_4488aa; 32 * 32], 32, 32).unwrap(),
        (16, 16),
    )
    .radius(4);
    let handle = manager.create_pin(request).unwrap();
    assert_eq!(handle.id(), 1);
    assert_eq!(next_event(&manager), PinEvent::Created { id: 1 });

    let first = owned_pin();
    assert!(!first.is_null());
    assert_ne!(
        unsafe { GetWindowLongPtrW(first, GWL_EXSTYLE) } & WS_EX_NOACTIVATE as isize,
        0
    );

    let second_handle = manager
        .create_pin(
            PinRequest::new(
                ImageSource::from_bgra_bytes(&vec![0x88; 32 * 32 * 4], 32, 32).unwrap(),
                (64, 64),
            )
            .radius(4),
        )
        .unwrap();
    assert_eq!(second_handle.id(), 2);
    assert_eq!(next_event(&manager), PinEvent::Created { id: 2 });
    assert!(ImageSource::from_bgra_bytes(&[0; 3], 1, 1).is_err());
    assert!(manager.try_recv_event().is_none());

    manager.close_pin(handle).unwrap();
    assert_eq!(next_event(&manager), PinEvent::Closed { id: 1 });
    assert_eq!(unsafe { IsWindow(first) }, 0);
    manager.close_pin(handle).unwrap(); // already closed is harmless

    let clone = manager.clone();
    drop(manager);
    let second = owned_pin();
    assert!(
        !second.is_null(),
        "a clone must retain the host and its remaining pin"
    );
    drop(clone); // synchronous shutdown joins the window-owning thread
    assert_eq!(unsafe { IsWindow(second) }, 0);

    // A new host can register the same window class after the previous host exits.
    // F6 may already belong to another application, which is intentionally non-fatal.
    let restarted = NativeImages::start().unwrap();
    drop(restarted);
}
