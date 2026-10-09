//! `P6.01` — the A-class regression suite, frozen before anything is deleted.
//!
//! ## Why this is a table
//!
//! `AGENTS.md` 第 7/8 条把"相关已有功能正常 + 验证通过"写成验收标准。一段散文说
//! "普通截图还能用"不是判据：删掉一个测试、改掉一个名字、把 `#[ignore]` 挪到别的
//! 函数上，散文都还是真的。可判定的形式只有一种——**把每个行为绑到一个具体的测试
//! 名字上**，然后让一个元测试去核对那张表还在不在。
//!
//! 这正是 `P6.03`/`P6.04`（删除清单 `D-1…D-15` 与重构清单 `R-1…R-7`）之前必须先
//! 做完这一步的原因：删除项每一条都要先证明"它冻住的东西在别处仍然被冻住"。
//!
//! ## 这个文件检查什么
//!
//! 1. `A_CLASS` 覆盖了 [`BEHAVIOURS`] 这个**闭集**——多一个行为少一个行为都红，
//!    没有通配分支（同 `snapclip-model` 里 `CaptureState` 的穷举测试）。
//! 2. 每一行点名的文件存在、文件里真的有 `fn <test>(`。
//! 3. 被冻住的测试**本身不是** `#[ignore]`——一个不跑的测试冻不住任何东西。
//! 4. [`DESKTOP_ONLY`] 里那些真实桌面探针仍然存在、**并且仍然被 `#[ignore]`**
//!    （P6 退出条件 ①：不得出现非 `#[ignore]` 的真实桌面用例）。
//!
//! 数字断言（`docs/31 §0.3` 的 `477 / 10 / 0` 只增不减）在
//! `tools/check-test-baseline.ps1` 里，由 `.githooks/pre-push` 对刚跑完的日志执行：
//! 一个测试被删掉时，源码里的名字可能还在，只有跑出来的计数会变。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One frozen behaviour: a behaviour name, the file that freezes it, the test that freezes it.
struct Frozen {
    /// Must be one of [`BEHAVIOURS`].
    behaviour: &'static str,
    /// Repository-relative path, forward slashes.
    file: &'static str,
    /// The test function name, without parentheses.
    test: &'static str,
}

/// The closed set of A-class behaviours (`docs/31` `P6.01` 的 GREEN 清单，逐条对应
/// `docs/30 §30.7` 后半与既有测试）。
const BEHAVIOURS: &[&str] = &[
    "f5_full_flow",
    "region_capture",
    "window_capture",
    "browser_window",
    "hotkey",
    "cancel",
    "history_ui",
    "hit_test_budget",
    "capture_path_unchanged",
    "capture_state_coverage",
];

