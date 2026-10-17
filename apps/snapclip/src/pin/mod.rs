//! 贴图：把一块像素钉在最上层，供对照阅读。
//!
//! 对外只暴露 [`PinManager`]。它做三件事：
//!
//! - 起那条拥有全部贴图窗口的线程（[`host::PinWindowHost`]）；
//! - 把 F6 的取图来源（[`PinHome`]）交给它——热键是窗口消息，直接落在那条线程上，
//!   所以取图也必须发生在那里；
//! - 把宿主线程报回来的 `PinEvent::Closed` 收掉并记账，不让那条有界通道积压。
//!
//! 模块边界：这里**不**截屏、也不找窗口。像素由 [`PinHome`] 交进来，所以这个模块
//! 不认识 `snapclip-capture`；[`PinHome`] 由外壳实现，唯一知道怎么从捕获那边取走
//! 刚确认的选区。
//!
//! # 入口
//!
//! 第一版**只有 F6 一个入口**，而 F6 贴的就是用户刚按 F5 截下来的那张图：宿主线程
//! 在泵到 `WM_HOTKEY` 时向 [`PinHome`] 要一次选区，拿到就贴，拿不到（开机后还没截过
//! 图、或者那张图已经贴过一次了）就只在日志里说一句。没有托盘菜单，也没有「贴一块
//! 调用方备好的像素」的公开方法——那两条入口等真有调用方时再加，现在加等于给没人走
//! 的代码写测试。
//!
//! F6 走的是宿主线程自己的 `RegisterHotKey`（[`host`]），不是捕获 crate 的
//! `register_scoped`：后者是**会话级**热键的配对台账（覆盖层的 Esc 就是这么用的），
//! 而 F6 是进程级的，生命周期与宿主线程完全相同。
//!
//! # 编号从哪来
//!
//! F6 那条路径的编号由宿主线程自己发（[`host::HostState::take_id`]），而且必须
//! **先确认有图**再发：还没截图就按 F6 什么都不发生，不该在编号表里留下一个洞。
//! 调用方路径（若将来有）相反：句柄要同步返回，号必须先发好，用
//! [`PinManager::take_id`]。

pub mod host;
pub mod source;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use snapclip_model::geometry::Rect;

pub use host::{PinEvent, PinHandle, PinPoster, PinRequest, PinWindowHost};
pub use source::ImageSource;

/// 一块被选中的像素，以及它在屏幕上的位置——F6 要贴的东西。
///
/// `pixels` 是 `region` 大小、自上而下的 BGRA。`Arc<Vec<u8>>` 不是装饰：这块缓冲动辄
/// 几 MB，而它要在捕获线程、宿主线程之间过手，引用计数是唯一不产生副本的搬法。
/// 为什么是 `Arc<Vec<u8>>` 而不是更漂亮的 `Arc<[u8]>`：生产者只借得到 `&[u8]`
/// （写盘回调的参数），而 `Arc<[u8]>` 只能从拥有所有权的值构造。写成切片就必然
/// 先克隆一份，那正是这条路径要避免的。
#[derive(Debug, Clone)]
pub struct PinSelection {
    /// 像素在**虚拟桌面**物理像素里的位置。
    ///
    /// 这是捕获那边唯一必须一起交出来的东西：选区本身是显示器局部坐标，离开它
    /// 就不知道该把窗口放哪儿。
    pub region: Rect,
    pub pixels: Arc<Vec<u8>>,
    /// 成图四角的圆角半径（`region` 自己的物理像素，0 = 直角）。
    ///
    /// 半径必须跟着这块像素走，而不是从"当前偏好"再读一次：贴图是一张**已经拍摄的图**，
    /// 它的形状在拍摄那一刻就定了，之后用户在面板里改圆角不该改变已经贴出去的那张。
    pub radius: i32,
}

impl PinSelection {
    /// 这块选区的 BGRA 字节，长度恰好是 `region` 面积 × 4。
    pub fn bytes(&self) -> &[u8] {
        &self.pixels
    }
}

/// 谁能在 F6 按下时交出「刚截的那张图」。
///
/// 一个只有一条方法的接口，存在的理由是**方向**：像素存在捕获那边（那里才有冻结
/// 帧的回读结果），而 F6 落在贴图自己的线程上。让贴图模块去 `use snapclip_capture`
/// 会把两个能力 crate 绑死；让外壳实现这个接口，贴图只知道「有人能给我像素」。
///
/// 实现必须是 `Send + Sync`：调用发生在宿主线程。返回 `None` 表示还没有可贴的东西
/// （开机后还没按过 F5），这不是错误。
pub trait PinHome: Send + Sync + 'static {
    fn take_selection(&self) -> Option<PinSelection>;
}

/// 贴图管理器。整个进程一个。
///
/// 克隆是廉价的，而且克隆出来的是**同一份**管理：同一条宿主线程、同一本编号表。
#[derive(Clone)]
pub struct PinManager {
    inner: Arc<PinManagerInner>,
}

struct PinManagerInner {
    host: PinWindowHost,
    /// 下一个贴图编号。理由见模块头。
    next_id: AtomicU64,
}

impl PinManager {
    /// 拿一个可以跨线程调用的贴图手柄：调用它等于用户按了一次 `F6`。
    pub fn pin_poster(&self) -> PinPoster {
        self.inner.host.pin_poster()
    }

    /// 起贴图管理器。它会在后台起一条宿主线程。
    ///
    /// 这条线程在贴第一张图之前只是一条阻塞在 `GetMessageW` 上的空泵，
    /// 所以启动它是廉价的。
    ///
    /// `home` 是 F6 的取图来源，被搬进那条线程；它必须在 `PinManager` 之前构造好。
    pub fn start(home: Arc<dyn PinHome>) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(PinManagerInner {
                host: PinWindowHost::spawn(home).context("启动贴图宿主线程失败")?,
                next_id: AtomicU64::new(1),
            }),
        })
    }

    /// 发一个贴图编号。理由见模块头。
    pub fn take_id(&self) -> u64 {
        self.inner.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 把宿主线程报回来的事件收掉，汇总成一份报告（创建 / 关闭 / 没能创建）。
    ///
    /// 非阻塞，调用方应当在事件循环里顺手轮询。三种事件必须分开数：把"创建后被关闭"
    /// 当成"没能创建"会让日志长期说谎（docs/23 §14.55.9.6）。
    ///
    /// 必须有人轮询：那条通道是有界的，没人收就会在攒够 64 条之后开始丢。
    pub fn drain_events(&self) -> host::PinReport {
        let mut events = Vec::new();
        while let Some(event) = self.inner.host.try_recv_event() {
            events.push(event);
        }
        host::summarize(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_start_at_one() {
        // 编号必须单调：句柄靠它认窗口。
        let counter = AtomicU64::new(1);
        let a = counter.fetch_add(1, Ordering::Relaxed);
        let b = counter.fetch_add(1, Ordering::Relaxed);
        assert_eq!((a, b), (1, 2));
    }

    #[test]
    fn an_empty_region_is_never_a_selection() {
        // 空选区不该走到宿主线程：贴一块 0×0 的图没有意义，而 `Rect::is_empty`
        // 是判断它的唯一方式（`Rect` 没有负尺寸的表达）。
        assert!(Rect::new(10, 10, 10, 50).is_empty());
        assert!(Rect::new(10, 10, 50, 10).is_empty());
        assert!(!Rect::new(10, 10, 11, 11).is_empty());
    }
}
