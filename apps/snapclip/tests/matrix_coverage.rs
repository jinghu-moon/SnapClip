//! `P6.02` — every row of the test matrix names a test that exists.
//!
//! ## Why this is a table
//!
//! `docs/30 §30.1`–`§30.6` 是一份**需求**（78 行场景）；`docs/31 §13` 把它翻译成**排期**
//! （每行一个主责任务）。翻译只有被核对才是真的：一行写着一个任务、那个任务却从没写过
//! 对应测试，排期就退回成愿望清单。这个文件把 `docs/31 §13` 的 78 行读出来，逐行要求
//! [`MATRIX`] 给出至少一个**真实存在**的测试。
//!
//! ## 这个文件检查什么
//!
//! 1. 文档与表格**逐行对齐**：区域、编号、分类（A/B/C/D）、层级（L1–L4/`CI`）四列一字不差。
//!    文档多一行少一行都红（`§13.1`–`§13.7` 的 7 个区、78 行）。
//! 2. 每一行至少点名一个测试，且那个测试在它说的文件里真的定义 `fn <name>(`。
//! 3. 分类合计与 `D : B ≥ 50%` 与文档 `§13.8` 的声称一致——这是用户 §31
//!    "失败场景必须和成功场景同等重要"那条要求被**数字执行**的地方。
//! 4. 层级合计同样与 `§13.8` 一致（`§13.8` 的那一行是**逐行统计的结论**，不是愿景）。
//! 5. **L1/L2 的测试不得带 `#[ignore]`**——一层/二层的测试不需要真实桌面，被忽略
//!    就等于不存在。L3/L4 允许 `#[ignore]`，但 [`DESKTOP`] 里列出的真实桌面用例
//!    必须**仍然**被 `#[ignore]`（P6 退出条件 ①）。
//!
//! 测试名写成 `"<仓库相对路径>::<函数名>"`。路径是显式的，因为一个函数名不构成
//! 定位：`a_resize_ends_the_stream_...` 这样的名字在三个文件里都可能出现。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One row of `docs/31 §13`: where it is, and what carries it.
struct Row {
    /// `"13.1"` … `"13.7"`.
    area: &'static str,
    /// The `#` column.
    number: u32,
    /// `'A' | 'B' | 'C' | 'D'`.
    class: char,
    /// `"L1" | "L2" | "L3" | "L4" | "L1+L2" | "CI"`.
    level: &'static str,
    /// `"<repo-relative path>::<test fn>"`, at least one.
    tests: &'static [&'static str],
}

