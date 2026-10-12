//! The magnifier pass: the info panel, its text formats and the loupe itself.
//!
//! Methods live in their own `impl OverlayRenderer` block; sibling modules of `d2d` can see
//! the private fields (privacy is module-scoped, and these are descendants).

use super::*;

impl OverlayRenderer {
    pub(super) fn draw_magnifier(
        &mut self,
        view: &RenderView,
        resources: &ChromeResources,
        metrics: RenderMetrics,
    ) -> Result<(), String> {
        let config = MagnifierConfig::with_zoom(view.magnifier_zoom).scaled(metrics.dpi);
        let geometry =
            crate::geometry::magnifier_geometry(view.cursor, config, view.frame, view.work_area);
        // A missing frame bitmap means there is nothing to magnify yet.
        let Some(bitmap) = resources.frame_bitmap.as_ref() else {
            return Ok(());
        };
        let band = &resources.magnifier_band;
        let crosshair = &resources.crosshair;
        let grid = &resources.magnifier_grid;
        let dark = &resources.magnifier_info;
        let white = &resources.label_text;
        let panel = geometry.panel;
        let zoom = geometry.zoom; // f32: physical pixels per source pixel
        // Convert a source-pixel delta to a panel-pixel delta.
        let z2p = |src_delta: i32| (src_delta as f32 * zoom).round() as i32;

        // Rounded-corner clip: PushLayer with a geometric mask clips all content
        // to a rounded rectangle, giving the panel the soft-cornered look.
        let corner_radius = 8.0 * (metrics.dpi.max(96) as f32 / 96.0);
        let factory = unsafe { self.d2d.GetFactory() }
            .map_err(|e| crate::windows::win::hresult("ID2D1DeviceContext::GetFactory", &e))?;
        let rounded_geo = unsafe {
            factory.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                rect: to_d2d(geometry.bounds),
                radiusX: corner_radius,
                radiusY: corner_radius,
            })
        }
        .map_err(|e| crate::windows::win::hresult("CreateRoundedRectangleGeometry", &e))?;
        let geo_mask: ID2D1Geometry = rounded_geo
            .cast()
            .map_err(|e| crate::windows::win::hresult("cast to ID2D1Geometry", &e))?;
        let layer = unsafe { self.d2d.CreateLayer(None) }
            .map_err(|e| crate::windows::win::hresult("CreateLayer", &e))?;

        unsafe {
            // Push the geometry-masked layer so all content is clipped to the
            // rounded-rect bounds.
            let lp = D2D1_LAYER_PARAMETERS1 {
                contentBounds: to_d2d(geometry.bounds),
                geometricMask: std::mem::ManuallyDrop::new(Some(geo_mask)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: windows_numerics::Matrix3x2::identity(),
                opacity: 1.0,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: Default::default(),
            };
            self.d2d.PushLayer(&lp, &layer);

            // Clip so the magnified pixels cannot spill out of the panel.
            self.d2d
                .PushAxisAlignedClip(&to_d2d(geometry.panel), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            // `geometry.source` is always exactly cursor-centred and may reach
            // past the frame at a screen edge; fill the whole panel first so the
            // out-of-frame margin shows a clean background instead of stale
            // pixels, then blit only the part of the source that actually exists.
            self.d2d.FillRectangle(&to_d2d(geometry.panel), dark);
            let visible_source = geometry.source.intersect(view.frame);
            if !visible_source.is_empty() {
                let visible_panel = Rect::new(
                    panel.left + z2p(visible_source.left - geometry.source.left),
                    panel.top + z2p(visible_source.top - geometry.source.top),
                    panel.left + z2p(visible_source.right - geometry.source.left),
                    panel.top + z2p(visible_source.bottom - geometry.source.top),
                );
                let interp = if zoom < 1.0 {
                    D2D1_INTERPOLATION_MODE_LINEAR
                } else {
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR
                };
                self.d2d.DrawBitmap(
                    bitmap,
                    Some(&to_d2d(visible_panel)),
                    1.0,
                    interp,
                    Some(&to_d2d(visible_source)),
                    None,
                );
            }

            // The sampled source pixel is `geometry.center` by construction;
            // since the source rect is built cursor-centred it always lands on
            // the panel's own centre cell, never translated aside at an edge.
            let half_w = (geometry.source.width() / 2).max(0);
            let half_h = (geometry.source.height() / 2).max(0);
            let center_x = panel.left + z2p(half_w);
            let center_y = panel.top + z2p(half_h);
            let cell_size = z2p(1).max(1);
            let center_cell =
                Rect::from_origin_size(Point::new(center_x, center_y), cell_size, cell_size);

            // Light-blue crosshair bands through the sampled pixel's whole row
            // and column: the eye follows them to the cell even before the ring.
            self.d2d.FillRectangle(
                &to_d2d(Rect::new(
                    panel.left,
                    center_y,
                    panel.right,
                    center_y + cell_size,
                )),
                band,
            );
            self.d2d.FillRectangle(
                &to_d2d(Rect::new(
                    center_x,
                    panel.top,
                    center_x + cell_size,
                    panel.bottom,
                )),
                band,
            );

            // Pixel grid: only at zoom >= 4 where each cell is wide enough to
            // render legibly. Integer-aligned to avoid sub-pixel blur.
            if geometry.zoom >= 4.0 {
                let step = geometry.zoom;
                // Vertical separators: one between each pair of columns.
                for index in 1..geometry.source.width() {
                    let x = panel.left as f32 + index as f32 * step + 0.5;
                    self.d2d.DrawLine(
                        vector2(x, panel.top as f32),
                        vector2(x, panel.bottom as f32),
                        grid,
                        1.0,
                        None,
                    );
                }
                // Horizontal separators: one between each pair of rows.
                for index in 1..geometry.source.height() {
                    let y = panel.top as f32 + index as f32 * step + 0.5;
                    self.d2d.DrawLine(
                        vector2(panel.left as f32, y),
                        vector2(panel.right as f32, y),
                        grid,
                        1.0,
                        None,
                    );
                }
            }

            // Mark the sampled cell with its true colour so it agrees with the
            // info-strip swatch. Both read the same async `magnifier_rgb`, so
            // filling (rather than relying on the blit's live-frame pixel) keeps
            // them identical even when sampling lags a frame. A white ring on the
            // cell edge plus a dark ring one pixel outside keep it readable on any
            // background.
            let cell_fill = self.require_brush(&self.info_swatch_fill_brush, "swatch fill")?;
            {
                let (r, g, b) = view.magnifier_rgb.unwrap_or((0, 0, 0));
                let _ = cell_fill.SetColor(&color(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    1.0,
                ));
            }
            self.d2d.FillRectangle(&to_d2d(center_cell), &cell_fill);
            self.d2d
                .DrawRectangle(&to_d2d(center_cell), white, 1.0, None);
            let outer_cell = Rect::new(
                center_cell.left - 1,
                center_cell.top - 1,
                center_cell.right + 1,
                center_cell.bottom + 1,
            );
            self.d2d.DrawRectangle(&to_d2d(outer_cell), dark, 1.0, None);

            // ── Zoom badge (top-right corner of the loupe panel) ──
            {
                let badge_scale = metrics.dpi.max(96) as f32 / 96.0;
                let badge_font_sz = 14.0 * badge_scale;
                let badge_pad_h = 8.0 * badge_scale;
                let badge_pad_v = 5.0 * badge_scale;
                let badge_inset = 6.0 * badge_scale;
                let badge_radius = 6.0 * badge_scale;
                let label = if zoom >= 10.0 {
                    format!("{:.0}×", zoom)
                } else if zoom >= 1.0 {
                    let r = zoom.round();
                    if (zoom - r).abs() < 0.01 {
                        format!("{:.0}×", r)
                    } else {
                        format!("{:.1}×", zoom)
                    }
                } else {
                    format!("{:.1}×", zoom)
                };
                let badge_fmt =
                    self.info_text_format_mut(false, 14.0, DWRITE_FONT_WEIGHT_BOLD, true)?;
                let text_w = self.measure_text_width_in(&label, &badge_fmt)?;
                let bw = text_w + badge_pad_h * 2.0;
                let bh = badge_font_sz + badge_pad_v * 2.0;
                let badge_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: panel.right as f32 - badge_inset - bw,
                        top: panel.top as f32 + badge_inset,
                        right: panel.right as f32 - badge_inset,
                        bottom: panel.top as f32 + badge_inset + bh,
                    },
                    radiusX: badge_radius,
                    radiusY: badge_radius,
                };
                let badge_bg = self.require_brush(&self.info_badge_bg_brush, "zoom badge bg")?;
                self.d2d.FillRoundedRectangle(&badge_rect, &badge_bg);
                let wide = label.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide,
                    &badge_fmt,
                    &badge_rect.rect,
                    white,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            self.d2d.PopAxisAlignedClip();

            // ── Info strip: the session's blurred frame cropped to the strip, a tint, two rows ──
            // Row 1: [swatch] colour value  ............  (x, y)
            // Row 2: [S] 格式   [C] 复制   [P] 坐标   [Z] 缩放   (keys as bound, spread end to end)
            let info = geometry.info_panel;
            let primary_brush = self.require_brush(&self.info_hex_brush, "info primary colour")?;
            let coord_brush = self.require_brush(&self.info_coord_value_brush, "info coord")?;
            let hint_brush = self.require_brush(&self.info_hint_brush, "info hint")?;
            let kbd_fill = self.require_brush(&self.info_kbd_fill_brush, "info kbd fill")?;
            let swatch_border =
                self.require_brush(&self.info_swatch_border_brush, "swatch border")?;
            let swatch_fill = self.require_brush(&self.info_swatch_fill_brush, "swatch fill")?;
            let tint = self.require_brush(&self.info_tint_brush, "info tint")?;
            let backdrop = info.intersect(view.frame);
            if let Some(blurred) = self.blurred_frame.as_ref()
                && !backdrop.is_empty()
            {
                let rect = to_d2d(backdrop);
                self.d2d.DrawBitmap(
                    blurred,
                    Some(&rect),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    Some(&rect),
                    None,
                );
            }
            self.d2d.FillRectangle(&to_d2d(info), &tint);

            // The strip is 76 DIP tall at every scale (`MagnifierConfig::scaled`), so its contents
            // are DIP × k too. The loupe's width is fixed in physical pixels, which is why both rows
            // shrink to fit, and why row 1 cuts the colour value when even its floor is too wide.
            let k = metrics.dpi.max(96) as f32 / 96.0;
            let swatch = INFO_SWATCH_SIZE_DIP * k;
            let swatch_radius = INFO_SWATCH_RADIUS_DIP * k;
            let content_l = info.left as f32 + INFO_PADDING_H_DIP * k;
            let content_r = info.right as f32 - INFO_PADDING_H_DIP * k;
            let avail = content_r - content_l;
            let row1_y = info.top as f32 + INFO_PADDING_V_DIP * k;

            // ---- Row 1: swatch, colour value, position ----
            let swatch_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: content_l,
                    top: row1_y,
                    right: content_l + swatch,
                    bottom: row1_y + swatch,
                },
                radiusX: swatch_radius,
                radiusY: swatch_radius,
            };
            {
                let (r, g, b) = view.magnifier_rgb.unwrap_or((0, 0, 0));
                let _ = swatch_fill.SetColor(&color(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    1.0,
                ));
            }
            self.d2d.FillRoundedRectangle(&swatch_rect, &swatch_fill);
            self.d2d
                .DrawRoundedRectangle(&swatch_rect, &swatch_border, 1.0, None);

            let (cx, cy) = if view.magnifier_relative && !view.selection.is_empty() {
                (
                    view.cursor.x - view.selection.left,
                    view.cursor.y - view.selection.top,
                )
            } else {
                (
                    view.cursor.x + view.screen_origin.x,
                    view.cursor.y + view.screen_origin.y,
                )
            };
            let pos_text = format!("({}, {})", cx, cy);
            let colour_text = view.magnifier_color_text.as_deref().unwrap_or("--");
            let fixed = swatch + (INFO_SWATCH_GAP_DIP + INFO_COORD_BLOCK_GAP_DIP) * k;
            let row1_font =
                shrink_to_fit(INFO_ROW1_FONT_MAX_DIP, INFO_ROW1_FONT_MIN_DIP, |size| {
                    let colour =
                        self.info_text_width(colour_text, size, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
                    let pos = self.info_text_width(&pos_text, size, DWRITE_FONT_WEIGHT_NORMAL)?;
                    Ok(fixed + colour + pos <= avail)
                })?;
            let pos_format =
                self.info_text_format_mut(true, row1_font, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            let pos_w = self.measure_text_width_in(&pos_text, &pos_format)?;
            let coord_left = content_r - pos_w;
            let colour_format =
                self.info_text_format_mut(true, row1_font, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
            let colour_x = content_l + swatch + INFO_SWATCH_GAP_DIP * k;
            let colour_right = (coord_left - INFO_COORD_BLOCK_GAP_DIP * k).max(colour_x);
            // Both texts span the swatch's height and are centred in it by the format's paragraph
            // alignment. The colour value is clipped to its band: at the floor size it is the one
            // that gives way, the position never does.
            let wide = colour_text.encode_utf16().collect::<Vec<u16>>();
            self.d2d.DrawText(
                &wide,
                &colour_format,
                &D2D_RECT_F {
                    left: colour_x,
                    top: row1_y,
                    right: colour_right,
                    bottom: row1_y + swatch,
                },
                &primary_brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            let wide = pos_text.encode_utf16().collect::<Vec<u16>>();
            self.d2d.DrawText(
                &wide,
                &pos_format,
                &D2D_RECT_F {
                    left: coord_left,
                    top: row1_y,
                    right: content_r,
                    bottom: row1_y + swatch,
                },
                &coord_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            // ---- Row 2: the hint row ----
            let ky = row1_y + swatch + INFO_HINT_ROW_GAP_DIP * k;
            let kh = INFO_HINT_ROW_HEIGHT_DIP * k;
            let hints = view.loupe_hints.clone();
            let fit = fit_hint_row(&hints, avail, k, |text, size, key| {
                let weight = if key {
                    DWRITE_FONT_WEIGHT_SEMI_BOLD
                } else {
                    DWRITE_FONT_WEIGHT_NORMAL
                };
                self.info_text_width(text, size, weight)
            })?;
            let key_format = self.info_text_format_mut(
                true,
                fit.font - 1.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                true,
            )?;
            let label_format =
                self.info_text_format_mut(false, fit.font, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            let inset = INFO_KBD_INSET_DIP * k;
            let radius = INFO_KBD_RADIUS_DIP * k;
            for (hint, slot) in hints.iter().zip(&fit.slots) {
                let x = content_l + slot.cap_x;
                let cap = D2D_RECT_F {
                    left: x,
                    top: ky + inset,
                    right: x + slot.cap_w,
                    bottom: ky + kh - inset,
                };
                self.d2d.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: cap,
                        radiusX: radius,
                        radiusY: radius,
                    },
                    &kbd_fill,
                );
                let wide = hint.key.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide,
                    &key_format,
                    &D2D_RECT_F {
                        top: ky,
                        bottom: ky + kh,
                        ..cap
                    },
                    &primary_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                let label_x = content_l + slot.label_x;
                let wide = hint.label.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide,
                    &label_format,
                    &D2D_RECT_F {
                        left: label_x,
                        top: ky,
                        right: label_x + slot.label_w + 2.0,
                        bottom: ky + kh,
                    },
                    &hint_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // Panel outline: a thin dark rounded frame around the magnifier.
            let outline_rr = D2D1_ROUNDED_RECT {
                rect: to_d2d(geometry.bounds),
                radiusX: corner_radius,
                radiusY: corner_radius,
            };
            self.d2d.DrawRoundedRectangle(&outline_rr, dark, 1.0, None);
            self.d2d.PopLayer();
            core::mem::drop(lp);

            // The pointer reticle is a *local* crosshair centred on the cursor.
            // The magnifier itself deliberately has no detached pixel marker: the
            // sampled pixel is emphasised by the cell ring above, which moves with
            // the panel and never paints outside it.
            let reticle = crate::geometry::crosshair_geometry(
                view.cursor,
                crate::geometry::crosshair_radius(metrics.dpi),
                view.frame,
            );
            self.d2d.DrawLine(
                vector2(
                    reticle.horizontal.left as f32 + 0.5,
                    view.cursor.y as f32 + 0.5,
                ),
                vector2(
                    reticle.horizontal.right as f32 - 0.5,
                    view.cursor.y as f32 + 0.5,
                ),
                crosshair,
                1.0,
                None,
            );
            self.d2d.DrawLine(
                vector2(
                    view.cursor.x as f32 + 0.5,
                    reticle.vertical.top as f32 + 0.5,
                ),
                vector2(
                    view.cursor.x as f32 + 0.5,
                    reticle.vertical.bottom as f32 - 0.5,
                ),
                crosshair,
                1.0,
                None,
            );
        }
        Ok(())
    }

    /// Advance width of `text` in the info strip's font at `size_dip` (physical pixels).
    pub(super) fn info_text_width(
        &mut self,
        text: &str,
        size_dip: f32,
        weight: DWRITE_FONT_WEIGHT,
    ) -> Result<f32, String> {
        let format = self.info_text_format_mut(true, size_dip, weight, false)?;
        self.measure_text_width_in(text, &format)
    }

    /// Advance width of `text` rendered in a specific DirectWrite format.
    pub(super) fn measure_text_width_in(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
    ) -> Result<f32, String> {
        let wide = text.encode_utf16().collect::<Vec<u16>>();
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&wide, format, f32::INFINITY, f32::INFINITY)
        }
        .map_err(|e| crate::windows::win::hresult("IDWriteFactory::CreateTextLayout", &e))?;
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m) }
            .map_err(|e| crate::windows::win::hresult("IDWriteTextLayout::GetMetrics", &e))?;
        Ok(m.width)
    }

    /// Resolve (and cache) a DirectWrite format for the info panel.
    ///
    /// `mono`: use Consolas/Lucida Console; otherwise Segoe UI Variable Display/Segoe UI.
    /// `weight`: a `DWRITE_FONT_WEIGHT` constant (NORMAL/SEMI_BOLD/BOLD).
    pub(super) fn info_text_format_mut(
        &mut self,
        mono: bool,
        size_dip: f32,
        weight: DWRITE_FONT_WEIGHT,
        center: bool,
    ) -> Result<IDWriteTextFormat, String> {
        let scale = self.metrics.dpi.max(96) as f32 / 96.0;
        let font_size = size_dip * scale;
        let family_slot = if mono { 1u8 } else { 0u8 };
        let size_key = (font_size * 100.0).round() as u32;
        let weight_val = weight.0 as u32;
        let align_val = if center { 1u32 } else { 0u32 };
        let key = (family_slot, size_key, weight_val, align_val);
        if let Some((_, f)) = self.info_formats.iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
        let locale = to_wide("en-us");
        // The embedded HarmonyOS subset is the primary family for every info-panel
        // string; the slot's original fonts survive only as fallbacks in case the
        // private registration was refused (e.g. a locked-down font host).
        let fallbacks: &[&str] = if mono {
            &[MONO_FONT_FAMILY, MONO_FONT_FALLBACK]
        } else {
            &[LABEL_FONT_FAMILY, LABEL_FONT_FALLBACK]
        };
        let families = std::iter::once(INFO_FONT_FAMILY).chain(fallbacks.iter().copied());
        let dw_weight = weight;
        let mut format = None;
        let mut last_err = String::new();
        for family in families {
            let wide = to_wide(family);
            match unsafe {
                self.dwrite.CreateTextFormat(
                    PCWSTR(wide.as_ptr()),
                    None,
                    dw_weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font_size,
                    PCWSTR(locale.as_ptr()),
                )
            } {
                Ok(created) => {
                    unsafe {
                        if center {
                            let _ = created.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                        }
                        let _ = created.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                        // Info strings are always single-line; never wrap them.
                        let _ = created.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                    }
                    format = Some(created);
                    break;
                }
                Err(e) => {
                    last_err = crate::windows::win::hresult(family, &e);
                    continue;
                }
            }
        }
        let format = format.ok_or_else(|| format!("no usable info font family ({last_err})"))?;
        self.info_formats.push((key, format.clone()));
        Ok(format)
    }
}