// GREEN (`P6.01`): every behaviour in [`BEHAVIOURS`] resolves to at least one runnable test.
// The rows are the ones the task lists — `docs/31` `P6.01` 的 GREEN 清单逐条落地。
const A_CLASS: &[Frozen] = &[
    // 普通截图全流程（F5）：arming -> selecting -> selected -> exporting -> idle
    Frozen {
        behaviour: "f5_full_flow",
        file: "crates/snapclip-capture/src/session.rs",
        test: "happy_path_walks_through_every_state",
    },
    Frozen {
        behaviour: "f5_full_flow",
        file: "crates/snapclip-capture/src/session.rs",
        test: "preparing_accepts_the_frame_and_arms_the_session",
    },
    Frozen {
        behaviour: "f5_full_flow",
        file: "crates/snapclip-capture/src/session.rs",
        test: "chrome_is_hidden_until_a_selection_exists",
    },
    // 区域截图：一个拖动定义矩形；在帧外/帧边缘的矩形按帧裁剪
    Frozen {
        behaviour: "region_capture",
        file: "crates/snapclip-capture/src/session.rs",
        test: "a_press_alone_never_creates_a_zero_size_selection",
    },
    Frozen {
        behaviour: "region_capture",
        file: "crates/snapclip-capture/src/session.rs",
        test: "dragging_inside_moves_the_existing_selection",
    },
    Frozen {
        behaviour: "region_capture",
        file: "crates/snapclip-capture/src/session.rs",
        test: "snap_to_adopts_a_usable_rectangle_and_relies_on_the_clip_for_edges",
    },
    Frozen {
        behaviour: "region_capture",
        file: "crates/snapclip-capture/src/windows/providers.rs",
        test: "read_region_only_transfers_the_selection",
    },
    Frozen {
        behaviour: "region_capture",
        file: "crates/snapclip-capture/src/windows/providers.rs",
        test: "read_region_clips_a_selection_hanging_off_the_right_and_bottom",
    },
    Frozen {
        behaviour: "region_capture",
        file: "crates/snapclip-capture/src/windows/providers.rs",
        test: "read_region_clips_a_negative_origin_to_the_frame",
    },
    // 窗口截图：窗口目标的选择与窗口级捕获能力（真实桌面那一条属 DESKTOP_ONLY）
    Frozen {
        behaviour: "window_capture",
        file: "crates/snapclip-capture/src/windows/win/wgc.rs",
        test: "support_probe_never_panics",
    },
    Frozen {
        behaviour: "window_capture",
        file: "crates/snapclip-capture/src/windows/providers.rs",
        test: "wgc_is_tried_before_the_bitblt_fallback",
    },
    Frozen {
        behaviour: "window_capture",
        file: "crates/snapclip-capture/src/window_detection/hit_test.rs",
        test: "hit_test_picks_the_frontmost_overlapping_window",
    },
    // 浏览器窗口：UIA 元素层（浏览器的一等公民路径）与它画的预览标签
    Frozen {
        behaviour: "browser_window",
        file: "crates/snapclip-capture/src/window_detection/uia.rs",
        test: "a_text_run_inside_an_element_is_not_a_target",
    },
    Frozen {
        behaviour: "browser_window",
        file: "crates/snapclip-capture/src/windows/overlay/tests.rs",
        test: "the_preview_label_names_the_element_it_snapped_to",
    },
    Frozen {
        behaviour: "browser_window",
        file: "crates/snapclip-capture/src/windows/win/d2d/tests.rs",
        test: "an_element_preview_keeps_its_own_pixels_and_the_window_fallback_does_not",
    },
    // 快捷键：F5 是本进程注册的全局热键，Esc/Enter 是会话内的键
    Frozen {
        behaviour: "hotkey",
        file: "crates/snapclip-capture/src/windows/hotkey.rs",
        test: "f5_is_registered_without_modifiers_and_with_repeat_suppressed",
    },
    Frozen {
        behaviour: "hotkey",
        file: "crates/snapclip-capture/src/windows/hotkey.rs",
        test: "escape_and_enter_are_not_global_hotkeys",
    },
    Frozen {
        behaviour: "hotkey",
        file: "crates/snapclip-capture/src/windows/hotkey.rs",
        test: "conflict_errors_are_distinguishable_from_other_failures",
    },
    // Cancel：Esc 从每个活跃状态回到 idle；窗口销毁与设备移除走同一条清理
    Frozen {
        behaviour: "cancel",
        file: "crates/snapclip-capture/src/session.rs",
        test: "esc_from_every_active_state_returns_to_idle",
    },
    Frozen {
        behaviour: "cancel",
        file: "crates/snapclip-capture/src/session.rs",
        test: "window_destroy_and_device_removal_use_the_same_cleanup",
    },
    // 既有 UI：历史窗口的六个屏幕级用例（gpui 测试宿主）
    Frozen {
        behaviour: "history_ui",
        file: "apps/snapclip/tests/ui.rs",
        test: "typing_filters_the_list_and_escape_clears_it",
    },
    Frozen {
        behaviour: "history_ui",
        file: "apps/snapclip/tests/ui.rs",
        test: "rows_do_not_paint_over_each_other",
    },
    Frozen {
        behaviour: "history_ui",
        file: "apps/snapclip/tests/ui.rs",
        test: "the_filter_the_next_page_and_the_delete_flow_all_reach_the_screen",
    },
    Frozen {
        behaviour: "history_ui",
        file: "apps/snapclip/tests/ui.rs",
        test: "the_arrows_belong_to_the_list_and_the_field_keeps_them_while_it_types",
    },
    Frozen {
        behaviour: "history_ui",
        file: "apps/snapclip/tests/ui.rs",
        test: "toggling_the_setting_writes_it_to_disk",
    },
    Frozen {
        behaviour: "history_ui",
        file: "apps/snapclip/tests/ui.rs",
        test: "a_clipboard_event_refreshes_the_history_screen",
    },
    // hit_test.rs:435 的 p95 断言（文档点名的那一条）
    Frozen {
        behaviour: "hit_test_budget",
        file: "crates/snapclip-capture/src/window_detection/hit_test.rs",
        test: "hit_test_and_nearest_target_stay_inside_the_latency_budget",
    },
    // 捕获路径不变：`P2.06` 只加了滚动自己的顺序，没有改 `attempt_order`
    Frozen {
        behaviour: "capture_path_unchanged",
        file: "crates/snapclip-capture/src/windows/providers.rs",
        test: "wgc_is_tried_before_the_bitblt_fallback",
    },
    // 无可达状态负债：`CaptureState` 的每个变体都有测试可达（`Adjusting` 的删除会先撞上这两条）
    Frozen {
        behaviour: "capture_state_coverage",
        file: "crates/snapclip-model/src/capture.rs",
        test: "capture_state_reports_activity",
    },
    Frozen {
        behaviour: "capture_state_coverage",
        file: "crates/snapclip-model/src/capture.rs",
        test: "state_and_format_names_are_the_frozen_contract_strings",
    },
];

