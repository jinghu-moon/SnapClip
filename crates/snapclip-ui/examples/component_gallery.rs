#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(not(windows))]
fn main() {
    eprintln!("component_gallery is Windows-only");
}

#[cfg(windows)]
mod windows_gallery {
    use snapclip_model::geometry::{Point, Rect};
    use snapclip_ui::panel::d2d::{PanelPainter, SegmentColors};
    use snapclip_ui::panel::{
        Button, ButtonState, Checkbox, CheckboxState, Dropdown, DropdownKey, DropdownTarget,
        PointerEvent, RadioGroup, RadioGroupState, RadioOption, Segment, SegmentState, Slider,
        SliderKey, Tag, TagState, Toast, Toggle, ToggleState, Tooltip,
    };
    use snapclip_ui::panel::{
        ButtonColors, CheckboxColors, DropdownColors, FeedbackColors, RadioColors, SliderColors,
        TagColors, ToggleColors,
    };
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D_SIZE_U, D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
    };
    use windows::Win32::Graphics::Direct2D::{
        D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
        D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1CreateFactory, ID2D1Factory,
        ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    };
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
        DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWriteCreateFactory,
        IDWriteFactory, IDWriteTextFormat,
    };
    use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        ReleaseCapture, SetCapture, SetFocus,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{Result, w};

    const TIMER_ID: usize = 1;
    const TOAST_BUTTON: Rect = Rect::new(40, 635, 180, 671);

    struct GalleryState {
        d2d_factory: ID2D1Factory,
        dwrite: IDWriteFactory,
        text_format: IDWriteTextFormat,
        target: Option<ID2D1HwndRenderTarget>,
        dropdown: Dropdown,
        slider: Slider,
        tooltip: Tooltip,
        toast: Toast,
        button: Button,
        button_state: ButtonState,
        toast_button: Button,
        toast_button_state: ButtonState,
        checkbox: Checkbox,
        checkbox_state: CheckboxState,
        radio: RadioGroup,
        radio_state: RadioGroupState,
        toggle: Toggle,
        toggle_state: ToggleState,
        tag: Tag,
        tag_state: TagState,
        segmented: SegmentState,
        last_tick: u64,
    }

    impl GalleryState {
        fn new() -> Result<Self> {
            let d2d_factory = unsafe {
                D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?
            };
            let dwrite =
                unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)? };
            let text_format = unsafe {
                dwrite.CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    14.0,
                    w!(""),
                )?
            };
            unsafe {
                text_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                text_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            }
            let button = Rect::new(40, 118, 270, 158);
            let items = vec![
                Rect::new(40, 164, 270, 198),
                Rect::new(40, 200, 270, 234),
                Rect::new(40, 236, 270, 270),
            ];
            Ok(Self {
                d2d_factory,
                dwrite,
                text_format,
                target: None,
                dropdown: Dropdown::new(button, Rect::new(40, 160, 270, 278), items, Some(0)),
                slider: Slider::new(Rect::new(40, 335, 270, 365), 0.0, 100.0, 42.0, 1.0),
                tooltip: Tooltip::new(
                    Rect::new(430, 140, 590, 180),
                    "Button: press to fire",
                ),
                toast: Toast::new(2200),
                button: Button::new(Rect::new(430, 140, 590, 180)),
                button_state: ButtonState::default(),
                toast_button: Button::new(TOAST_BUTTON),
                toast_button_state: ButtonState::default(),
                checkbox: Checkbox::new(Rect::new(430, 220, 450, 240)),
                checkbox_state: CheckboxState::default(),
                radio: RadioGroup {
                    options: vec![
                        RadioOption {
                            rect: Rect::new(430, 275, 450, 295),
                            disabled: false,
                        },
                        RadioOption {
                            rect: Rect::new(430, 310, 450, 330),
                            disabled: false,
                        },
                    ],
                },
                radio_state: RadioGroupState::default(),
                toggle: Toggle::new(Rect::new(430, 360, 478, 384)),
                toggle_state: ToggleState::default(),
                tag: Tag::new(Rect::new(430, 410, 535, 442)),
                tag_state: TagState::default(),
                segmented: SegmentState {
                    track: Rect::new(430, 470, 650, 508),
                    segments: vec![
                        Segment {
                            rect: Rect::new(434, 474, 504, 504),
                            selected: true,
                            disabled: false,
                        },
                        Segment {
                            rect: Rect::new(508, 474, 578, 504),
                            selected: false,
                            disabled: false,
                        },
                        Segment {
                            rect: Rect::new(582, 474, 646, 504),
                            selected: false,
                            disabled: false,
                        },
                    ],
                    hovered: None,
                    pressed: None,
                    focused: false,
                    slide: None,
                },
                last_tick: 0,
            })
        }

        fn ensure_target(&mut self, hwnd: HWND) -> Result<&ID2D1HwndRenderTarget> {
            let mut client = RECT::default();
            unsafe {
                GetClientRect(hwnd, &mut client)?;
            }
            let width = (client.right - client.left).max(1) as u32;
            let height = (client.bottom - client.top).max(1) as u32;
            if self.target.is_none() {
                self.target = Some(unsafe {
                    self.d2d_factory.CreateHwndRenderTarget(
                        &D2D1_RENDER_TARGET_PROPERTIES {
                            pixelFormat: D2D1_PIXEL_FORMAT {
                                format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
                                alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
                            },
                            ..Default::default()
                        },
                        &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                            hwnd,
                            pixelSize: D2D_SIZE_U { width, height },
                            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                        },
                    )?
                });
            } else {
                unsafe {
                    self.target
                        .as_ref()
                        .expect("target initialized")
                        .Resize(&D2D_SIZE_U { width, height })?;
                }
            }
            Ok(self.target.as_ref().expect("target initialized"))
        }

        fn paint(&mut self, hwnd: HWND) -> Result<()> {
            self.ensure_target(hwnd)?;
            let target = self.target.as_ref().expect("target initialized");
            let bg = brush(target, color(20, 24, 31, 1.0))?;
            unsafe {
                target.BeginDraw();
                target.Clear(Some(&color(20, 24, 31, 1.0)));
            }
            text(
                target,
                &self.dwrite,
                &self.text_format,
                "SnapClip component gallery",
                Rect::new(40, 28, 500, 68),
                &brush(target, color(244, 247, 251, 1.0))?,
            )?;
            text(
                target,
                &self.dwrite,
                &self.text_format,
                "Native Win32 + Direct2D test surface",
                Rect::new(40, 70, 500, 96),
                &brush(target, color(156, 169, 186, 1.0))?,
            )?;

            let painter = PanelPainter {
                context: target,
                text_factory: &self.dwrite,
            };
            let fill = brush(target, color(45, 53, 66, 1.0))?;
            let line = brush(target, color(70, 84, 104, 1.0))?;
            let accent = brush(target, color(49, 130, 206, 1.0))?;
            let accent_soft = brush(target, color(80, 149, 222, 1.0))?;
            let hover = brush(target, color(57, 76, 101, 1.0))?;
            let pressed = brush(target, color(67, 117, 177, 1.0))?;
            let text_brush = brush(target, color(244, 247, 251, 1.0))?;
            let muted = brush(target, color(156, 169, 186, 1.0))?;
            let menu = brush(target, color(33, 39, 49, 1.0))?;
            let disabled = brush(target, color(80, 88, 102, 1.0))?;

            label(
                target,
                &self.dwrite,
                &self.text_format,
                "Dropdown",
                Rect::new(40, 96, 260, 116),
            )?;
            self.dropdown.paint(
                &painter,
                &["PNG", "JPG", "WEBP"],
                &self.text_format,
                self.dropdown.selected,
                self.dropdown.hovered.map(DropdownTarget::Item),
                self.dropdown.focused,
                DropdownColors {
                    button: &fill,
                    menu: &menu,
                    selected: &accent_soft,
                    line: &line,
                    text: &text_brush,
                    muted: &muted,
                    focus: &accent,
                },
            );
            label(
                target,
                &self.dwrite,
                &self.text_format,
                "Slider",
                Rect::new(40, 300, 260, 320),
            )?;
            self.slider.paint(
                &painter,
                SliderColors {
                    track: &line,
                    fill: &accent,
                    thumb: &accent_soft,
                    hover: &hover,
                    focus: &accent,
                    disabled: &disabled,
                },
            );
            text(
                target,
                &self.dwrite,
                &self.text_format,
                &format!("{}", self.slider.value.round() as i32),
                Rect::new(292, 330, 350, 370),
                &text_brush,
            )?;

            label(
                target,
                &self.dwrite,
                &self.text_format,
                "Button",
                Rect::new(430, 116, 650, 138),
            )?;
            self.button.paint(
                &painter,
                self.button_state,
                ButtonColors {
                    fill: &fill,
                    hover: &hover,
                    pressed: &pressed,
                    focus: &accent,
                    disabled: &disabled,
                },
                6,
            );
            text(
                target,
                &self.dwrite,
                &self.text_format,
                "Button",
                Rect::new(440, 140, 580, 180),
                &text_brush,
            )?;
            self.checkbox.paint(
                &painter,
                self.checkbox_state,
                CheckboxColors {
                    surface: &fill,
                    border: &line,
                    selected: &accent,
                    hover: &hover,
                    focus: &accent,
                    mark: &text_brush,
                    disabled: &disabled,
                },
                1.0,
                4,
            );
            text(
                target,
                &self.dwrite,
                &self.text_format,
                "Checkbox",
                Rect::new(462, 220, 590, 240),
                &text_brush,
            )?;
            self.radio.paint(
                &painter,
                &self.radio_state,
                RadioColors {
                    ring: &muted,
                    selected: &accent,
                    focus: &accent,
                    disabled: &disabled,
                },
            );
            for (index, option) in self.radio.options.iter().enumerate() {
                text(
                    target,
                    &self.dwrite,
                    &self.text_format,
                    if index == 0 { "Option A" } else { "Option B" },
                    Rect::new(
                        option.rect.right + 10,
                        option.rect.top,
                        560,
                        option.rect.bottom,
                    ),
                    &text_brush,
                )?;
            }
            self.toggle.paint(
                &painter,
                self.toggle_state,
                ToggleColors {
                    off_track: &fill,
                    off_hover: &hover,
                    on_track: &accent,
                    off_line: &line,
                    on_line: &accent_soft,
                    thumb: &text_brush,
                    focus: &accent,
                    disabled: &disabled,
                },
                2,
                if self.toggle_state.checked { 1.0 } else { 0.0 },
            );
            text(
                target,
                &self.dwrite,
                &self.text_format,
                "Switch",
                Rect::new(490, 360, 600, 384),
                &text_brush,
            )?;
            self.tag.paint(
                &painter,
                self.tag_state,
                TagColors {
                    off_fill: &fill,
                    off_hover: &hover,
                    on_fill: &accent,
                    line: &line,
                    on_line: &accent_soft,
                    focus: &accent,
                    disabled: &disabled,
                },
            );
            text(
                target,
                &self.dwrite,
                &self.text_format,
                "Selectable Tag",
                Rect::new(440, 410, 525, 442),
                &text_brush,
            )?;
            painter.segmented(
                &self.segmented,
                &["One", "Two", "Three"],
                &self.text_format,
                SegmentColors {
                    track: &menu,
                    selected: &accent,
                    hover: &hover,
                    focus: &accent,
                    text: &text_brush,
                    muted: &muted,
                },
                1.0,
            );
            self.toast_button.paint(
                &painter,
                self.toast_button_state,
                ButtonColors {
                    fill: &accent,
                    hover: &hover,
                    pressed: &pressed,
                    focus: &accent_soft,
                    disabled: &disabled,
                },
                6,
            );
            text(
                target,
                &self.dwrite,
                &self.text_format,
                "Show Toast",
                Rect::new(52, 635, 168, 671),
                &brush(target, color(244, 247, 251, 1.0))?,
            )?;
            self.tooltip.paint(
                &painter,
                &self.text_format,
                300,
                FeedbackColors {
                    fill: &menu,
                    line: &line,
                    text: &text_brush,
                },
            );
            self.toast.paint(
                &painter,
                &self.text_format,
                Rect::new(430, 620, 760, 674),
                FeedbackColors {
                    fill: &accent,
                    line: &accent_soft,
                    text: &text_brush,
                },
            );
            let _ = bg;
            let result = unsafe { target.EndDraw(None, None) };
            if let Err(error) = result {
                eprintln!("component_gallery: EndDraw failed: {error:?}");
                self.target = None;
                return Err(error);
            }
            Ok(())
        }

        fn tick(&mut self, hwnd: HWND) {
            self.last_tick = self.last_tick.saturating_add(16);
            self.tooltip.tick(16);
            self.toast.tick(16);
            unsafe {
                InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }
    fn brush(target: &ID2D1HwndRenderTarget, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
        unsafe { target.CreateSolidColorBrush(&color, None) }
    }
    fn text(
        target: &ID2D1HwndRenderTarget,
        _factory: &IDWriteFactory,
        format: &IDWriteTextFormat,
        value: &str,
        rect: Rect,
        brush: &ID2D1SolidColorBrush,
    ) -> Result<()> {
        let wide: Vec<u16> = value.encode_utf16().collect();
        unsafe {
            target.DrawText(
                &wide,
                format,
                &windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: rect.left as f32,
                    top: rect.top as f32,
                    right: rect.right as f32,
                    bottom: rect.bottom as f32,
                },
                brush,
                windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
    fn label(
        target: &ID2D1HwndRenderTarget,
        factory: &IDWriteFactory,
        format: &IDWriteTextFormat,
        value: &str,
        rect: Rect,
    ) -> Result<()> {
        text(
            target,
            factory,
            format,
            value,
            rect,
            &brush(target, color(156, 169, 186, 1.0))?,
        )
    }

    fn point_from_lparam(lparam: LPARAM) -> Point {
        Point::new(
            (lparam.0 as u16 as i16) as i32,
            ((lparam.0 >> 16) as u16 as i16) as i32,
        )
    }
    fn state(hwnd: HWND) -> Option<&'static mut GalleryState> {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GalleryState };
        (!ptr.is_null()).then(|| unsafe { &mut *ptr })
    }

    extern "system" fn wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_NCCREATE => {
                let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
                }
                LRESULT(1)
            }
            WM_PAINT => {
                if let Some(s) = state(hwnd) {
                    let mut ps = PAINTSTRUCT::default();
                    unsafe {
                        BeginPaint(hwnd, &mut ps);
                    }
                    if let Err(error) = s.paint(hwnd) {
                        eprintln!("component_gallery: paint failed: {error:?}");
                        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                    }
                    unsafe {
                        EndPaint(hwnd, &ps);
                    }
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER => {
                if let Some(s) = state(hwnd) {
                    s.tick(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some(s) = state(hwnd) {
                    let p = point_from_lparam(lparam);
                    s.slider.pointer_move(p);
                    s.dropdown.pointer_move(p);
                    s.button_state.hovered = s.button.hit_test(p);
                    s.toast_button_state.hovered = s.toast_button.hit_test(p);
                    s.checkbox_state.interaction.hovered = s.checkbox.hit_test(p);
                    s.radio_state.hovered = s.radio.target_at(p);
                    s.toggle_state.interaction.hovered = s.toggle.hit_test(p);
                    s.tag_state.interaction.hovered = s.tag.hit_test(p);
                    s.segmented.hovered = s.segmented.target_at(p);
                    s.tooltip.set_hovered(s.button_state.hovered);
                    unsafe {
                        InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Some(s) = state(hwnd) {
                    let p = point_from_lparam(lparam);
                    let captured = s.slider.pointer_down(p) | s.dropdown.pointer_down(p);
                    s.button
                        .pointer_event(&mut s.button_state, PointerEvent::Down, p);
                    s.toast_button
                        .pointer_event(&mut s.toast_button_state, PointerEvent::Down, p);
                    s.checkbox
                        .pointer_event(&mut s.checkbox_state, PointerEvent::Down, p);
                    s.radio
                        .pointer_event(&mut s.radio_state, PointerEvent::Down, p);
                    s.toggle
                        .pointer_event(&mut s.toggle_state, PointerEvent::Down, p);
                    s.tag.pointer_event(&mut s.tag_state, PointerEvent::Down, p);
                    s.segmented.pointer_event(PointerEvent::Down, p);
                    if captured
                        || s.button_state.pressed
                        || s.toast_button_state.pressed
                        || s.checkbox_state.interaction.pressed
                        || s.radio_state.pressed.is_some()
                        || s.toggle_state.interaction.pressed
                        || s.tag_state.interaction.pressed
                        || s.segmented.pressed.is_some()
                    {
                        unsafe {
                            SetCapture(hwnd);
                        }
                    }
                    unsafe {
                        InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(s) = state(hwnd) {
                    let p = point_from_lparam(lparam);
                    s.slider.pointer_up(p);
                    s.dropdown.pointer_up(p);
                    s.button
                        .pointer_event(&mut s.button_state, PointerEvent::Up, p);
                    if s.toast_button
                        .pointer_event(&mut s.toast_button_state, PointerEvent::Up, p)
                    {
                        s.toast.show("Toast is visible for 2.2 seconds");
                    }
                    s.checkbox
                        .pointer_event(&mut s.checkbox_state, PointerEvent::Up, p);
                    s.radio
                        .pointer_event(&mut s.radio_state, PointerEvent::Up, p);
                    s.toggle
                        .pointer_event(&mut s.toggle_state, PointerEvent::Up, p);
                    s.tag.pointer_event(&mut s.tag_state, PointerEvent::Up, p);
                    s.segmented.pointer_event(PointerEvent::Up, p);
                    unsafe {
                        ReleaseCapture();
                        InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if let Some(s) = state(hwnd) {
                    let key = wparam.0 as u32;
                    if s.dropdown.open {
                        let action = match key {
                            0x26 => Some(DropdownKey::Up),
                            0x28 => Some(DropdownKey::Down),
                            0x0d => Some(DropdownKey::Enter),
                            0x1b => Some(DropdownKey::Escape),
                            _ => None,
                        };
                        if let Some(action) = action {
                            s.dropdown.key(action);
                        }
                    } else {
                        let action = match key {
                            0x25 => Some(SliderKey::Left),
                            0x27 => Some(SliderKey::Right),
                            0x24 => Some(SliderKey::Home),
                            0x23 => Some(SliderKey::End),
                            _ => None,
                        };
                        if let Some(action) = action {
                            s.slider.key(action);
                        }
                    }
                    unsafe {
                        InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let ptr = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut GalleryState };
                if !ptr.is_null() {
                    unsafe {
                        drop(Box::from_raw(ptr));
                        PostQuitMessage(0);
                    }
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    pub fn run() -> Result<()> {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let class = w!("SnapClipComponentGallery");
        unsafe {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            });
        }
        let state = Box::new(GalleryState::new()?);
        let state_ptr = Box::into_raw(state);
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                class,
                w!("SnapClip Components"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                820,
                760,
                None,
                None,
                Some(instance),
                Some(state_ptr as *const _),
            )?
        };
        unsafe {
            SetTimer(Some(hwnd), TIMER_ID, 16, None);
            SetFocus(Some(hwnd));
        }
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.into() {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    windows_gallery::run()
}