/// The largest size from `max` down to `min`, in [`INFO_FONT_STEP_DIP`] steps, that `fits`; `min`
/// when none does. The last size `fits` was asked about is the one returned.
pub(super) fn shrink_to_fit(
    max: f32,
    min: f32,
    mut fits: impl FnMut(f32) -> Result<bool, String>,
) -> Result<f32, String> {
    let mut size = max;
    while !fits(size)? && size - INFO_FONT_STEP_DIP >= min {
        size -= INFO_FONT_STEP_DIP;
    }
    Ok(size)
}

/// One item of the hint row, in physical pixels from the row's left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HintSlot {
    pub cap_x: f32,
    pub cap_w: f32,
    pub label_x: f32,
    pub label_w: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct HintRowFit {
    /// The label size in DIP; the key text is one smaller.
    pub font: f32,
    pub slots: Vec<HintSlot>,
}

/// Lay the hint row into `avail` physical pixels at scale `k`.
///
/// The label size steps down from 12 to 10 DIP until the items fit with at least 8 DIP between
/// them; whatever is left is spread evenly between the items, so the row runs end to end.
/// `measure(text, size_dip, is_key)` is the text's width in physical pixels; key text is measured
/// one size smaller than `size_dip` and semi-bold.
pub(super) fn fit_hint_row(
    hints: &[LoupeHint],
    avail: f32,
    k: f32,
    mut measure: impl FnMut(&str, f32, bool) -> Result<f32, String>,
) -> Result<HintRowFit, String> {
    let cap_min = (INFO_HINT_ROW_HEIGHT_DIP - 2.0 * INFO_KBD_INSET_DIP) * k;
    let gaps = hints.len().saturating_sub(1) as f32;
    let mut measured = Vec::with_capacity(hints.len());
    let font = shrink_to_fit(INFO_HINT_FONT_MAX_DIP, INFO_HINT_FONT_MIN_DIP, |size| {
        measured.clear();
        for hint in hints {
            let key = measure(&hint.key, size - 1.0, true)?;
            let label = measure(hint.label, size, false)?;
            measured.push(((key + INFO_KBD_TEXT_PAD_DIP * k).max(cap_min), label));
        }
        let sum: f32 = measured
            .iter()
            .map(|&(cap, label)| item_width(cap, label, k))
            .sum();
        Ok(sum + gaps * INFO_HINT_ITEM_GAP_MIN_DIP * k <= avail)
    })?;
    // `shrink_to_fit` returns the size it asked about last, so `measured` is for `font`.
    let sum: f32 = measured
        .iter()
        .map(|&(cap, label)| item_width(cap, label, k))
        .sum();
    let gap = if gaps > 0.0 {
        ((avail - sum) / gaps).max(0.0)
    } else {
        0.0
    };
    let mut x = 0.0;
    let slots = measured
        .iter()
        .map(|&(cap_w, label_w)| {
            let slot = HintSlot {
                cap_x: x,
                cap_w,
                label_x: x + cap_w + INFO_KBD_LABEL_GAP_DIP * k,
                label_w,
            };
            x += item_width(cap_w, label_w, k) + gap;
            slot
        })
        .collect();
    Ok(HintRowFit { font, slots })
}