/// The `#[ignore]`d real-desktop probes that existed before the scroll work — the `9 个既有
/// #[ignore]` of `docs/31` `P6.01`. They must be **kept**: neither deleted nor "fixed" into
/// tests that run without a desktop session.
const DESKTOP_ONLY: &[Frozen] = &[
    Frozen {
        behaviour: "desktop_probe",
        file: "crates/snapclip-capture/src/ring_contrast.rs",
        test: "ring_contrast_probe",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "crates/snapclip-capture/src/windows/uia_provider/tests/probes.rs",
        test: "browser_element_probe",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "crates/snapclip-capture/src/windows/uia_provider/tests/probes.rs",
        test: "dump_uia_names_under_the_cursor",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "crates/snapclip-capture/src/windows/uia_provider/tests/probes.rs",
        test: "explorer_rule_probe",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "crates/snapclip-capture/src/windows/win/d2d/tests.rs",
        test: "write_drawn_text_for_the_font_subset",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "crates/snapclip-capture/src/windows/win/d2d/tests.rs",
        test: "font_cost_probe",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "apps/snapclip/src/capture/mod.rs",
        test: "the_shell_can_host_the_capture_overlay",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "apps/snapclip/src/clipboard_ingest.rs",
        test: "the_pipeline_starts_and_stops",
    },
    Frozen {
        behaviour: "desktop_probe",
        file: "apps/snapclip/src/tray.rs",
        test: "the_icon_can_be_created_and_taken_away",
    },
];

fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // `apps/snapclip` -> the repository root
    manifest
        .parent()
        .and_then(Path::parent)
        .expect("the app crate lives two levels below the repository root")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "the frozen behaviour names `{relative}`, which cannot be read ({}): {error}",
            path.display()
        )
    })
}

/// `true` when `source` defines a function called `name`.
fn defines_fn(source: &str, name: &str) -> bool {
    source.contains(&format!("fn {name}("))
}

/// `true` when the `fn name(` declaration carries an `#[ignore]`, including the multi-line
/// form (`#[ignore = "…" \` + continued string) used in this repository.
fn is_ignored(source: &str, name: &str) -> bool {
    let needle = format!("fn {name}(");
    let mut previous: Vec<&str> = Vec::new();
    for line in source.lines() {
        if line.contains(&needle) {
            return previous.iter().any(|line| line.contains("#[ignore"));
        }
        previous.push(line);
        if previous.len() > 4 {
            previous.remove(0);
        }
    }
    false
}

#[test]
fn the_regression_suite_names_the_behaviours_it_freezes() {
    assert!(
        !A_CLASS.is_empty(),
        "the A-class list is empty: nothing is frozen, so `P6.03`/`P6.04` would be deleting \
         into an untested tree"
    );

    // 1. Closed set: no row names an unknown behaviour, no behaviour is left unfrozen.
    let declared: BTreeSet<&str> = BEHAVIOURS.iter().copied().collect();
    assert_eq!(
        declared.len(),
        BEHAVIOURS.len(),
        "`BEHAVIOURS` repeats an entry"
    );
    let named: BTreeSet<&str> = A_CLASS.iter().map(|row| row.behaviour).collect();
    for behaviour in &named {
        assert!(
            declared.contains(behaviour),
            "`{behaviour}` is not part of the A-class closed set"
        );
    }
    for behaviour in &declared {
        assert!(
            named.contains(behaviour),
            "no frozen test covers the A-class behaviour `{behaviour}`"
        );
    }

    // 2. Every row resolves to a real test, and that test is not skipped.
    for row in A_CLASS {
        let source = read(row.file);
        assert!(
            defines_fn(&source, row.test),
            "`{}` does not define `{}`, so `{}` is not frozen by it",
            row.file,
            row.test,
            row.behaviour
        );
        assert!(
            !is_ignored(&source, row.test),
            "`{}` in `{}` is `#[ignore]`d, so it cannot freeze `{}`",
            row.test,
            row.file,
            row.behaviour
        );
    }

    // 3. The log carries the table: a green run still says what it froze.
    println!("[P6.01] the A-class regression suite ({} rows):", A_CLASS.len());
    for behaviour in BEHAVIOURS {
        for row in A_CLASS.iter().filter(|row| row.behaviour == *behaviour) {
            println!("[P6.01]   {behaviour}: {}::{}", row.file, row.test);
        }
    }
}

#[test]
fn the_pre_existing_desktop_probes_are_still_present_and_still_ignored() {
    assert!(
        !DESKTOP_ONLY.is_empty(),
        "the pre-existing real-desktop probes are not listed, so `P6.01` cannot show they were kept"
    );

    for row in DESKTOP_ONLY {
        let source = read(row.file);
        assert!(
            defines_fn(&source, row.test),
            "the pre-existing desktop probe `{}` in `{}` is gone",
            row.test,
            row.file
        );
        assert!(
            is_ignored(&source, row.test),
            "`{}` in `{}` runs without a real desktop session, which `P6` 退出条件 ① forbids",
            row.test,
            row.file
        );
    }

    println!(
        "[P6.01] {} pre-existing real-desktop probes are still `#[ignore]`d",
        DESKTOP_ONLY.len()
    );
}
