//! 光标底下那个窗口：贴图的默认目标。
//!
//! F6 的语义是「把光标指着的那个窗口贴出来」。这个文件只回答一个问题：
//! 光标在哪个窗口上、那个窗口在屏幕上的矩形是什么。
//!
//! # 为什么复用捕获那边的判断
//!
//! 「哪个窗口值得截图」是捕获早就解决过的问题：排除自己、排除正在创建的窗口、
//! 跳过桌面和任务栏这些壳表面。这里**不重新实现**它，而是用
//! [`snapclip_capture::window_detection`] 导出的同一份排除表和同一个壳表面名单。
//! 否则同一个光标位置，截图和贴图会给出两个不同的答案——那是要被解释的 bug。
//!
//! 这里**不**做的是捕获那边更重的事：UIA 深挖（贴图不需要「元素级」的目标）、
//! DWM 边框补偿（贴图要的是窗口的完整外框，包括阴影之外的可见区域）、
//! 以及悬停缓存。贴图是按下 F6 那一瞬间的一次判定，不是持续跟踪。

use anyhow::{Context, Result};
use snapclip_capture::window_detection::{Exclusions, is_shell_surface_class};
use snapclip_model::geometry::Rect;
use windows_sys::Win32::Foundation::{HWND, MAX_PATH, POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClassNameW, GetWindowRect, GetWindowThreadProcessId, WindowFromPoint,
};

/// 一次贴图的目标：一个顶层窗口和它在屏幕上的矩形。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinTarget {
    /// 顶层窗口句柄，只剩身份用途。
    pub hwnd: isize,
    /// 窗口在屏幕上的矩形（含边框，不含 DWM 投影）。
    pub rect: Rect,
    /// 窗口类名，只用于日志。
    pub class_name: String,
}

/// 找光标底下的窗口。
///
/// 返回 `None` 表示「这里没有可以贴的东西」，分三种情况：光标落在壳表面上
/// （桌面、任务栏）、落在本进程自己的窗口上、或者窗口没有可见矩形。这三种都
/// 不是错误——用户对着桌面按 F6，什么都不发生是正确行为。
pub fn target_at_point(point: (i32, i32)) -> Option<PinTarget> {
    target_at_point_with(point, &this_process_exclusions())
}

/// 本进程要排除的东西。
///
/// `Exclusions::new()` 是**空集**——排除表本身不知道要排除谁，得由建它的人填。
/// 捕获那边在跑覆盖层时填（`windows/overlay.rs:595-597`），贴图这里也要填同一
/// 份，否则第二次按 F6 会把上一次的贴图当成目标再贴一遍：贴图窗口属于本进程，
/// 而且它正好压在最上层。
///
/// 这里没有具体的 HWND 要排除（贴图窗口在宿主线程上按需创建，且创建时贴图
/// 还不存在），所以只填进程级那一条。进程级排除本来就是这个用途：兜住进程之后
/// 才创建的窗口。
fn this_process_exclusions() -> Exclusions {
    let mut exclusions = Exclusions::new();
    exclusions.exclude_process(std::process::id());
    exclusions
}

/// 同上，但显式传入排除表，便于测试。
pub fn target_at_point_with(point: (i32, i32), exclusions: &Exclusions) -> Option<PinTarget> {
    let hit = unsafe { WindowFromPoint(POINT { x: point.0, y: point.1 }) };
    if hit.is_null() {
        return None;
    }
    // 取根窗口：点在一个控件上时，用户的意图是「那个控件所属的窗口」，
    // 而不是一条 20 像素高的输入框。贴一个小控件也没有任何阅读价值。
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    let root = if root.is_null() { hit } else { root };

    let process_id = process_id_of(root)?;
    if exclusions.excludes(root as isize, process_id) {
        return None;
    }
    let class_name = class_name_of(root).unwrap_or_default();
    if is_shell_surface_class(&class_name) {
        return None;
    }
    let rect = window_rect(root)?;
    Some(PinTarget {
        hwnd: root as isize,
        rect,
        class_name,
    })
}

/// 窗口在屏幕上的矩形。空矩形或读取失败都返回 `None`。
pub fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return None;
    }
    let rect = Rect::new(rect.left, rect.top, rect.right, rect.bottom);
    if rect.is_empty() { None } else { Some(rect) }
}

/// 窗口所属进程。读不到就返回 `None`——读不到进程 id 的窗口不该被贴。
fn process_id_of(hwnd: HWND) -> Option<u32> {
    let mut process_id: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    if process_id == 0 { None } else { Some(process_id) }
}

/// 窗口类名。`GetClassNameW` 在缓冲区不够时返回 0 而不写完整名字，
/// 这种情况按「不知道类名」处理，而不是拿半截字符串去比对壳表面名单。
fn class_name_of(hwnd: HWND) -> Result<String> {
    let mut buffer = [0u16; MAX_PATH as usize];
    let written = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if written <= 0 {
        anyhow::bail!("GetClassNameW 失败");
    }
    String::from_utf16(&buffer[..written as usize]).context("窗口类名不是合法的 UTF-16")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_surface_classes_are_recognised() {
        // 这份名单归捕获那边所有，这里只确认复用是生效的：桌面和任务栏必须被跳过。
        assert!(is_shell_surface_class("Progman"));
        assert!(is_shell_surface_class("shell_traywnd"));
        assert!(!is_shell_surface_class("Notepad"));
    }

    #[test]
    fn empty_rect_is_not_a_target() {
        // `GetWindowRect` 不可能返回空矩形，但边界检查在这儿：所有下游都假设
        // 目标有正面积。
        let rect = Rect::new(10, 10, 10, 10);
        assert!(rect.is_empty());
    }

    #[test]
    fn exclusions_cover_this_process() {
        // 贴图窗口属于本进程，排除表必须把它排除掉，否则连按两次 F6 会把贴图
        // 自己再贴一遍。这条断言盯的不是捕获那边，而是本模块**填过**排除表：
        // `Exclusions::new()` 是空集，忘了填这里就会红。
        let exclusions = this_process_exclusions();
        assert!(exclusions.excludes(0x1234, std::process::id()));
    }
}