/// The 78 rows of `docs/31 §13.1`–`§13.7`, each mapped to the test that carries it.
///
/// RED (`P6.02`): empty. GREEN fills it in, one row per `§13` row, in the same order.
///
/// 一行的载体是 `"<路径>::<测试名>"`；以 `.ps1` 结尾的条目是**门禁脚本**，它的退出码就是断言
/// （`§13.7` 的两行本来就是 CI 行）。
const MATRIX: &[Row] = &[
    // ── §13.1 Capture ──────────────────────────────────────────────────────────────
    Row { area: "13.1", number: 1, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/win/wgc.rs::a_window_capture_item_can_be_created_for_a_top_level_window", "crates/snapclip-capture/src/windows/scroll_probe.rs::capture_probe"] },
    Row { area: "13.1", number: 2, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_probe.rs::capture_probe", "crates/snapclip-capture/src/windows/scroll_probe.rs::inject_matrix_probe"] },
    Row { area: "13.1", number: 3, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/windows/win/wgc.rs::the_pool_is_not_recreated_between_frames"] },
    Row { area: "13.1", number: 4, class: 'D', level: "L2", tests: &["crates/snapclip-capture/src/windows/win/wgc.rs::an_unavailable_capture_option_is_recorded_and_asserted_not_swallowed", "crates/snapclip-capture/src/windows/win/wgc.rs::an_unavailable_cursor_option_does_not_fail_the_capture"] },
    Row { area: "13.1", number: 5, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::the_window_backend_is_preferred_and_the_monitor_backend_is_the_fallback", "crates/snapclip-capture/src/windows/win/wgc.rs::a_window_capture_item_can_be_created_for_a_top_level_window"] },
    Row { area: "13.1", number: 6, class: 'D', level: "L2", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::closed_minimised_and_resized_are_three_distinct_endings"] },
    Row { area: "13.1", number: 7, class: 'D', level: "L2", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::a_target_resize_stops_the_session", "crates/snapclip-capture/src/windows/scroll_source.rs::closed_minimised_and_resized_are_three_distinct_endings"] },
    Row { area: "13.1", number: 8, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::the_target_moving_to_another_monitor_with_the_same_dpi_keeps_running"] },
    Row { area: "13.1", number: 9, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::a_topology_change_on_another_monitor_leaves_the_session_running"] },
    Row { area: "13.1", number: 10, class: 'D', level: "L2", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::a_dpi_change_on_the_targets_monitor_stops_the_session"] },
    Row { area: "13.1", number: 11, class: 'D', level: "L2", tests: &["crates/snapclip-capture/src/windows/providers.rs::a_device_lost_hresult_is_classified_as_device_removal", "crates/snapclip-capture/src/windows/win/d3d11.rs::device_lost_codes_are_recognised_from_tagged_messages"] },
    Row { area: "13.1", number: 12, class: 'B', level: "L1+L2", tests: &["crates/snapclip-capture/src/windows/providers.rs::one_hundred_delivered_frames_cost_one_hundred_region_reads", "crates/snapclip-capture/src/windows/providers.rs::a_scroll_source_reads_a_region_instead_of_the_whole_frame"] },
    // ── §13.2 Scroll ───────────────────────────────────────────────────────────────
    Row { area: "13.2", number: 1, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_actuator.rs::send_input_places_the_cursor_before_it_fires", "crates/snapclip-capture/src/windows/scroll_probe.rs::inject_probe"] },
    Row { area: "13.2", number: 2, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_actuator.rs::the_lparam_uses_screen_coordinates", "crates/snapclip-capture/src/windows/scroll_probe.rs::inject_probe"] },
    Row { area: "13.2", number: 3, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_actuator.rs::post_message_sinks_to_the_deepest_child_window"] },
    Row { area: "13.2", number: 4, class: 'D', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_probe.rs::uipi_probe"] },
    Row { area: "13.2", number: 5, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/windows/scroll_actuator.rs::the_four_combinations_match_the_table", "crates/snapclip-capture/src/windows/scroll_actuator.rs::a_non_foreground_target_never_uses_send_input"] },
    Row { area: "13.2", number: 6, class: 'D', level: "L2", tests: &["crates/snapclip-capture/src/scroll/loop_control.rs::three_posted_steps_without_a_confirmed_step_switch_the_path"] },
    Row { area: "13.2", number: 7, class: 'D', level: "L2", tests: &["crates/snapclip-capture/src/scroll/loop_control.rs::both_paths_failing_three_times_ends_the_session"] },
    Row { area: "13.2", number: 8, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/scroll/loop_control.rs::smooth_scrolling_is_waited_out_instead_of_being_estimated", "crates/snapclip-capture/src/scroll/loop_control.rs::the_loop_waits_until_two_consecutive_frames_agree_before_estimating"] },
    Row { area: "13.2", number: 9, class: 'B', level: "L4", tests: &["crates/snapclip-capture/src/scroll/loop_control.rs::cancel_latency_has_a_measured_max", "crates/snapclip-capture/src/scroll/session.rs::a_cancel_publishes_its_instant_with_its_bit"] },
    Row { area: "13.2", number: 10, class: 'B', level: "L4", tests: &["crates/snapclip-capture/src/scroll/session.rs::stop_commits_the_export_and_cancel_discards_it"] },
    // ── §13.3 Matching and offset ──────────────────────────────────────────────────
    Row { area: "13.3", number: 1, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::every_integer_shift_from_one_to_forty_comes_back_exactly", "crates/snapclip-capture/src/scroll/acceptance.rs::the_full_funnel_never_confirms_a_wrong_shift"] },
    Row { area: "13.3", number: 2, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/acceptance.rs::the_full_funnel_never_confirms_a_wrong_shift", "crates/snapclip-capture/src/scroll/acceptance.rs::every_scan_case_is_a_real_step"] },
    Row { area: "13.3", number: 3, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::the_gate_accepts_exactly_viewport_extent_and_rejects_one_more"] },
    Row { area: "13.3", number: 4, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::the_gate_accepts_exactly_viewport_extent_and_rejects_one_more", "crates/snapclip-capture/src/scroll/displacement.rs::the_verifiability_floor_is_a_different_number_from_the_hard_gate"] },
    Row { area: "13.3", number: 5, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::uniformly_flat_input_scores_zero_instead_of_nan", "crates/snapclip-capture/src/scroll/displacement.rs::a_candidate_that_cannot_be_measured_and_a_blank_page_are_both_not_scene_cuts"] },
    Row { area: "13.3", number: 6, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_periodic_line_carrier_does_not_decide_the_step", "crates/snapclip-capture/src/scroll/displacement.rs::the_prior_reorders_a_periodic_page_without_confirming_it"] },
    Row { area: "13.3", number: 7, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::one_d_candidates_include_the_true_shift_on_a_periodic_image", "crates/snapclip-capture/src/scroll/displacement.rs::the_prior_is_soft_and_cannot_reject_on_its_own"] },
    Row { area: "13.3", number: 8, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::equal_scoring_modes_report_uncertain", "crates/snapclip-capture/src/scroll/displacement.rs::no_rival_and_no_scale_are_both_not_an_ambiguity"] },
    Row { area: "13.3", number: 9, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_low_texture_page_has_no_independent_supporters", "crates/snapclip-capture/src/scroll/displacement.rs::a_gradient_page_ties_on_correlation_and_leaves_ambiguity_to_gate_four"] },
    Row { area: "13.3", number: 10, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::a_duplicate_frame_is_answered_without_touching_the_estimator"] },
    Row { area: "13.3", number: 11, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::the_gate_never_returns_half_the_dimension"] },
    Row { area: "13.3", number: 12, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::closing_any_gate_changes_the_error_rate", "crates/snapclip-capture/src/scroll/acceptance.rs::the_full_funnel_never_confirms_a_wrong_shift"] },
    Row { area: "13.3", number: 13, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::closing_any_gate_changes_the_error_rate", "crates/snapclip-capture/src/scroll/acceptance.rs::the_scan_catches_a_funnel_that_lost_the_last_layer"] },
    Row { area: "13.3", number: 14, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_wrong_prior_converges_instead_of_locking", "crates/snapclip-capture/src/scroll/loop_control.rs::the_loop_converges_on_a_page_whose_gain_it_does_not_start_with"] },
    Row { area: "13.3", number: 15, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_single_rearranged_frame_stays_uncertain_and_the_session_continues"] },
    Row { area: "13.3", number: 16, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::three_consecutive_scene_cuts_decay_the_model_but_do_not_stop"] },
    // ── §13.4 Stitching ────────────────────────────────────────────────────────────
    Row { area: "13.4", number: 1, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::assert_invariants_fires_on_each_of_the_eight_violations"] },
    Row { area: "13.4", number: 2, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::only_the_new_rows_are_written"] },
    Row { area: "13.4", number: 3, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::one_hundred_confirmed_steps_leave_zero_drift"] },
    Row { area: "13.4", number: 4, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::band_height_never_drops_below_a_quarter_of_the_extent"] },
    Row { area: "13.4", number: 5, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::scrolling_up_produces_a_prepend_not_a_duplicate"] },
    Row { area: "13.4", number: 6, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::a_small_rollback_that_is_fully_covered_is_contained"] },
    Row { area: "13.4", number: 7, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::undo_returns_to_the_previous_step_and_deletes_later_bands", "crates/snapclip-capture/src/scroll/canvas.rs::undo_does_not_touch_the_learned_estimate"] },
    Row { area: "13.4", number: 8, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::a_one_band_budget_still_produces_a_correct_canvas", "crates/snapclip-capture/src/scroll/canvas.rs::the_reference_band_and_the_last_two_confirmed_bands_are_never_evicted"] },
    Row { area: "13.4", number: 9, class: 'B', level: "L1+L2", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::the_architecture_limit_is_injectable_and_trims_to_a_valid_partial", "apps/snapclip/src/capture/row_band_png.rs::finish_with_abort_writes_a_complete_iend_and_the_file_decodes"] },
    Row { area: "13.4", number: 10, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/scroll/export.rs::writing_rows_out_of_order_is_rejected", "apps/snapclip/src/capture/row_band_png.rs::a_skipped_row_range_is_an_error"] },
    Row { area: "13.4", number: 11, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::the_horizontal_axis_scores_through_the_same_code", "crates/snapclip-capture/src/scroll/observation.rs::the_same_script_answers_the_same_on_both_axes"] },
    Row { area: "13.4", number: 12, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/observation.rs::the_axis_mapping_is_exhaustive"] },
    // ── §13.5 Browser ──────────────────────────────────────────────────────────────
    Row { area: "13.5", number: 1, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_probe.rs::inject_matrix_probe", "crates/snapclip-capture/src/scroll/acceptance.rs::the_full_funnel_never_confirms_a_wrong_shift"] },
    Row { area: "13.5", number: 2, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_fixed_header_becomes_a_low_weight_region_without_being_excluded"] },
    Row { area: "13.5", number: 3, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_region_that_changes_on_its_own_is_down_weighted_but_still_covers_its_rows"] },
    Row { area: "13.5", number: 4, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/scroll/acceptance.rs::a_moving_region_over_the_revealed_rows_is_written_verbatim"] },
    Row { area: "13.5", number: 5, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::content_that_changes_in_the_rows_being_revealed_does_not_move_the_estimate", "crates/snapclip-capture/src/scroll/acceptance.rs::a_moving_region_over_the_revealed_rows_is_written_verbatim"] },
    Row { area: "13.5", number: 6, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::the_architecture_limit_is_injectable_and_trims_to_a_valid_partial", "crates/snapclip-capture/src/scroll/canvas.rs::the_warn_length_is_a_pure_ui_parameter"] },
    Row { area: "13.5", number: 7, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::the_target_moving_to_another_monitor_with_the_same_dpi_keeps_running", "crates/snapclip-capture/src/windows/scroll_actuator.rs::a_non_foreground_target_never_uses_send_input"] },
    Row { area: "13.5", number: 8, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_frame_at_a_different_scale_is_not_silently_stitched"] },
    Row { area: "13.5", number: 9, class: 'C', level: "L3", tests: &["crates/snapclip-capture/src/scroll/displacement.rs::a_region_that_changes_on_its_own_is_down_weighted_but_still_covers_its_rows"] },
    // ── §13.6 UI ───────────────────────────────────────────────────────────────────
    Row { area: "13.6", number: 1, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/scroll/panel.rs::the_eight_questions_have_concrete_answers_after_ten_steps"] },
    Row { area: "13.6", number: 2, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/scroll/panel.rs::uncertain_steps_are_drawn_dashed_with_a_counter_not_in_red", "crates/snapclip-capture/src/scroll/displacement.rs::every_status_has_exactly_one_effect"] },
    Row { area: "13.6", number: 3, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/scroll/panel.rs::the_viewport_box_never_gets_thinner_than_four_dip"] },
    Row { area: "13.6", number: 4, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/preview.rs::a_hundred_thousand_pixel_canvas_never_materialises_a_whole_thumbnail", "crates/snapclip-capture/src/scroll/preview.rs::the_preview_scale_lands_the_thumbnail_at_its_target_width"] },
    Row { area: "13.6", number: 5, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/scroll/loop_control.rs::dragging_the_preview_leaves_follow_mode_and_returns_on_command"] },
    Row { area: "13.6", number: 6, class: 'B', level: "L2", tests: &["crates/snapclip-capture/src/scroll/loop_control.rs::a_slow_consumer_does_not_slow_the_capture_loop", "crates/snapclip-capture/src/scroll/preview.rs::a_publish_while_the_consumer_holds_the_lock_is_dropped_not_queued"] },
    Row { area: "13.6", number: 7, class: 'B', level: "L1", tests: &["crates/snapclip-capture/src/scroll/preview.rs::updates_are_capped_at_ten_hertz", "crates/snapclip-capture/src/scroll/loop_control.rs::a_rate_limited_preview_publishes_less_and_still_ends_correctly"] },
    Row { area: "13.6", number: 8, class: 'B', level: "L4", tests: &["crates/snapclip-capture/src/windows/win/d2d/tests.rs::the_overlay_thread_never_blocks_longer_than_eight_ms"] },
    Row { area: "13.6", number: 9, class: 'B', level: "L3", tests: &["crates/snapclip-capture/src/windows/win/d2d/tests.rs::the_preview_panel_does_not_cover_the_toolbar_hit_regions"] },
    // ── §13.7 Memory and regression ────────────────────────────────────────────────
    Row { area: "13.7", number: 1, class: 'D', level: "L4", tests: &["crates/snapclip-capture/src/scroll/mem_probe.rs::the_probe_reports_live_peak_and_allocated_separately", "crates/snapclip-capture/src/scroll/mem_probe.rs::the_three_lengths_run_in_separate_processes"] },
    Row { area: "13.7", number: 2, class: 'D', level: "L1", tests: &["crates/snapclip-capture/src/scroll/canvas.rs::a_cancelled_session_leaves_no_spill_files", "crates/snapclip-capture/src/scroll/canvas.rs::a_spill_file_is_removed_when_its_band_is_evicted_back_to_memory"] },
    Row { area: "13.7", number: 3, class: 'D', level: "L2", tests: &["apps/snapclip/tests/png_params_probe.rs::the_encoder_streams_rows_without_materializing_the_image"] },
    Row { area: "13.7", number: 4, class: 'D', level: "L2", tests: &["apps/snapclip/src/capture/row_band_png.rs::an_oversized_dimension_is_refused_before_the_header", "apps/snapclip/src/capture/artifact_writer.rs::an_oversized_dimension_is_rejected_before_the_first_byte"] },
    Row { area: "13.7", number: 5, class: 'A', level: "L3", tests: &["crates/snapclip-capture/src/session.rs::happy_path_walks_through_every_state", "apps/snapclip/tests/regression_baseline.rs::the_regression_suite_names_the_behaviours_it_freezes"] },
    Row { area: "13.7", number: 6, class: 'A', level: "L4", tests: &["apps/snapclip/tests/regression_baseline.rs::the_regression_suite_names_the_behaviours_it_freezes", "crates/snapclip-capture/src/window_detection/hit_test.rs::hit_test_and_nearest_target_stay_inside_the_latency_budget"] },
    Row { area: "13.7", number: 7, class: 'A', level: "L1", tests: &["crates/snapclip-capture/src/windows/providers.rs::wgc_is_tried_before_the_bitblt_fallback", "crates/snapclip-capture/src/windows/providers.rs::bitblt_is_the_only_provider_once_wgc_is_dropped"] },
    Row { area: "13.7", number: 8, class: 'A', level: "CI", tests: &["tools/check-dependency-direction.ps1"] },
    Row { area: "13.7", number: 9, class: 'D', level: "L1", tests: &["crates/snapclip-model/src/capture.rs::capture_state_reports_activity", "crates/snapclip-model/src/capture.rs::state_and_format_names_are_the_frozen_contract_strings"] },
    Row { area: "13.7", number: 10, class: 'A', level: "CI", tests: &["crates/snapclip-capture/src/windows/scroll_source.rs::no_real_desktop_test_silently_skips_in_this_module"] },
];

/// Real-desktop tests cited by [`MATRIX`]. Every one of them must stay `#[ignore]`d.
///
/// 这些用例需要一台**可交互的**桌面（真实窗口、真实合成器、真实输入），在 CI 或锁屏的
/// 会话里不会有意义的结果；P6 退出条件 ① 要求它们只能以 `#[ignore]` 的形态存在。
const DESKTOP: &[&str] = &[
    "crates/snapclip-capture/src/windows/win/wgc.rs::a_window_capture_item_can_be_created_for_a_top_level_window",
    "crates/snapclip-capture/src/windows/scroll_probe.rs::capture_probe",
    "crates/snapclip-capture/src/windows/scroll_probe.rs::inject_matrix_probe",
    "crates/snapclip-capture/src/windows/scroll_probe.rs::inject_probe",
    "crates/snapclip-capture/src/windows/scroll_probe.rs::uipi_probe",
    "crates/snapclip-capture/src/windows/win/d2d/tests.rs::the_overlay_thread_never_blocks_longer_than_eight_ms",
    "crates/snapclip-capture/src/windows/win/d2d/tests.rs::the_preview_panel_does_not_cover_the_toolbar_hit_regions",
];

/// `docs/31 §13` 的 7 个区与各自的期望行数（`§13.1`–`§13.7`）。
const AREAS: &[(&str, u32)] = &[
    ("13.1", 12),
    ("13.2", 10),
    ("13.3", 16),
    ("13.4", 12),
    ("13.5", 9),
    ("13.6", 9),
    ("13.7", 10),
];

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `apps/snapclip`; the repository root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("apps/snapclip has a grandparent")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn defines_fn(source: &str, name: &str) -> bool {
    source.contains(&format!("fn {name}("))
}

/// Whether `fn <name>(` carries an `#[ignore]` on one of the four lines above it.
fn is_ignored(source: &str, name: &str) -> bool {
    let needle = format!("fn {name}(");
    let lines: Vec<&str> = source.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(&needle) {
            continue;
        }
        let window = index.saturating_sub(4);
        return lines[window..index]
            .iter()
            .any(|above| above.trim_start().starts_with("#[ignore"));
    }
    false
}

/// One parsed row of `docs/31 §13`: `(area, number, class, level)`.
type ParsedRow = (String, u32, char, String);

/// Parse the `§13.1`–`§13.7` tables out of `docs/31`.
///
/// 场景列里**含有字面量 `|`**（例如 `|d| ∈ 1..40`），所以不能从头切；`#` 取第 1 段，
/// 分类与层级取倒数两段。
fn parse_matrix(document: &str) -> Vec<ParsedRow> {
    let mut rows = Vec::new();
    let mut area = String::new();
    for line in document.lines() {
        if let Some(heading) = line.strip_prefix("### ") {
            area = heading.split_whitespace().next().unwrap_or("").to_string();
            continue;
        }
        if !area.starts_with("13.") || !line.starts_with("| ") {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 6 {
            continue;
        }
        let Some(number) = parts[1].trim().parse::<u32>().ok() else {
            continue;
        };
        let class = parts[parts.len() - 3].trim();
        let level = parts[parts.len() - 2].trim();
        if class.len() != 1 || !"ABCD".contains(class) {
            continue;
        }
        rows.push((area.clone(), number, class.chars().next().unwrap(), level.to_string()));
    }
    rows
}

/// Parse the `层级` row of `docs/31 §13.8` into `level -> count`.
///
/// 只读**统计格**（第 2 格）。差异原因格里可以引用旧值（`DEV-112` 就引用了），如果把整行都扫
/// 一遍，那些"曾经的数字"会被当成当前的结论。
fn parse_claimed_levels(document: &str) -> BTreeMap<String, u32> {
    let mut claimed = BTreeMap::new();
    for line in document.lines() {
        if !line.starts_with("| 层级 |") {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 3 {
            continue;
        }
        for cell in parts[2].split('·') {
            let cell = cell.trim().trim_matches('|').trim();
            let Some((level, count)) = cell.split_once('=') else {
                continue;
            };
            if let Ok(count) = count.trim().parse::<u32>() {
                claimed.insert(level.trim().to_string(), count);
            }
        }
    }
    claimed
}

/// Parse the `分类` rows of `docs/31 §13.8` into `class -> count`.
fn parse_claimed_classes(document: &str) -> BTreeMap<char, u32> {
    let mut claimed = BTreeMap::new();
    for line in document.lines() {
        for (class, prefix) in [('B', "| **B（新功能）**"), ('C', "| **C（边界）**"), ('D', "| **D（失败）**"), ('A', "| **A（原有功能，矩阵内）**")] {
            if !line.starts_with(prefix) {
                continue;
            }
            let parts: Vec<&str> = line.split('|').collect();
            let count = parts[2].trim().trim_matches('*').trim();
            if let Ok(count) = count.parse::<u32>() {
                claimed.insert(class, count);
            }
        }
    }
    claimed
}

fn split_test(entry: &str) -> (&str, &str) {
    entry
        .rsplit_once("::")
        .unwrap_or_else(|| panic!("`{entry}` is not written as <path>::<test>"))
}

/// `P6.02` RED/GREEN: every row of `docs/31 §13` is carried by a test that exists.
#[test]
fn every_matrix_row_maps_to_at_least_one_test() {
    let document = read("docs/31-scroll-capture-tdd-tasklist.md");
    let parsed = parse_matrix(&document);

    assert!(
        !MATRIX.is_empty(),
        "the matrix table is empty: none of the {} rows in docs/31 §13 is mapped to a test, so \
         §13 is a wish list and `P6.03`/`P6.04` would be deleting into an unverified tree",
        parsed.len()
    );

    // 1. The document itself still has the shape the mapping was written against.
    let mut expected_rows = 0;
    for (area, count) in AREAS {
        let found = parsed.iter().filter(|row| &row.0 == area).count() as u32;
        assert_eq!(
            found, *count,
            "docs/31 §{area} has {found} rows, and the mapping was written against {count}"
        );
        expected_rows += count;
    }
    assert_eq!(
        parsed.len() as u32,
        expected_rows,
        "docs/31 §13 has {} rows outside §13.1–§13.7",
        parsed.len() as u32 - expected_rows
    );

    // 2. Row for row, the four machine-checkable columns agree.
    let mut mapped: BTreeMap<(String, u32), &Row> = BTreeMap::new();
    for row in MATRIX {
        let key = (row.area.to_string(), row.number);
        assert!(
            mapped.insert(key.clone(), row).is_none(),
            "docs/31 §{} row {} is mapped twice",
            key.0,
            key.1
        );
    }
    let mut missing = Vec::new();
    for (area, number, class, level) in &parsed {
        let Some(row) = mapped.get(&(area.clone(), *number)) else {
            missing.push(format!("§{area} row {number}"));
            continue;
        };
        assert_eq!(
            row.class, *class,
            "§{area} row {number} is class {class} in the document and {} in the mapping",
            row.class
        );
        assert_eq!(
            row.level, *level,
            "§{area} row {number} is level {level} in the document and {} in the mapping",
            row.level
        );
    }
    assert!(
        missing.is_empty(),
        "{} row(s) of docs/31 §13 have no test: {}",
        missing.len(),
        missing.join(", ")
    );
    for row in MATRIX {
        assert!(
            parsed
                .iter()
                .any(|(area, number, ..)| area == row.area && *number == row.number),
            "the mapping has §{} row {}, which the document does not",
            row.area,
            row.number
        );
    }

    // 3. Every row names a test, and every named test is really there.
    let mut sources: BTreeMap<&str, String> = BTreeMap::new();
    for row in MATRIX {
        assert!(
            !row.tests.is_empty(),
            "§{} row {} names no test",
            row.area,
            row.number
        );
        for entry in row.tests {
            if entry.ends_with(".ps1") {
                // A gate script carries its own evidence: its exit code is the assertion, and the
                // only thing this file can check is that the gate is still in the tree.
                assert!(
                    repo_root().join(entry).is_file(),
                    "§{} row {} names the gate `{entry}`, which is not in the tree",
                    row.area,
                    row.number
                );
                continue;
            }
            let (file, test) = split_test(entry);
            let source = sources
                .entry(file)
                .or_insert_with(|| read(file));
            assert!(
                defines_fn(source, test),
                "§{} row {} names `{test}`, and {file} does not define it",
                row.area,
                row.number
            );
            if row.level == "L1" || row.level == "L2" {
                assert!(
                    !is_ignored(source, test),
                    "§{} row {} is {} and names `{test}`, which is `#[ignore]`d: a level that \
                     needs no desktop must not be skipped",
                    row.area,
                    row.number,
                    row.level
                );
            }
        }
    }

    // 4. The class tallies and the ratio the document claims.
    let mut counts: BTreeMap<char, u32> = BTreeMap::new();
    for row in MATRIX {
        *counts.entry(row.class).or_default() += 1;
    }
    let claimed_classes = parse_claimed_classes(&document);
    for (class, claimed) in &claimed_classes {
        let found = counts.get(class).copied().unwrap_or(0);
        assert_eq!(
            found, *claimed,
            "docs/31 §13.8 claims {claimed} class-{class} rows and the mapping has {found}"
        );
    }
    let b = counts.get(&'B').copied().unwrap_or(0);
    let d = counts.get(&'D').copied().unwrap_or(0);
    assert!(b > 0, "the mapping has no B-class row");
    assert!(
        d * 100 >= b * 50,
        "the mapping has {d} D-class rows against {b} B-class rows: below the 50% the user's \
         §31 criterion demands"
    );

    // 5. The level tallies the document claims (its `层级` row is a conclusion, not a wish).
    let mut levels: BTreeMap<String, u32> = BTreeMap::new();
    for row in MATRIX {
        *levels.entry(row.level.to_string()).or_default() += 1;
    }
    let claimed_levels = parse_claimed_levels(&document);
    assert_eq!(
        levels, claimed_levels,
        "docs/31 §13.8's `层级` row disagrees with its own per-row table"
    );

    // 6. The real-desktop discipline.
    let mut desktop_seen = BTreeSet::new();
    for entry in DESKTOP {
        let (file, test) = split_test(entry);
        desktop_seen.insert(test);
        let source = sources.entry(file).or_insert_with(|| read(file));
        assert!(
            is_ignored(source, test),
            "`{test}` is a real-desktop probe and is not `#[ignore]`d (P6 exit condition 1)"
        );
    }
    println!(
        "[P6.02] {} rows mapped, {} test(s) checked, {} real-desktop probe(s) still ignored",
        MATRIX.len(),
        MATRIX.iter().map(|row| row.tests.len()).sum::<usize>(),
        desktop_seen.len()
    );
}
