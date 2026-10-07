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
        let geometry = crate::geometry::magnifier_geometry(
            view.cursor,
            config,
            view.frame,
            view.work_area,
        );
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
            let center_cell = Rect::from_origin_size(Point::new(center_x, center_y), cell_size, cell_size);

            // Light-blue crosshair bands through the sampled pixel's whole row
            // and column: the eye follows them to the cell even before the ring.
            self.d2d.FillRectangle(
                &to_d2d(Rect::new(panel.left, center_y, panel.right, center_y + cell_size)),
                band,
            );
            self.d2d.FillRectangle(
                &to_d2d(Rect::new(center_x, panel.top, center_x + cell_size, panel.bottom)),
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
            self.d2d.DrawRectangle(&to_d2d(center_cell), white, 1.0, None);
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
                    if (zoom - r).abs() < 0.01 { format!("{:.0}×", r) } else { format!("{:.1}×", zoom) }
                } else {
                    format!("{:.1}×", zoom)
                };
                let badge_fmt = self.info_text_format_mut(false, 14.0, DWRITE_FONT_WEIGHT_BOLD, true)?;
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
                    &wide, &badge_fmt, &badge_rect.rect,
                    white, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            self.d2d.PopAxisAlignedClip();

            // ── Info panel: two-column glass layout ──
            // Left: colour swatch + S-cycled value (row 1) and X/Y coordinates
            // (row 2). Right: three stacked `<kbd>` hint rows for S / C / P.
            let info = geometry.info_panel;
            let bg = self.require_brush(&self.info_bg_brush, "info bg")?;
            let primary_brush = self.require_brush(&self.info_hex_brush, "info primary colour")?;
            let coord_val_brush = self.require_brush(&self.info_coord_value_brush, "info coord")?;
            let hint_brush = self.require_brush(&self.info_hint_brush, "info hint")?;
            let kbd_text_brush = self.require_brush(&self.info_kbd_text_brush, "info kbd text")?;
            let kbd_fill_brush = self.require_brush(&self.info_kbd_fill_brush, "info kbd fill")?;
            let kbd_bottom_brush = self.require_brush(&self.info_kbd_bottom_brush, "info kbd bot")?;
            let swatch_border = self.require_brush(&self.info_swatch_border_brush, "swatch border")?;
            let swatch_inset = self.require_brush(&self.info_swatch_inset_brush, "swatch inset")?;
            let swatch_fill = self.require_brush(&self.info_swatch_fill_brush, "swatch fill")?;

            // Info-strip backdrop: real Gaussian blur restricted to `info`, with
            // a translucent tint on top. If either the effect or the tint brush
            // is missing (or there is no frame to sample), fall back to the old
            // solid `bg` so the strip never disappears.
            let blur_owned = self.blur_effect.clone();
            let tint_owned = self.info_tint_brush.clone();
            if let (Some(fb), Some(blur), Some(tint)) = (
                resources.frame_bitmap.as_ref(),
                blur_owned.as_ref(),
                tint_owned.as_ref(),
            ) {
                // Rebind the effect input every frame: the effect instance is
                // cached across sessions, but `frame_bitmap` is rebuilt per
                // session, so its input slot must be refreshed against the
                // currently frozen frame. SetInput returns (), not Result.
                let fb_img: ID2D1Image = fb
                    .cast()
                    .map_err(|error| crate::windows::win::hresult("ID2D1Bitmap1::cast", &error))?;
                blur.SetInput(0, &fb_img, true);
                // Mode A (context DPI = 96, coordinates in physical pixels):
                // std_dev is expressed in DIP (= physical pixels here); the
                // `* scale` widens the kernel proportionally on high-DPI
                // displays so the perceived blur strength stays constant.
                let std_dev = 12.0 * (metrics.dpi.max(96) as f32 / 96.0);
                blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_UNKNOWN,
                    &std_dev.to_ne_bytes(),
                )
                .map_err(|error| {
                    crate::windows::win::hresult("ID2D1Effect::SetValue(STANDARD_DEVIATION)", &error)
                })?;
                // DrawImage is bounded to `info` via the source rectangle, so
                // D2D only evaluates the blur over the region actually needed
                // (info rect + kernel padding). Both optional pointers must be
                // raw pointers in windows-rs 0.61 (`Option<*const _>`).
                let dest = vector2(info.left as f32, info.top as f32);
                let src = to_d2d(info);
                let blur_img: ID2D1Image = blur
                    .cast()
                    .map_err(|error| crate::windows::win::hresult("ID2D1Effect::cast", &error))?;
                self.d2d
                    .DrawImage(
                        &blur_img,
                        Some(&dest as *const Vector2),
                        Some(&src as *const D2D_RECT_F),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                // 0.48 tint on top keeps text legible without hiding the texture.
                self.d2d.FillRectangle(&to_d2d(info), tint);
            } else {
                self.d2d.FillRectangle(&to_d2d(info), &bg);
            }

            // The loupe is a single fixed **physical**-pixel box (340×180 image +
            // info strip, DPI-invariant — see MagnifierConfig::PANEL_WIDTH_PHYSICAL)
            // clipped to one rounded rect, so the info strip shares that fixed
            // width. Its fonts and spacing therefore stay in physical pixels:
            // scaling them by DPI while the box stays put would overflow it on
            // high-DPI displays. On the single colour/coordinate line the panel is
            // wide enough that both fit with room to spare (asserted by
            // `widest_colour_value_fits_the_info_row`).
            let scale = 1.0f32;
            let pad_v = INFO_PADDING_V_DIP * scale;
            let pad_h = INFO_PADDING_H_DIP * scale;
            let swatch_sz = INFO_SWATCH_SIZE_DIP * scale;
            let swatch_radius = INFO_SWATCH_RADIUS_DIP * scale;
            let swatch_gap = INFO_SWATCH_GAP_DIP * scale;
            let coord_block_gap = INFO_COORD_BLOCK_GAP_DIP * scale;
            let kbd_pad = INFO_KBD_PADDING_DIP * scale;
            let kbd_radius = INFO_KBD_RADIUS_DIP * scale;
            let kbd_text_gap = INFO_KBD_TEXT_GAP_DIP * scale;
            let kbd_line_h = INFO_KBD_LINE_HEIGHT_DIP * scale;
            let hint_item_gap = INFO_HINT_ITEM_GAP_DIP * scale;
            let kbd_row_gap = INFO_KBD_ROW_GAP_DIP * scale;
            let kbd_block_gap = INFO_KBD_BLOCK_GAP_DIP * scale;

            // ── Row 1 (colour value + right-aligned coordinates) + 2×2 grid ──
            // Row 1: [swatch] colour value  ............  (x, y)   (coords right-aligned)
            // Row 2: [S 色值格式]  [C 复制色值]
            // Row 3: [P 坐标模式]  [Z 滚轮缩放]
            let content_l = info.left as f32 + pad_h;
            let content_r = info.right as f32 - pad_h;
            let row1_h = swatch_sz; // colour/coordinate line height (swatch-sized)
            let content_h = (info.bottom - info.top) as f32 - pad_v * 2.0;
            let block_h = row1_h + kbd_block_gap + kbd_line_h * 2.0 + kbd_row_gap;
            let row1_y = info.top as f32 + pad_v + (content_h - block_h).max(0.0) / 2.0;
            let kbd_row_a_y = row1_y + row1_h + kbd_block_gap;
            let kbd_row_b_y = kbd_row_a_y + kbd_line_h + kbd_row_gap;

            // ---- Row 1: swatch + colour value ----
            let swatch_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: content_l,
                    top: row1_y + (row1_h - swatch_sz) / 2.0,
                    right: content_l + swatch_sz,
                    bottom: row1_y + (row1_h + swatch_sz) / 2.0,
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
            self.d2d.DrawRoundedRectangle(&swatch_rect, &swatch_border, 1.0, None);
            let inset_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: swatch_rect.rect.left + 1.0,
                    top: swatch_rect.rect.top + 1.0,
                    right: swatch_rect.rect.right - 1.0,
                    bottom: swatch_rect.rect.bottom - 1.0,
                },
                radiusX: (swatch_radius - 1.0).max(0.0),
                radiusY: (swatch_radius - 1.0).max(0.0),
            };
            self.d2d.DrawRoundedRectangle(&inset_rect, &swatch_inset, 1.0, None);

            // Position: a single "(x, y)" string, right-aligned on row 1, so the
            // colour value keeps the remaining (left) width.
            let (cx, cy) = if view.magnifier_relative && !view.selection.is_empty() {
                (view.cursor.x - view.selection.left, view.cursor.y - view.selection.top)
            } else {
                (view.cursor.x + view.screen_origin.x, view.cursor.y + view.screen_origin.y)
            };
            let pos_text = format!("({}, {})", cx, cy);
            let value_format = self.info_text_format_mut(true, INFO_COORD_VALUE_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            let pos_w = self.measure_text_width_in(&pos_text, &value_format)?;
            let coord_left = content_r - pos_w;

            // Colour value fills the band between the swatch and the position.
            // The 340 px panel leaves ample room, so the widest value fits without
            // ever reaching the position text (asserted by the info-row width test).
            let colour_text = view.magnifier_color_text.as_deref().unwrap_or("--");
            let primary_format = self.info_text_format_mut(true, INFO_PRIMARY_COLOR_FONT_DIP, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
            let colour_x = content_l + swatch_sz + swatch_gap;
            let colour_right = (coord_left - coord_block_gap).max(colour_x);
            {
                let wide = colour_text.encode_utf16().collect::<Vec<u16>>();
                // Draw across the full row-1 band and let the format's paragraph
                // centring put the text on the swatch's exact vertical centre; the
                // earlier manual (row1_h - font)/2 offset compounded with paragraph
                // centring pushed the glyphs ~3px below the swatch.
                self.d2d.DrawText(
                    &wide, &primary_format,
                    &D2D_RECT_F {
                        left: colour_x, top: row1_y,
                        right: colour_right, bottom: row1_y + row1_h,
                    },
                    &primary_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // Right-aligned position text, sharing row 1's vertical centre.
            {
                let wide_pos = pos_text.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide_pos, &value_format,
                    &D2D_RECT_F {
                        left: coord_left, top: row1_y,
                        right: content_r, bottom: row1_y + row1_h,
                    },
                    &coord_val_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // ---- Row 2: four <kbd> hint items, laid out horizontally & centred ----
            // `hint_format` measures the CJK description (left-flushed); `hint_centered`
            // centres the single key letter inside its box. Both share the
            // vertical-centre paragraph alignment baked into `info_text_format_mut`.
            let hint_format = self.info_text_format_mut(false, INFO_HINT_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            let hint_centered = self.info_text_format_mut(false, INFO_HINT_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL, true)?;
            let hints = INFO_HINTS;
            // Measure every item (box + gap + description); the two columns
            // share the widest item width so the grid lines up, and the whole
            // 2×2 block is centred inside the strip.
            let mut box_ws = [0.0f32; 4];
            let mut desc_ws = [0.0f32; 4];
            let mut item_ws = [0.0f32; 4];
            for i in 0..4 {
                let key_w = self.measure_text_width_in(hints[i].0, &hint_format)?;
                box_ws[i] = (key_w + kbd_pad * 2.0).max(kbd_line_h);
                desc_ws[i] = self.measure_text_width_in(hints[i].1, &hint_format)?;
                item_ws[i] = box_ws[i] + kbd_text_gap + desc_ws[i];
            }
            let col_w = item_ws.iter().cloned().fold(0.0f32, f32::max);
            let grid_w = col_w * 2.0 + hint_item_gap;
            let col0_x = (content_l + content_r - grid_w) / 2.0;
            let col1_x = col0_x + col_w + hint_item_gap;
            let slots = [
                (col0_x, kbd_row_a_y),
                (col1_x, kbd_row_a_y),
                (col0_x, kbd_row_b_y),
                (col1_x, kbd_row_b_y),
            ];
            for i in 0..4 {
                let (key, desc) = hints[i];
                let (item_x, row_y) = slots[i];
                let box_w = box_ws[i];
                let desc_w = desc_ws[i];
                let box_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: item_x, top: row_y,
                        right: item_x + box_w, bottom: row_y + kbd_line_h,
                    },
                    radiusX: kbd_radius, radiusY: kbd_radius,
                };
                self.d2d.FillRoundedRectangle(&box_rect, &kbd_fill_brush);
                let bot_rect = D2D_RECT_F {
                    left: item_x + 1.0, top: row_y + kbd_line_h - 1.0,
                    right: item_x + box_w - 1.0, bottom: row_y + kbd_line_h,
                };
                self.d2d.FillRectangle(&bot_rect, &kbd_bottom_brush);
                {
                    let wide = key.encode_utf16().collect::<Vec<u16>>();
                    self.d2d.DrawText(
                        &wide, &hint_centered,
                        &D2D_RECT_F {
                            left: item_x, top: row_y,
                            right: item_x + box_w, bottom: row_y + kbd_line_h,
                        },
                        &kbd_text_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
                let desc_x = item_x + box_w + kbd_text_gap;
                let wide_desc = desc.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide_desc, &hint_format,
                    &D2D_RECT_F {
                        left: desc_x, top: row_y,
                        right: desc_x + desc_w + 2.0, bottom: row_y + kbd_line_h,
                    },
                    &hint_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
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
                vector2(reticle.horizontal.left as f32 + 0.5, view.cursor.y as f32 + 0.5),
                vector2(reticle.horizontal.right as f32 - 0.5, view.cursor.y as f32 + 0.5),
                crosshair,
                1.0,
                None,
            );
            self.d2d.DrawLine(
                vector2(view.cursor.x as f32 + 0.5, reticle.vertical.top as f32 + 0.5),
                vector2(view.cursor.x as f32 + 0.5, reticle.vertical.bottom as f32 - 0.5),
                crosshair,
                1.0,
                None,
            );
        }
        Ok(())
    }

    /// Advance width of `text` rendered in a specific DirectWrite format.
    pub(super) fn measure_text_width_in(&self, text: &str, format: &IDWriteTextFormat) -> Result<f32, String> {
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
