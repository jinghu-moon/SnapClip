# 面板图标（Tabler）

目录里现在有 18 个 SVG（都已烘焙成 `.png` + `.rgba`）。下表是**已经在面板上用到的** 7 个；其余 11 个
（`folder` / `folder-open` / `refresh` / `braces` / `copy` / `info-circle` / `grip-vertical` /
`chevrons-left-right` / `link-off` / `minus` / `plus`）已烘好但还没有 `Glyph` 变体 —— 按"不留没有
消费者的实现"，等各自的调用点出现时再加。

| 文件 | 用在哪 | 来源 |
| --- | --- | --- |
| `swap.svg` | 尺寸卡：互换宽高 | Tabler |
| `center.svg` | 位置卡：居中到屏幕 | Tabler |
| `origin.svg` | 位置卡：移到原点 | Tabler |
| `close.svg` | 面板右上角关闭 | Tabler |
| `chevron.svg` | 预设下拉的箭头（展开时上下翻转） | Tabler |
| `check.svg` | 下拉里选中那一项的勾 | Tabler |
| `link.svg` | 保存面板：尺寸行两格之间的锁定 | 设计稿自带的 `ICON.link` |

`swap`…`check` 是 [Tabler Icons](https://tabler.io/icons)（MIT）的原文件。`link.svg` **不是** Tabler 的文件，它是
设计稿 `docs/design/save-panel-demo-v3.html:709` 里 `ICON.link` 的**描边链条**：三条子路径逐字照抄，只补了 XML 命名
空间，一个坐标都没改。设计稿里它是 16 单位，而烘焙工具读文件自己的尺寸，所以它原样放进来了 —— 不再需要手工包一层
`scale(1.5)`。（Tabler 的 `icons-tabler-filled` 版本是**填充**的，与设计稿不是同一个图形，所以没有采用。）

设计稿 key → 上游名的对照，供以后对表：`link` → 设计稿（无上游）· `unlink` → Tabler `link-off` ·
`reset` → Tabler `refresh` · `folderOpen` → Tabler `folder-open` · `check` → Tabler `check`。

面板上画出来的就是这些文件本身，**不是照着它们重画的线段**。

## 管线：SVG → PNG（resvg）→ Direct2D

```
assets/icon/*.svg ──(cargo run -p icon-bake, resvg)──▶ assets/icon/*.png
                                                          │ include_bytes!
                                                          ▼
                              d2d/panel_icon.rs: WIC 解码 → ID2D1Bitmap（白、透明、96×96）
                                                          │ CLSID_D2D1ColorMatrix 按状态色着色
                                                          ▼
                                            ID2D1Bitmap1（预着色，96×96）
                                                          │ ID2D1DeviceContext::DrawBitmap
                                                          ▼
                                              面板上的 14 dip 图形（目标矩形负责缩放）
```

- **为什么烘而不是运行时解析**：Direct2D 自带的 SVG（`DrawSvgDocument`）在这台机器上一个像素都不画
  （实测记录见下），而"用线段重画"永远达不到"和设计稿一样"。所以让**真正的 SVG 渲染器**（resvg）
  出图，浮层只负责贴图与着色。
- **`resvg` 只出现在 `tools/icon-bake`**：它是开发期工具。浮层用的是烘好的 PNG，所以 capture crate
  与 app 都不引入 SVG 依赖。
- **96×96**：画布按**文件自己的尺寸**缩放而来（24 单位 ⇒ ×4；16 单位 ⇒ ×6），覆盖到 400% 缩放；
  面板按 14 dip 往下缩，这一步是超采样下采样。
- **画布来自文件，不是假设的**：工具读 `usvg` 给出的固有尺寸再缩放，所以 16 单位的文件也**铺满画布**。
  以前写死 24 单位，16 单位的文件会以 2/3 大小挤在左上角，唯一的办法是手工包一层 `scale(1.5)`。
- **PNG 里的位置与 SVG 里一致**：工具只把画布缩放到铺满，**不移动**图形。曾经做过"测 alpha bbox 再把墨平移到画布
  中心"，那是**光学居中**，而设计稿摆在哪就是哪 —— 烘焙不该替设计师重摆一遍。那次改动把 3 个"看着对称、其实不对称"
  的图标（`check` / `folder` / `folder-open`）悄悄挪了 2 px，所以现在这条被明确写死：**PNG 就是缩放后的 SVG**。
- **不可预测的 SVG 会被拒绝**：`<text>`/`<tspan>`/`<mask>`/`<image>`/动画/外部 `href`/`filter` 一律不烘 ——
  换台机器或换个 resvg 小版本就会画出别的样子，而这种漂移**在只看本机对比的测试里是看不见的**。
- **白色 + 透明底**：颜色留到绘制时给，所以一张 PNG 服务所有状态。
- **改动 SVG 之后必须重烘**：`cargo run -p icon-bake`。忘了也没关系 —— `cargo test -p icon-bake`
  会重新渲染并逐字节比对，不一致就红。
- **PNG 编码器用 `tiny_skia` 的，不手写 DEFLATE**：另外实现过一版手写固定 Huffman + LZ77 的 DEFLATE，实测产物小
  35–60%。但它换来的只是**二进制体积**：PNG 只在启动时被解码一次成 96×96 的位图，之后**显存与每帧成本与压缩率无关**
  —— 所以这不是性能优化，只是体积优化。按现有 18 张 PNG 合计 **36 KB** 算，省下来是 **13–22 KB**，不值得为此维护
  一百八十行压缩代码。

## 换图标 / 加图标时

1. 把 SVG 放进 `assets/icon/`（`currentColor` 由烘焙工具换成 `#ffffff`）。`viewBox` 是多少都行 ——
   工具按文件自己的尺寸缩放到铺满 96×96，所以 16 单位的文件可以直接放（`viewBox="0 0 16 16"`）。
   唯一会被拒绝的是上面列的"不可预测"元素。
2. 跑 `cargo run -p icon-bake`。
3. 在 `crates/snapclip-ui/src/panel/icons.rs` 的 `Glyph` 里加一个变体（`ICON_COUNT` / `ALL` / `name()` /
   `png()` 各一处），再在调用点用 `Glyph::新名字`。
4. 因为 resvg 渲染，**不再有"Direct2D 的 SVG 子集"这类限制**（`<style>`、`<use>`、渐变、clip 都能画）。
   测试会盯住"每个变体指向不同文件""PNG 是 96×96 RGBA""PNG 与 SVG 一致"。

## 改完跑一次这两条

```
cargo test -p icon-bake                                              # 产物是否跟 SVG 一致、预检是否拦得住
cargo test -p snapclip-capture --lib -- --ignored --test-threads=1 the_panel_glyphs the_glyph_tint
```

第一条里除了"PNG 与 SVG 一致"，还有一条是**对着已提交的产物**断言的：`.rgba` 是**直通**像素。位置那条不需要单独
的测试 —— "已提交产物 == 渲染结果"本身就把位置钉住了（渲染不移动图形，产物一旦被挪过就红）。

第二条在真 GPU 上画面板，按"每个图形自己方框里的墨"断言：图形落进自己的格子、墨量是线稿而不是
一整块、并且**换一种状态色画出来的像素必须不同**（颜色矩阵没生效就会红）。

## 第三种核对：换个渲染器（`compare.html`）

上面第一条比的是**已提交产物 ↔ 现在重新渲染同一个 SVG** —— 同一个渲染器、同一份输入。它抓得到"忘了重烘"
和"有人手改过 PNG"，**抓不到"渲染器把 SVG 画错了"**。而 SVG 是矢量描述、本身没有像素，所以"SVG 与 PNG
逐像素一致"这件事**根本无法直接比较**，只能**经由某个渲染器**。于是第三条核对是：

> **我们的渲染 ↔ 另一个渲染器**（浏览器），即"我们画得忠不忠实"。

用法（**不需要服务器**，双击打开即可）：

1. 打开 `assets/icon/compare.html`；
2. 点「选择 assets/icon 文件夹」（或把文件拖进页面）—— 同名配对：`link.svg` ↔ `link.png`；
3. 看**结论表**：每个图标一行 —— 差异像素数、占比、最大通道差、alpha 差异、**最大差异块**、判定；
4. 拖动「容差」滑块重算；勾「只比 alpha」可忽略颜色、只看形状；「导出报告」得到制表符分隔的文本；
5. 以后加图标时，把新 SVG 拖进页面就能复检。

**判定规则**：差异占比 < 0.5% 且**最大差异块**很小 ⇒ "形状一致：差异只在边缘抗锯齿"；一旦差异**成片**
（最大 4 连通差异区域 > 200 px）⇒ "不一致：形状可能不同"。第二个判据是必要的 —— 抗锯齿差异是**细边**，
形状不同是**块**。

**为什么不做成单元测试**：两个渲染器的抗锯齿**必然不同**，逐像素相等会**假红**；而放宽容差到能过，
就等于**削弱断言**。所以它是一次**人眼可复核的对照**，不是门禁。

**一处必要的替换**：源文件用 `currentColor`，而在 `<img>` 里它解析成**黑色** —— 页面注入
`<style>:root{color:#ffffff}</style>` 强制白色，与 `icon-bake` 把 `currentColor` 换成白色是同一件事。
若某个 SVG 写死了别的颜色，用「只比 alpha」即可。

---

# 历史：为什么不再走 Direct2D 的原生 SVG

下面这段是 2026-05 的实测记录，保留下来是因为它解释了这条管线为什么长这样。
**不要再走 `DrawSvgDocument`**。

已确认可用的部分：

- `ID2D1DeviceContext5::CreateSvgDocument` 存在，8 个文件全部解析成功
  （`CreateStreamOnHGlobal` → `IStream` → `CreateSvgDocument`）。
- `ID2D1SvgDocument::GetRoot` + `ID2D1SvgElement::SetAttributeValue2(POD_TYPE_COLOR)` 可用。
- 一个坑：把 `SetTransform` 打在 `ID2D1DeviceContext5` 上、或复位时传空指针，**会直接
  ACCESS_VIOLATION（0xc0000005）**，在应用里表现为"点尺寸栏就闪退"。

当时卡住的地方：

- **`DrawSvgDocument` 一个像素都不画。** 不是位置错，是完全不出现。排除过：变换（有/无/仅平移/
  平移+缩放）、`SetViewportSize`、纯白不透明颜色、目标位图与文档是否同一 device，**连手写的
  `<svg viewBox="0 0 24 24"><rect fill="#ffffff"/></svg>` 也不画**。解析、着色、变换都对，
  只有最后一步是空的。
- 一次**误诊**也记下来：这批文件把 `fill`/`stroke` 写在根 `<svg>` 上靠继承，我一度认定"Direct2D 不做
  根属性继承"是根因，写了"把根画法解析出来显式盖到每个形状"（还要避开 Tabler 那个透明底框）。
  手写 SVG 用显式 `fill` 同样不画 ⇒ 方向错了，那次改动已撤回。

结论：原生 SVG 这条路在这台机器上不成立，而"线段重画"达不到设计稿的还原度 —— 于是改成
**让 resvg 出图、浮层贴图**，也就是上面那套管线。