fn item_width(cap: f32, label: f32, k: f32) -> f32 {
    cap + INFO_KBD_LABEL_GAP_DIP * k + label
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints() -> Vec<LoupeHint> {
        crate::keymap::Keymap::default().loupe_hints()
    }

    /// Every character is half the size wide, key text included.
    fn measure(text: &str, size: f32, _key: bool) -> Result<f32, String> {
        Ok(text.chars().count() as f32 * size * 0.5)
    }

    #[test]
    fn shrink_to_fit_takes_the_largest_size_that_fits() {
        let mut asked = Vec::new();
        let size = shrink_to_fit(12.0, 10.0, |size| {
            asked.push(size);
            Ok(size <= 11.0)
        })
        .unwrap();
        assert_eq!(size, 11.0);
        assert_eq!(asked, [12.0, 11.5, 11.0]);
    }

    #[test]
    fn shrink_to_fit_stops_at_the_floor_when_nothing_fits() {
        let size = shrink_to_fit(13.0, 11.0, |_| Ok(false)).unwrap();
        assert_eq!(size, 11.0);
    }

    #[test]
    fn a_roomy_row_keeps_the_largest_font_and_runs_end_to_end() {
        let fit = fit_hint_row(&hints(), 400.0, 1.0, measure).unwrap();
        assert_eq!(fit.font, INFO_HINT_FONT_MAX_DIP);
        assert_eq!(fit.slots.len(), 4);
        assert_eq!(fit.slots[0].cap_x, 0.0);
        let last = fit.slots.last().unwrap();
        assert!(
            (last.label_x + last.label_w - 400.0).abs() < 0.01,
            "{fit:?}"
        );
        // Space-between: every gap is the same.
        let gaps: Vec<f32> = fit
            .slots
            .windows(2)
            .map(|pair| pair[1].cap_x - (pair[0].label_x + pair[0].label_w))
            .collect();
        assert!(
            gaps.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 0.01),
            "{gaps:?}"
        );
    }

    #[test]
    fn a_tight_row_steps_the_font_down_until_the_minimum_gap_fits() {
        let roomy = fit_hint_row(&hints(), 1000.0, 1.0, measure).unwrap();
        let widths = |font: f32| {
            hints()
                .iter()
                .map(|hint| {
                    let cap = (hint.key.chars().count() as f32 * (font - 1.0) * 0.5
                        + INFO_KBD_TEXT_PAD_DIP)
                        .max(INFO_HINT_ROW_HEIGHT_DIP - 2.0 * INFO_KBD_INSET_DIP);
                    cap + INFO_KBD_LABEL_GAP_DIP + hint.label.chars().count() as f32 * font * 0.5
                })
                .sum::<f32>()
                + 3.0 * INFO_HINT_ITEM_GAP_MIN_DIP
        };
        // Just too narrow for 12, wide enough for 11.5.
        let avail = widths(11.5);
        assert!(avail < widths(12.0));
        let fit = fit_hint_row(&hints(), avail, 1.0, measure).unwrap();
        assert_eq!(roomy.font, 12.0);
        assert_eq!(fit.font, 11.5);
        for pair in fit.slots.windows(2) {
            let gap = pair[1].cap_x - (pair[0].label_x + pair[0].label_w);
            assert!(gap >= INFO_HINT_ITEM_GAP_MIN_DIP - 0.01, "{fit:?}");
        }
    }

    #[test]
    fn caps_are_never_narrower_than_they_are_tall_and_scale_with_dpi() {
        let fit = fit_hint_row(&hints(), 600.0, 1.5, measure).unwrap();
        let min = (INFO_HINT_ROW_HEIGHT_DIP - 2.0 * INFO_KBD_INSET_DIP) * 1.5;
        for slot in &fit.slots {
            assert!(slot.cap_w >= min, "{slot:?}");
            assert!(
                (slot.label_x - slot.cap_x - slot.cap_w - INFO_KBD_LABEL_GAP_DIP * 1.5).abs()
                    < 0.01
            );
        }
    }
}
